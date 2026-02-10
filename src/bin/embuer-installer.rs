/*
    embuer: an embedded software updater DBUS daemon and CLI interface
    Copyright (C) 2025  Denis Benato

    This program is free software; you can redistribute it and/or modify
    it under the terms of the GNU General Public License as published by
    the Free Software Foundation; either version 2 of the License, or
    (at your option) any later version.

    This program is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
    GNU General Public License for more details.

    You should have received a copy of the GNU General Public License along
    with this program; if not, write to the Free Software Foundation, Inc.,
    51 Franklin Street, Fifth Floor, Boston, MA 02110-1301 USA.
*/

extern crate sys_mount;

use std::{collections::VecDeque, sync::Arc};

use argh::FromArgs;
use embuer::manifest::Manifest;
use futures::TryStreamExt;
use log::{debug, error, info, warn};
use reqwest::Client;
use std::pin::Pin;
use tokio::process::Command;
use tokio_util::io::StreamReader;

/// Embuer Client - Control and monitor the Embuer update service
#[derive(FromArgs)]
struct EmbuerInstallCli {
    #[argh(
        option,
        description = "device in /dev to bootstrap the system into (e.g., /dev/nvme0n1)",
        short = 'd'
    )]
    pub device: Option<std::path::PathBuf>,

    #[argh(
        option,
        description = "image file to bootstrap the system into",
        short = 'i'
    )]
    pub image: Option<std::path::PathBuf>,

    #[argh(option, description = "image file size if creating a new one (in GiB)")]
    pub image_size: Option<usize>,

    #[argh(
        option,
        description = "source of the deployment: either URL, file or manual for a manual (or scripted) installation",
        short = 's'
    )]
    pub deployment_source: String,

    #[argh(option, description = "name of the deployment", short = 'k')]
    pub deployment_name: String,

    #[argh(
        option,
        description = "script to be used for manual installations, full manual mode when unspecified"
    )]
    pub manual_script: Option<String>,

    #[argh(
        option,
        description = "path of the kernel to compile and install (path to sources)",
        short = 'm'
    )]
    pub manual_kernel: Option<String>,

    #[argh(
        option,
        description = "kernel defconfig to use for manual kernel compilation (use when --manual-kernel is specified, uses .config if not specified)",
        short = 'f'
    )]
    pub manual_kernel_defconfig: Option<String>,

    #[argh(
        option,
        description = "architecture of the machine (e.g., x86_64, arm64) in addition to kernel names the following are available: imx8",
        short = 'a'
    )]
    pub arch: Option<String>,

    #[argh(
        option,
        description = "bootloader to install (refind, imx8://<file>)",
        short = 'b'
    )]
    pub bootloader: Option<String>,

    #[argh(option, description = "kernel cmdline", short = 'c')]
    pub cmdline: Option<String>,

    #[argh(option, description = "name of the installation", short = 'n')]
    pub name: Option<String>,

    #[argh(option, description = "wait for input before exiting", short = 'w')]
    pub wait: Option<bool>,
}

enum Architecture {
    GenericAMD64,
    GenericAarch64,
    IMX8,
}

enum Bootloader {
    Refind,
    IMX8(std::path::PathBuf)
}

enum MountType {
    Image(String),
    Device(std::path::PathBuf),
}

impl Drop for MountType {
    fn drop(&mut self) {
        match self {
            MountType::Image(loopdev) => {
                info!("Unmounting and detaching loop device: {}", loopdev);
                std::process::Command::new("losetup")
                    .arg("-d")
                    .arg(loopdev)
                    .status()
                    .unwrap();
            }
            Self::Device(path) => {
                info!("Unmounting and detaching device: {}", path.display());
                std::process::Command::new("umount")
                    .arg(path)
                    .status()
                    .unwrap();
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Debug)
        .init();

    let cli: EmbuerInstallCli = argh::from_env();

    let mut mounts: VecDeque<MountType> = VecDeque::new();

    let base_mount_path = std::env::temp_dir().join("embuer_mnt");
    std::fs::create_dir_all(&base_mount_path)?;

    let name = match cli.name.as_ref() {
        Some(n) => n.clone(),
        None => "embuer".to_string(),
    };

    let device_partition = match (cli.device.as_ref(), cli.image.as_ref()) {
        (Some(dev), None) => dev.clone(),
        (None, Some(img)) => {
            if !img.exists() {
                warn!(
                    "Image file {} does not exist: creating it...",
                    img.display()
                );
                let size = cli.image_size.unwrap_or(2);
                if size == 0 {
                    error!("Error: image size must be greater than 0");
                    return Err(Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "Image size must be greater than 0",
                    )) as Box<dyn std::error::Error>);
                }
                Command::new("fallocate")
                    .arg("-l")
                    .arg(format!("{size}G"))
                    .arg(img)
                    .output()
                    .await?;
            }

            // Setup loop device, example command:
            // losetup -P -f --show test.img
            let output = Command::new("losetup")
                .arg("-P")
                .arg("-f")
                .arg("--show")
                .arg(img)
                .output()
                .await?;

            let loopdev = String::from_utf8_lossy(&output.stdout).trim().to_string();
            info!("Setup loop device: {loopdev}");

            mounts.push_front(MountType::Image(loopdev.clone()));

            loopdev.into()
        }
        (Some(_), Some(_)) => {
            eprintln!("Error: only one of --device or --image can be specified");
            std::process::exit(1)
        }
        (None, None) => {
            eprintln!("Error: either --device or --image must be specified");
            std::process::exit(1)
        }
    };

