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

use argh::FromArgs;
use std::process;

/// Embuer Client - Control and monitor the Embuer update service
#[derive(FromArgs)]
struct EmbuerInstallCli {
    
}

#[tokio::main]
async fn main() {
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .init();

    let cli: EmbuerInstallCli = argh::from_env();

    //let result = match cli.command {
    //    SubCommand::Status(_) => get_status().await,
    //    SubCommand::BootInfo(_) => get_boot_info().await,
    //    SubCommand::Watch(_) => watch_status().await,
    //    SubCommand::InstallFile(cmd) => install_from_file(&cmd.path).await,
    //    SubCommand::InstallUrl(cmd) => install_from_url(&cmd.url).await,
    //    SubCommand::PendingUpdate(_) => get_pending_update().await,
    //    SubCommand::Accept(_) => confirm_update(true).await,
    //    SubCommand::Reject(_) => confirm_update(false).await,
    //};

    //if let Err(e) = result {
    //    eprintln!("{} {}", "✗".red().bold(), e.to_string().red());
    //    process::exit(1);
    //}
}
