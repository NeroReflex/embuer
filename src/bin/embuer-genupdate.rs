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

use std::path::PathBuf;

use argh::FromArgs;
use log::{error, info, warn};
use tokio::process::Command;

/// Embuer GenUpdate - Generate an installable deployment
#[derive(FromArgs)]
struct EmbuerGenupdateCli {
    #[argh(
        option,
        description = "path containing update.btrfs.xz and CHANGELOG files",
        short = 'p'
    )]
    pub path: PathBuf,

    #[argh(
        option,
        description = "private key to be used to sign the update, in PEM format",
        short = 'k'
    )]
    pub private_key_pem: PathBuf,

    #[argh(
        option,
        description = "public key to be used to check the update, in PEM format",
        short = 'e'
    )]
    pub public_key_pem: Option<PathBuf>,

    #[argh(
        switch,
        description = "remove intermediate files after generating the update package"
    )]
    pub clean: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Debug)
        .init();

    let cli: EmbuerGenupdateCli = argh::from_env();

    if !cli.path.exists() {
        error!("Path {} does not exist", cli.path.display());
        return Err(format!("Path {} does not exist", cli.path.display()).into());
    } else if !cli.path.is_dir() {
        error!("Path {} is not a directory", cli.path.display());
        return Err(format!("Path {} is not a directory", cli.path.display()).into());
    }

    let update_btrfs_xz = cli.path.join("update.btrfs.xz");
    if !update_btrfs_xz.exists() {
        error!("File {} does not exist", update_btrfs_xz.display());
        return Err(format!("File {} does not exist", update_btrfs_xz.display()).into());
    } else if !update_btrfs_xz.is_file() {
        error!("File {} is not a file", update_btrfs_xz.display());
        return Err(format!("File {} is not a file", update_btrfs_xz.display()).into());
    }

    let private_key_pem = cli.private_key_pem.clone();
    if !private_key_pem.exists() {
        error!("Private key file {} does not exist", private_key_pem.display());
        return Err(format!("Private key file {} does not exist", private_key_pem.display()).into());
    } else if !private_key_pem.is_file() {
        error!("Private key file {} is not a file", private_key_pem.display());
        return Err(format!("Private key file {} is not a file", private_key_pem.display()).into());
    }

    let update_signature_path = cli.path.join("update.signature");

    Command::new("openssl")
        .arg("dgst")
        .arg("-sha512")
        .arg("-sign")
        .arg(private_key_pem.to_str().unwrap())
        .arg("-out")
        .arg(update_signature_path.to_str().unwrap())
        .arg(update_btrfs_xz.to_str().unwrap())
        .output()
        .await
        .inspect_err(|e| error!("Error signing the update: {e}"))
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

    info!("Generated update signature at {}", update_signature_path.display());

    match cli.public_key_pem.clone() {
        Some(pubkey_path) => {
            if !pubkey_path.exists() {
                error!("Public key file {} does not exist", pubkey_path.display());
                return Err(format!("Public key file {} does not exist", pubkey_path.display()).into());
            }

            let verification_result = Command::new("openssl")
                .arg("dgst")
                .arg("-sha512")
                .arg("-verify")
                .arg(pubkey_path.to_str().unwrap())
                .arg("-signature")
                .arg(update_signature_path.to_str().unwrap())
                .arg(update_btrfs_xz.to_str().unwrap())
                .output()
                .await
                .inspect_err(|e| error!("Error verifying the update signature: {e}"))
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

            if verification_result.status.success() {
                info!("Update signature verification successful: {}", String::from_utf8_lossy(&verification_result.stdout).trim());
            } else {
                error!("Update signature verification failed: {}", String::from_utf8_lossy(&verification_result.stderr).trim());
                return Err("Update signature verification failed".into());
            }
        }
        None => {
            warn!("Public key file not provided, skipping signature verification");
        }
    }

    // tar cf "${BINARIES_DIR}/update_package.tar" -C "${BINARIES_DIR}" "CHANGELOG" "update.signature" "update.btrfs.xz"
    Command::new("tar")
        .arg("cf")
        .arg(cli.path.join("update_package.tar").to_str().unwrap())
        .arg("-C")
        .arg(cli.path.to_str().unwrap())
        .arg("CHANGELOG")
        .arg("update.signature")
        .arg("update.btrfs.xz")
        .output()
        .await
        .inspect_err(|e| error!("Error creating the update package: {e}"))
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

    info!("Generated update package at {}", cli.path.join("update_package.tar").display());

    if cli.clean {
        std::fs::remove_file(update_signature_path.clone())
            .inspect_err(|e| error!("Error removing update dsignature file {}: {e}", update_signature_path.display()))
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

        info!("Removed update signature file {}", update_signature_path.display());

        std::fs::remove_file(update_btrfs_xz.clone())
            .inspect_err(|e| error!("Error removing deployment file {}: {e}", update_btrfs_xz.display()))
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

        info!("Removed deployment file {}", update_btrfs_xz.display());
    }

    Ok(())
}