    // Parse architecture option into internal enum
    let architecture = match cli.arch.as_deref() {
        Some("x86_64") | Some("amd64") => Architecture::GenericAMD64,
        Some("arm64") | Some("aarch64") => Architecture::GenericAarch64,
        Some("imx8") => Architecture::IMX8,
        Some(other) => {
            warn!("Unknown architecture '{}', defaulting to x86_64", other);
            Architecture::GenericAMD64
        }
        None => {
            info!("No architecture specified, defaulting to x86_64");
            Architecture::GenericAMD64
        }
    };

    // Parse bootloader option. Supported forms:
    // - "refind" (will pick the rEFInd variant based on architecture)
    // - "imx8://<file>" (IMX8 bootloader file)
    let bootloader: Option<Bootloader> = match cli.bootloader.as_deref() {
        Some(s) => {
            if s.starts_with("imx8://") {
                let path = s.trim_start_matches("imx8://");
                let path = std::path::PathBuf::from(path);

                if !path.exists() {
                    error!("Specified IMX8 bootloader file does not exist: {}", path.display());
                    return Err(Box::new(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "Specified IMX8 bootloader file does not exist",
                    )) as Box<dyn std::error::Error>);
                }

                Some(Bootloader::IMX8(path))
            } else if s == "refind" {
                Some(Bootloader::Refind)
            } else {
                warn!("Unsupported bootloader string provided: {}", s);
                None
            }
        }
        None => None,
    };

    // Create a partition table
    info!(
        "Creating partition table on device {}...",
        device_partition.display()
    );
    Command::new("parted")
        .arg("-s")
        .arg(&device_partition)
        .arg("mklabel")
        .arg("gpt")
        .status()
        .await?;

    let rootfs_partition_offset = match bootloader {
        Some(Bootloader::IMX8(_)) => "16MiB",
        _ => "64MiB",
    };

    info!(
        "Creating btrfs partition on device {}...",
        device_partition.display()
    );
    Command::new("parted")
        .arg("-s")
        .arg(&device_partition)
        .arg("mkpart")
        .arg("primary")
        .arg("btrfs")
        .arg(rootfs_partition_offset)
        .arg("100%")
        .arg("type")
        .arg("2")
        .arg("4F68BCE3-E8CD-4DB1-96E7-FBCAF984B709")
        .status()
        .await
        .map_err(|e| e.to_string())?;
    let partition_rootfs = {
        let mut result = format!("{}", device_partition.display());
        if result.ends_with(char::is_numeric) {
            result = format!("{}p2", result);
        } else {
            result = format!("{}2", result);
        }

        result
    };

    info!("Formatting rootfs partition {} with btrfs...", partition_rootfs);
    Command::new("mkfs.btrfs")
        .arg("-f")
        .arg(&partition_rootfs)
        .arg("-L")
        .arg("rootfs")
        .status()
        .await?;

    let rootfs_mount_dir = {
        let rootfs_mount_point =
            std::path::PathBuf::from(format!("{}/rootfs", base_mount_path.display()));

        std::fs::create_dir_all(&rootfs_mount_point)?;

        info!(
            "Mounting rootfs partition {} to {}...",
            partition_rootfs,
            rootfs_mount_point.display()
        );

        Command::new("mount")
            .arg("-t")
            .arg("btrfs")
            .arg("-o")
            .arg("subvolid=5,compress-force=zstd:15,noatime,rw")
            .arg(&partition_rootfs)
            .arg(&rootfs_mount_point)
            .status()
            .await
            .map_err(|e| e.to_string())?;

        mounts.push_front(MountType::Device(std::path::PathBuf::from(&partition_rootfs)));

        rootfs_mount_point
    };

    let partuuid_find = Command::new("blkid")
        .arg("-s")
        .arg("PARTUUID")
        .arg("-o")
        .arg("value")
        .arg(&partition_rootfs)
        .output()
        .await?;

    let rootfs_partuuid = String::from_utf8_lossy(&partuuid_find.stdout)
        .trim()
        .to_string();

    info!("Rootfs PARTUUID: {rootfs_partuuid}");

    match bootloader {
        Some(Bootloader::Refind) => {
            info!("Installing bootloader: refind...");
            
        },
        Some(Bootloader::IMX8(path)) => {
            info!("Installing bootloader: IMX8 from file {}...", path.display());
            
            todo!()
        }
        None => warn!("No bootloader specified: skipping bootloader installation"),
    }

    match bootloader {
        Some(Bootloader::Refind) => {
            info!("Selected bootloader rEFInd requires an espo partition");

            // Create a fat32 boot partition for EFI of 512MiB
            info!(
                "Creating EFI boot partition on device {}...",
                device_partition.display()
            );
            Command::new("parted")
                .arg("-s")
                .arg(&device_partition)
                .arg("mkpart")
                .arg("primary")
                .arg("fat32")
                .arg("1MiB")
                .arg(rootfs_partition_offset)
                .arg("type")
                .arg("1")
                .arg("c12a7328-f81f-11d2-ba4b-00a0c93ec93b")
                .arg("set")
                .arg("1")
                .arg("esp")
                .arg("on")
                .status()
                .await?;
            let partition_esp = {
                let mut result = format!("{}", device_partition.display());
                if result.ends_with(char::is_numeric) {
                    result = format!("{}p1", result);
                } else {
                    result = format!("{}1", result);
                }

                result
            };

            info!("Formatting EFI partition {} with FAT32...", partition_esp);
            Command::new("mkfs.fat")
                .arg("-F32")
                .arg(&partition_esp)
                .status()
                .await?;

            // Mount ESP partition
            let esp_mount_dir = {
                let esp_mount_point =
                    std::path::PathBuf::from(format!("{}/esp", base_mount_path.display()));

                std::fs::create_dir_all(&esp_mount_point)?;

                info!(
                    "Mounting ESP partition {} to {}...",
                    partition_esp,
                    esp_mount_point.display()
                );

                Command::new("mount")
                    .arg(&partition_esp)
                    .arg(&esp_mount_point)
                    .status()
                    .await
                    .map_err(|e| e.to_string())?;

                mounts.push_front(MountType::Device(std::path::PathBuf::from(&partition_esp)));

                esp_mount_point
            };

            install_bootloader_refind(
                &esp_mount_dir,
                &architecture,
                rootfs_partuuid.as_str(),
                cli.cmdline.as_deref().unwrap_or(""),
                name.as_str(),
            )
            .await?;
        }
        Some(Bootloader::IMX8(path)) => {
            info!("Installing bootloader: IMX8 from file {}...", path.display());

            todo!()
        }
        None => {
            info!("No bootloader specified: skipping...");
        }
    }

    // Prepare the rootfs structure
    let btrfs = Arc::new(
        embuer::btrfs::Btrfs::new().map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?,
    );
    info!(
        "Prepare the rootfs from source image {}...",
        cli.deployment_source
    );
    let (deployments_dir, deployments_data_dir) =
        prepare_rootfs_partition(btrfs.clone(), &rootfs_mount_dir).await?;

    // From here on let the core component take over
    let deployment_name = cli.deployment_name.clone();
    match cli.deployment_source.as_str() {
        "manual" => {
            let (deployment_rootfs_dir, deployment_rootfs_data_dir) =
                prepare_deployment_directories(
                    btrfs.clone(),
                    &deployments_dir,
                    &deployments_data_dir,
                    &deployment_name,
                )
                .await?;

            // If a manual kernel source was specified, build and install it now
            if let Some(kernel_src) = cli.manual_kernel.as_ref() {
                info!(
                    "Manual kernel specified: building and installing from {}",
                    kernel_src
                );
                let kernel_path = std::path::Path::new(kernel_src);
                manual_kernel(
                    kernel_path,
                    cli.arch.as_deref(),
                    cli.manual_kernel_defconfig.as_deref(),
                    &deployment_rootfs_dir,
                )
                .await?;
            }

            match &cli.manual_script {
                Some(script_path) => {
                    info!("Executing manual installation script: {}", script_path);
                    let status = Command::new(script_path)
                        .arg(&deployment_rootfs_dir)
                        .arg(&deployment_rootfs_data_dir)
                        .status()
                        .await?;

                    if !status.success() {
                        error!(
                            "Manual installation script failed with exit code: {}",
                            status.code().unwrap_or(-1)
                        );
                        return Err(Box::new(std::io::Error::other(
                            "Manual installation script failed",
                        )) as Box<dyn std::error::Error>);
                    }
                }
                None => {
                    info!("No manual installation script specified: entering full manual mode.");

                    info!(
                        "Prepare {} and {} then press enter to complete the installation...",
                        deployment_rootfs_dir.display(),
                        deployment_rootfs_data_dir.display()
                    );
                    let mut input = String::new();
                    std::io::stdin().read_line(&mut input)?;
                }
            }

            let manifet_installed = deployment_rootfs_dir
                .clone()
                .join("usr")
                .join("share")
                .join("embuer")
                .join("manifest.json");

            if !manifet_installed.exists() {
                error!(
                    "Manifest file not found in the deployment: {}",
                    manifet_installed.display()
                );
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "Manifest file not found in the deployment",
                )) as Box<dyn std::error::Error>);
            }

            // Parse the manifest and decide whether the installed deployment should be read-only
            let manifest = match Manifest::from_file(&manifet_installed) {
                Ok(m) => m,
                Err(err) => {
                    error!("Failed to read manifest: {}", err);
                    return Err(Box::new(err) as Box<dyn std::error::Error>);
                }
            };

            let subvol_id = btrfs
                .btrfs_subvol_get_id(&deployment_rootfs_dir)
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

            if manifest.is_readonly() {
                info!("Manifest requests read-only deployment; setting subvolume ID {subvol_id} to read-only...");
                btrfs
                    .subvolume_set_ro(&deployment_rootfs_dir)
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
            } else {
                info!("Manifest requests read-write deployment; ensuring subvolume ID {subvol_id} is read-write...");
                btrfs
                    .subvolume_set_rw(&deployment_rootfs_dir)
                    .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
            }

            let display_root_disk = rootfs_mount_dir.display();
            info!("Setting the default subvolume of {display_root_disk} to {deployment_name} ({subvol_id})...");
            btrfs
                .subvolume_set_default(subvol_id, &rootfs_mount_dir)
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
        }
        src => {
            // Determine source stream: prefer local file if it exists, otherwise try HTTP(S).
            let local_path = std::path::PathBuf::from(&src);
            let wrapped_reader: Pin<Box<dyn tokio::io::AsyncRead + Send + Unpin>> =
                if local_path.exists() {
                    info!(
                        "Using local file as deployment source: {}",
                        local_path.display()
                    );
                    let file = tokio::fs::File::open(&local_path).await?;
                    Box::pin(file)
                } else if cli.deployment_source.as_str().starts_with("https://")
                    || cli.deployment_source.as_str().starts_with("http://")
                {
                    let url = cli.deployment_source.clone();
                    info!("Downloading deployment from URL: {}", url);

                    let client = Client::new();
                    let resp = client
                        .get(&url)
                        .send()
                        .await
                        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

                    if !resp.status().is_success() {
                        error!("Failed to download {}: HTTP {}", url, resp.status());
                        return Err(Box::new(std::io::Error::other("Failed to download update"))
                            as Box<dyn std::error::Error>);
                    }

                    let byte_stream = resp.bytes_stream().map_err(std::io::Error::other);
                    let stream_reader = StreamReader::new(byte_stream);

                    // The core.install_update path handles xz decompression internally.
                    // Just pass the raw HTTP stream reader through.
                    Box::pin(stream_reader)
                } else {
                    error!(
                        "Deployment source not found or unsupported: {}",
                        cli.deployment_source
                    );
                    return Err(Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "Deployment source not found or unsupported",
                    )) as Box<dyn std::error::Error>);
                };

            // Single unified install call
            let installed_deployment_name = match embuer::core::install_update(
                None,
                rootfs_mount_dir.clone(),
                deployments_dir.clone(),
                deployment_name.clone(),
                &btrfs,
                wrapped_reader,
            )
            .await
            {
                Ok(name) => match name {
                    Some(name) => {
                        info!("Successfully installed deployment: {name}");

                        if cli.wait.unwrap_or(false) {
                            info!("Press enter to continue...");
                            let mut input = String::new();
                            std::io::stdin().read_line(&mut input)?;
                        }

                        name
                    }
                    None => {
                        error!("Failed to install deployment: no deployment name returned");
                        return Err(
                            Box::new(std::io::Error::other("No deployment name returned"))
                                as Box<dyn std::error::Error>,
                        );
                    }
                },
                Err(e) => {
                    error!("Failed to install deployment: {}", e);
                    return Err(Box::new(e) as Box<dyn std::error::Error>);
                }
            };

            info!("Installed deployment: {}", installed_deployment_name);
        }
    };

    Ok(())
}

