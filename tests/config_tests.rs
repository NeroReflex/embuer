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

use embuer::config::Config;

#[test]
fn parse_config_minimal() {
    let json = r#"{
        "update_url": "http://example.com/update.btrfs.xz",
        "auto_install_updates": false
    }"#;

    let cfg = Config::new(json).expect("should parse config");
    assert!(cfg.update_url().is_some());
    assert_eq!(
        cfg.update_url().unwrap(),
        "http://example.com/update.btrfs.xz"
    );
    assert!(!cfg.auto_install_updates());
}

#[test]
fn parse_config_no_updates() {
    let json = r#"{
        "auto_install_updates": false
    }"#;

    let cfg = Config::new(json).expect("should parse config");
    assert!(cfg.update_url().is_none());
    assert!(!cfg.auto_install_updates());
}
