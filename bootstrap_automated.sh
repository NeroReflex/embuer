#!/bin/bash

./target/debug/embuer-installer -i test.img --arch "amd64" --bootloader "refind" --deployment-name "archlinux" --deployment-source "manual" --manual-script "./create_archlinux.sh" --generate-snapshot "archlinux.btrfs"

# The deployment subvolume is made read-only by the embuer-installer executable when we are finished

# btrfs send -f archlinux.btrfs /tmp/embuer_mnt/rootfs/deployments/archlinux
# xz -9e archlinux.btrfs