async fn manual_kernel(
    source_dir: &std::path::Path,
    arch: Option<&str>,
    defconfig: Option<&str>,
    deployment_rootfs_dir: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    build_kernel_manual(source_dir, arch, defconfig).await?;
    install_kernel_manual(source_dir, arch, deployment_rootfs_dir).await?;

    Ok(())
}

async fn kernel_cmd(
    source_dir: &std::path::Path,
    arch: Option<&str>,
    args: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    Command::new("make")
        .current_dir(source_dir)
        .arg("LLVM=1")
        .arg("LLVM_IAS=1")
        .arg(arch.map_or(String::new(), |a| format!("ARCH={}", a)))
        .args(args)
        .status()
        .await
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

    Ok(())
}

async fn build_kernel_manual(
    source_dir: &std::path::Path,
    arch: Option<&str>,
    defconfig: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Clean the tree
    kernel_cmd(source_dir, arch, &["mrproper"]).await?;

    // Configure: prefer provided defconfig, then existing .config, otherwise run defconfig
    if let Some(defc) = defconfig {
        kernel_cmd(source_dir, arch, &[defc]).await?;
    } else if !source_dir.join(".config").exists() {
        kernel_cmd(source_dir, arch, &["defconfig"]).await?;
    }

    // Determine number of parallel jobs
    let nproc_output = std::process::Command::new("nproc").output()?;
    let nproc = String::from_utf8_lossy(&nproc_output.stdout)
        .trim()
        .to_string();

    // Build kernel image and modules
    let mut build_args: Vec<String> = Vec::new();
    build_args.push("-j".to_string());
    build_args.push(nproc.clone());
    build_args.push("bzImage".to_string());
    build_args.push("modules".to_string());
    let build_args_refs: Vec<&str> = build_args.iter().map(|s| s.as_str()).collect();

    kernel_cmd(source_dir, arch, &build_args_refs).await?;

    Ok(())
}

