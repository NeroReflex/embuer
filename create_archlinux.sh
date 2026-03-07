#!/bin/bash

# The following is an example of a manual installation script for Arch Linux.
# It can be executed by embuer-installer whith the --manual-script option.

readonly deployment_name="$1"
readonly deployment_rootfs_dir="$2"
readonly deployment_rootfs_data_dir="$3"

echo "Creating Arch Linux deployment: $deployment_name on $deployment_rootfs_dir with data dir $deployment_rootfs_data_dir"

# Install the operating system
pacstrap -K $deployment_rootfs_dir base dracut xz iptables-nft linux linux-headers wireless-regdb linux-firmware intel-ucode amd-ucode nano

# Create the manifest file for the deployment.
mkdir -p $deployment_rootfs_dir/usr/share/embuer
echo "{" > $deployment_rootfs_dir/usr/share/embuer/manifest.json
echo "    \"version\": \"$deployment_name\"," >> $deployment_rootfs_dir/usr/share/embuer/manifest.json
echo "    \"readonly\": true" >> $deployment_rootfs_dir/usr/share/embuer/manifest.json
echo "}" >> $deployment_rootfs_dir/usr/share/embuer/manifest.json

# pacstrap leaves the gpg-agent running in the background, which prevents us from unmounting the deployment rootfs:
# we need to kill it before we can unmount.
readonly pgp_user=$(ps aux | grep $deployment_rootfs_dir | grep "gpg-agent" | awk '{print $2}')
if [ -n "$pgp_user" ]; then
    echo "Found gpg-agent process with PID: $pgp_user, killing it..."
    kill -9 "$pgp_user"
else
    echo "No gpg-agent process found for deployment rootfs: $deployment_rootfs_dir"
fi

# The deployment is booted by running the executable in /boot/bzImage
# We can simply hardlink the kernel uki to /boot/bzImage

# TODO: make the image bootable
