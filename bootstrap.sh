#!/bin/bash

./target/debug/embuer-installer -i test.img --bootloader "refind_amd64" --deployment-name "archlinux" --deployment-source "manual" --manual-kernel "../platform-drivers-x86/"

# You can create a deployment file via:
# pacstrap -K /tmp/embuer_mnt/rootfs/deployments/archlinux base
# mkdir -p /tmp/embuer_mnt/rootfs/deployments/archlinux/usr/share/embuer
# nano /tmp/embuer_mnt/rootfs/deployments/archlinux/usr/share/embuer/manifest.json
##{
##    "version": "",
##    "readonly": true
##}
# ps aux | grep /tmp/embuer_mnt/rootfs/deployments/archlinux
# kill -9 <PID>
# btrfs property set -fts /tmp/embuer_mnt/rootfs/deployments/archlinux ro true
# btrfs send -f archlinux.btrfs /tmp/embuer_mnt/rootfs/deployments/archlinux
# xz -9e archlinux.btrfs