async fn install_kernel_manual(
    build_dir: &std::path::Path,
    arch: Option<&str>,
    deployment_rootfs_dir: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    // Ensure /boot exists in the deployment rootfs
    let boot_dir = deployment_rootfs_dir.join("boot");
    std::fs::create_dir_all(&boot_dir)?;

    // Install modules into the target root
    let install_mod_arg = format!(
        "INSTALL_MOD_PATH={}/usr",
        deployment_rootfs_dir.display()
    );
    let mut mod_args: Vec<String> = Vec::new();
    mod_args.push("DEPMOD=/doesnt/exist".to_string());
    mod_args.push(install_mod_arg);
    mod_args.push("modules_install".to_string());
    let mod_args_refs: Vec<&str> = mod_args.iter().map(|s| s.as_str()).collect();
    kernel_cmd(build_dir, arch, &mod_args_refs).await?;

    // Try to locate the built kernel image
    let candidates = vec![
        build_dir
            .join("arch")
            .join("x86")
            .join("boot")
            .join("bzImage"),
        build_dir
            .join("arch")
            .join("x86")
            .join("boot")
            .join("vmlinuz"),
        build_dir
            .join("arch")
            .join("x86")
            .join("boot")
            .join("Image"),
        build_dir
            .join("arch")
            .join("arm64")
            .join("boot")
            .join("Image"),
        build_dir.join("vmlinux"),
    ];

    let image_path = candidates.into_iter().find(|p| p.exists());
    if let Some(img) = image_path {
        let dest = boot_dir.join(
            img.file_name()
                .unwrap_or_else(|| std::ffi::OsStr::new("vmlinuz")),
        );
        std::fs::copy(&img, &dest)?;
        info!("Installed kernel image to {}", dest.display());
    } else {
        warn!("Could not find built kernel image in source tree; skipping kernel image copy");
    }

    // Copy System.map if present
    let system_map = build_dir.join("System.map");
    if system_map.exists() {
        let dest = boot_dir.join("System.map");
        std::fs::copy(&system_map, &dest)?;
    }

    // Copy .config if present
    let config = build_dir.join(".config");
    if config.exists() {
        let dest = boot_dir.join("config");
        std::fs::copy(&config, &dest)?;
    }

    /*
    // Try running depmod to update module dependencies for the new root
    let _ = Command::new("depmod")
        .arg("-a")
        .arg("-b")
        .arg(deployment_rootfs_dir)
        .status()
        .await;
    */

    Ok(())
}

