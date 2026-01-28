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

use std::collections::VecDeque;

use argh::FromArgs;
use log::{debug, error, info, warn};
use tokio::process::Command;

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

    #[argh(option, description = "source of the deployment", short = 's')]
    pub deployment_source: String,

    #[argh(option, description = "name of the deployment", short = 'k')]
    pub deployment_name: String,

    #[argh(
        option,
        description = "bootloader to install (refind_amd64, refind_aarch64)",
        short = 'b'
    )]
    pub bootloader: Option<String>,

    #[argh(option, description = "kernel cmdline", short = 'c')]
    pub cmdline: Option<String>,

    #[argh(option, description = "name of the installation", short = 'n')]
    pub name: Option<String>,
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
                    return Err("Image size must be greater than 0".into());
                }
                Command::new("fallocate")
                    .arg("-l")
                    .arg(format!("{size}G"))
                    .arg(img)
                    .output()
                    .await
                    .map_err(|e| e.to_string())?;
            }

            // Setup loop device, example command:
            // losetup -P -f --show test.img
            let output = Command::new("losetup")
                .arg("-P")
                .arg("-f")
                .arg("--show")
                .arg(img)
                .output()
                .await
                .map_err(|e| e.to_string())?;

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
        .await
        .map_err(|e| e.to_string())?;

    let esp_part_size = "64MiB";

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
        .arg(esp_part_size)
        .arg("type")
        .arg("1")
        .arg("c12a7328-f81f-11d2-ba4b-00a0c93ec93b")
        .arg("set")
        .arg("1")
        .arg("esp")
        .arg("on")
        .status()
        .await
        .map_err(|e| e.to_string())?;
    let partition1 = {
        let mut result = format!("{}", device_partition.display());
        if result.ends_with(char::is_numeric) {
            result = format!("{}p1", result);
        } else {
            result = format!("{}1", result);
        }

        result
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
        .arg(esp_part_size)
        .arg("100%")
        .arg("type")
        .arg("2")
        .arg("4F68BCE3-E8CD-4DB1-96E7-FBCAF984B709")
        .status()
        .await
        .map_err(|e| e.to_string())?;
    let partition2 = {
        let mut result = format!("{}", device_partition.display());
        if result.ends_with(char::is_numeric) {
            result = format!("{}p2", result);
        } else {
            result = format!("{}2", result);
        }

        result
    };

    info!("Formatting EFI partition {} with FAT32...", partition1);
    Command::new("mkfs.fat")
        .arg("-F32")
        .arg(&partition1)
        .status()
        .await
        .map_err(|e| e.to_string())?;

    info!("Formatting rootfs partition {} with btrfs...", partition2);
    Command::new("mkfs.btrfs")
        .arg("-f")
        .arg(&partition2)
        .arg("-L")
        .arg("rootfs")
        .status()
        .await
        .map_err(|e| e.to_string())?;

    let base_mount_path = std::env::temp_dir().join("embuer_mnt");
    std::fs::create_dir_all(&base_mount_path)?;

    // Mount ESP partition
    let esp_mount_dir = {
        let esp_mount_point =
            std::path::PathBuf::from(format!("{}/esp", base_mount_path.display()));

        std::fs::create_dir_all(&esp_mount_point)?;

        info!(
            "Mounting ESP partition {} to {}...",
            partition1,
            esp_mount_point.display()
        );

        Command::new("mount")
            .arg(&partition1)
            .arg(&esp_mount_point)
            .status()
            .await
            .map_err(|e| e.to_string())?;

        mounts.push_front(MountType::Device(std::path::PathBuf::from(&partition1)));

        esp_mount_point
    };

    let rootfs_mount_dir = {
        let rootfs_mount_point =
            std::path::PathBuf::from(format!("{}/rootfs", base_mount_path.display()));

        std::fs::create_dir_all(&rootfs_mount_point)?;

        info!(
            "Mounting rootfs partition {} to {}...",
            partition2,
            rootfs_mount_point.display()
        );

        Command::new("mount")
            .arg("-t")
            .arg("btrfs")
            .arg("-o")
            .arg("subvolid=5,compress-force=zstd:15,noatime,rw")
            .arg(&partition2)
            .arg(&rootfs_mount_point)
            .status()
            .await
            .map_err(|e| e.to_string())?;

        mounts.push_front(MountType::Device(std::path::PathBuf::from(&partition2)));

        rootfs_mount_point
    };

    let partuuid_find = Command::new("blkid")
        .arg("-s")
        .arg("PARTUUID")
        .arg("-o")
        .arg("value")
        .arg(&partition2)
        .output()
        .await
        .map_err(|e| e.to_string())?;

    let rootfs_partuuid = String::from_utf8_lossy(&partuuid_find.stdout)
        .trim()
        .to_string();

    info!("Rootfs PARTUUID: {rootfs_partuuid}");

    // Install bootloader if specified
    if let Some(bootloader) = cli.bootloader.as_ref() {
        info!("Installing bootloader: {bootloader}...");
        install_bootloader(
            &esp_mount_dir,
            bootloader,
            rootfs_partuuid.as_str(),
            cli.cmdline.as_deref().unwrap_or(""),
            name.as_str(),
        )
        .await?;
    } else {
        warn!("No bootloader specified: skipping bootloader installation");
    }

    // Prepare the rootfs structure
    let btrfs = embuer::btrfs::Btrfs::new().map_err(|e| Box::new(e))?;
    info!(
        "Prepare the rootfs from source image {}...",
        cli.deployment_source
    );
    let rootfs_dir =
        prepare_rootfs_partition(&btrfs, &rootfs_mount_dir, &cli.deployment_name).await?;

    // Copy over the deployment rootfs contents
    info!("Populating the rootfs: {}", rootfs_dir.display());

    Ok(())
}

async fn prepare_rootfs_partition(
    btrfs: &embuer::btrfs::Btrfs,
    rootfs_mount_dir: &std::path::Path,
    deployment_name: &str,
) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let deployments_dir = rootfs_mount_dir.join("deployments");
    let result = btrfs.subvolume_create(&deployments_dir)?;
    debug!("{}", result.trim());

    let deployments_data_dir = rootfs_mount_dir.join("deployments_data");
    let result = btrfs.subvolume_create(&deployments_data_dir)?;
    debug!("{}", result.trim());

    let deployments_data_rootfs_dir = deployments_data_dir.join(&deployment_name);
    let result = btrfs.subvolume_create(&deployments_data_rootfs_dir)?;
    debug!("{}", result.trim());

    let deployment_rootfs_dir = deployments_dir.join(&deployment_name);
    let result = btrfs.subvolume_create(&deployment_rootfs_dir)?;
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

    Ok(deployment_rootfs_dir)
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
            let mut output_file = std::fs::File::create(output_path).map_err(|e| Box::new(e))?;
            std::io::copy(&mut file, &mut output_file)?;
        }
    }

    Ok(())
}

const REFIND_CONFIG: &[u8] = include_bytes!("../../refind.conf");
const SHIM_BOOTX64: &[u8] = include_bytes!("../../BOOTX64.EFI");
const SHIM_MMX64: &[u8] = include_bytes!("../../mmx64.efi");
async fn install_bootloader(
    mount_point: &std::path::Path,
    bootloader: &str,
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

    match bootloader {
        "refind_amd64" => {
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
        "refind_aarch64" => {
            info!("Installing rEFInd aarch64 bootloader...");

            // TODO: do aarch64 installation
            return Err("Not yet implemented".into());
        }
        _ => {
            error!("Unsupported bootloader: {}", bootloader);
            return Err("Unsupported bootloader".into());
        }
    }

    Ok(())
}
