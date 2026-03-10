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
use tokio::process::Command;
use log::{error, info};

/// Embuer GenKeys - Generate deployments signing keys
#[derive(FromArgs)]
struct EmbuerGenkeysCli {
    #[argh(
        option,
        description = "private key to be generated, in PEM format",
        short = 'k'
    )]
    pub private_key_pem: PathBuf,

    #[argh(
        option,
        description = "public key to be generated, in PEM format",
        short = 'p'
    )]
    pub public_key_pem: PathBuf,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Debug)
        .init();

    let cli: EmbuerGenkeysCli = argh::from_env();

    if cli.private_key_pem.exists() {
        error!("File {} already exists, refusing to overwrite", cli.private_key_pem.display());
        return Err(format!("File {} already exists, refusing to overwrite", cli.private_key_pem.display()).into());
    } else if cli.public_key_pem.exists() {
        error!("File {} already exists, refusing to overwrite", cli.public_key_pem.display());
        return Err(format!("File {} already exists, refusing to overwrite", cli.public_key_pem.display()).into());
    }

    // Generate an RSA 2048 private key in PEM format using openssl
    let priv_key_gen_res = Command::new("openssl")
        .arg("genrsa")
        .arg("-out")
        .arg(cli.private_key_pem.to_str().unwrap())
        .arg("2048")
        .output()
        .await
        .inspect_err(|e| error!("Error generating the private key: {e}"))
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

    info!("Generated private key {}: {}", cli.private_key_pem.display(), String::from_utf8_lossy(&priv_key_gen_res.stdout).trim());

    let pub_key_gen_res = Command::new("openssl")
        .arg("rsa")
        .arg("-in")
        .arg(cli.private_key_pem.to_str().unwrap())
        .arg("-pubout")
        .arg("-outform")
        .arg("PEM")
        .arg("-RSAPublicKey_out")
        .arg("-out")
        .arg(cli.public_key_pem.to_str().unwrap())
        .output()
        .await
        .inspect_err(|e| error!("Error generating the public key: {e}"))
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
    info!("Generated public key {}: {}", cli.public_key_pem.display(), String::from_utf8_lossy(&pub_key_gen_res.stdout).trim());

    Ok(())
}