async fn prepare_deployment_directories(
    btrfs: Arc<embuer::btrfs::Btrfs>,
    deployments_dir: &std::path::Path,
    deployments_data_dir: &std::path::Path,
    deployment_name: &str,
) -> Result<(std::path::PathBuf, std::path::PathBuf), Box<dyn std::error::Error>> {
    let deployment_rootfs_dir = deployments_dir.join(deployment_name);
    let result = btrfs.subvolume_create(&deployment_rootfs_dir)?;
    debug!("{}", result.trim());

    let deployments_data_rootfs_dir = deployments_data_dir.join(deployment_name);
    let result = btrfs.subvolume_create(&deployments_data_rootfs_dir)?;
    debug!("{}", result.trim());

    std::fs::create_dir_all(deployments_data_rootfs_dir.join("etc_overlay/upperdir"))?;
    std::fs::create_dir_all(deployments_data_rootfs_dir.join("etc_overlay/workdir"))?;
    std::fs::create_dir_all(deployments_data_rootfs_dir.join("var_overlay/upperdir"))?;
    std::fs::create_dir_all(deployments_data_rootfs_dir.join("var_overlay/workdir"))?;
    std::fs::create_dir_all(deployments_data_rootfs_dir.join("root_overlay/upperdir"))?;
    std::fs::create_dir_all(deployments_data_rootfs_dir.join("root_overlay/workdir"))?;

    let usr_overlay_dir = deployments_data_rootfs_dir.join("usr_overlay");
    let result = btrfs.subvolume_create(&usr_overlay_dir)?;
    debug!("{}", result.trim());
    std::fs::create_dir_all(usr_overlay_dir.join("upperdir"))?;
    std::fs::create_dir_all(usr_overlay_dir.join("workdir"))?;

    let opt_overlay_dir = deployments_data_rootfs_dir.join("opt_overlay");
    let result = btrfs.subvolume_create(&opt_overlay_dir)?;
    debug!("{}", result.trim());
    std::fs::create_dir_all(opt_overlay_dir.join("upperdir"))?;
    std::fs::create_dir_all(opt_overlay_dir.join("workdir"))?;

    btrfs.subvolume_set_ro(&usr_overlay_dir)?;
    btrfs.subvolume_set_ro(&opt_overlay_dir)?;

    Ok((deployment_rootfs_dir, deployments_data_rootfs_dir))
}

