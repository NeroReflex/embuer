#!/bin/bash

./target/debug/embuer-installer -i test.img --bootloader "refind_amd64" --deployment-name "test" --deployment-source "archlinux.btrfs.xz" --wait true

# Verify installation:
# losetup -P -f --show test.img
# mount /dev/loop0p1 /tmp/embuer_mnt/esp
# mount /dev/loop0p2 /tmp/embuer_mnt/rootfs
#
## Umount eveything
#
# umount /tmp/embuer_mnt/esp 
# umount /tmp/embuer_mnt/rootfs
# losetup -d /dev/loop0