async fn prepare_rootfs_partition(
    btrfs: Arc<embuer::btrfs::Btrfs>,
    rootfs_mount_dir: &std::path::Path,
) -> Result<(std::path::PathBuf, std::path::PathBuf), Box<dyn std::error::Error>> {
    let deployments_dir = rootfs_mount_dir.join("deployments");
    let result = btrfs.subvolume_create(&deployments_dir)?;
    debug!("{}", result.trim());

    let deployments_data_dir = rootfs_mount_dir.join("deployments_data");
    let result = btrfs.subvolume_create(&deployments_data_dir)?;
    debug!("{}", result.trim());

    Ok((deployments_dir, deployments_data_dir))
}

const ZIP_DATA: &[u8] = include_bytes!("../../refind-bin-0.14.2.zip");
async fn decompress_refind(
    destination: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Cursor;
    use zip::ZipArchive;

    // Create a zip archive from the bytes
    let cursor = Cursor::new(ZIP_DATA);
    let mut archive = ZipArchive::new(cursor)?;

    // Create a directory to extract files
    if !destination.exists() {
        error!(
            "Destination folder doesn't exists: {}",
            destination.display()
        );
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Destination folder doesn't exists",
        )));
    }

    // Iterate through the ZIP file and extract its contents
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let output_path = destination.join(file.name());

        if file.is_dir() {
            std::fs::create_dir_all(&output_path)?;
        } else {
            let mut output_file = std::fs::File::create(output_path).map_err(Box::new)?;
            std::io::copy(&mut file, &mut output_file)?;
        }
    }

    Ok(())
}

const REFIND_CONFIG: &[u8] = include_bytes!("../../refind.conf");
const SHIM_BOOTX64: &[u8] = include_bytes!("../../BOOTX64.EFI");
const SHIM_MMX64: &[u8] = include_bytes!("../../mmx64.efi");
async fn install_bootloader_refind(
    mount_point: &std::path::Path,
    arch: &Architecture,
    rootfs_partuuid: &str,
    cmdline: &str,
    name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let tmp_dir = std::env::temp_dir().join("refind_install");
    if tmp_dir.exists() {
        std::fs::remove_dir_all(&tmp_dir)?;
    }
    std::fs::create_dir_all(&tmp_dir)?;
    decompress_refind(&tmp_dir).await?;

    info!(
        "Decompressed rEFInd to temporary directory: {}",
        tmp_dir.display()
    );

    let refind_search_result = Command::new("find")
        .arg(&tmp_dir)
        .arg("-name")
        .arg("refind-bin-*")
        .output()
        .await
        .map_err(|e| e.to_string())?;

    let refind_bin_path = String::from_utf8_lossy(&refind_search_result.stdout)
        .trim()
        .to_string()
        .lines()
        .next()
        .ok_or("rEFInd binary path not found")?
        .to_string();

    info!("rEFInd binary path: {}", refind_bin_path);
    let refind_path = std::path::PathBuf::from(refind_bin_path);

    let efi_dir = mount_point.join("EFI");
    std::fs::create_dir_all(&efi_dir)?;
    std::fs::create_dir_all(efi_dir.join("BOOT"))?;
    Command::new("cp")
        .arg("-a")
        .arg(refind_path.join("refind"))
        .arg(efi_dir.clone())
        .status()
        .await
        .map_err(|e| e.to_string())?;

    // Remove the refind/refind.conf-sample file
    let refind_conf_sample = efi_dir.join("refind").join("refind.conf-sample");
    if refind_conf_sample.exists() {
        debug!(
            "Removing rEFInd sample config file: {}",
            refind_conf_sample.display()
        );
        std::fs::remove_file(&refind_conf_sample)?;
    }

    // Write our custom refind.conf
    let refind_conf_path = efi_dir.join("refind").join("refind.conf");
    debug!("Writing rEFInd config file: {}", refind_conf_path.display());
    let mut refind_conf_content = String::from_utf8_lossy(REFIND_CONFIG).to_string();
    refind_conf_content = refind_conf_content.replace("{ROOTFS_PARTUUID}", rootfs_partuuid);
    refind_conf_content = refind_conf_content.replace("{INSTALL_NAME}", name);
    refind_conf_content = refind_conf_content.replace("{KERNEL_CMDLINE}", cmdline);
    std::fs::write(&refind_conf_path, refind_conf_content)?;

    match arch {
        Architecture::GenericAMD64 => {
            info!("Installing rEFInd amd64 bootloader...");

            let bootx64_path = efi_dir.join("BOOT").join("BOOTX64.EFI");
            if bootx64_path.exists() {
                debug!("Removing existing BOOTX64.EFI: {}", bootx64_path.display());
                std::fs::remove_file(&bootx64_path)?;
            }
            std::fs::write(&bootx64_path, SHIM_BOOTX64)?;

            let mmx64_path = efi_dir.join("BOOT").join("mmx64.efi");
            if mmx64_path.exists() {
                debug!("Removing existing mmx64.efi: {}", mmx64_path.display());
                std::fs::remove_file(&mmx64_path)?;
            }
            std::fs::write(&mmx64_path, SHIM_MMX64)?;
        }
        Architecture::GenericAarch64 => {
            info!("Installing rEFInd aarch64 bootloader...");

            // TODO: do aarch64 installation
            return Err("Not yet implemented".into());
        }
        _ => {
            error!("Unsupported architecture for the specified bootloader");
            return Err("Unsupported bootloader".into());
        }
    }

    Ok(())
}
