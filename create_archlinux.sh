#!/bin/bash

# The following is an example of a manual installation script for Arch Linux.
# It can be executed by embuer-installer whith the --manual-script option.

readonly deployment_name="$1"
readonly deployment_rootfs_dir="$2"
readonly deployment_rootfs_data_dir="$3"

echo "Creating Arch Linux deployment: $deployment_name on $deployment_rootfs_dir with data dir $deployment_rootfs_data_dir"

# Install the operating system
pacstrap -K $deployment_rootfs_dir base xz systemd-ukify iptables-nft wireless-regdb linux-firmware intel-ucode amd-ucode nano util-linux btrfs-progs

mount --bind ${deployment_rootfs_dir} ${deployment_rootfs_dir}
arch-chroot ${deployment_rootfs_dir} /bin/bash <<EOF
set -e
set -x

pacman-key --populate

echo "LANG=en_US.UTF-8" > /etc/locale.conf
locale-gen

# Cannot check space in chroot
sed -i '/CheckSpace/s/^/#/g' /etc/pacman.conf

# Disable check and debug for makepkg on the final image
sed -i '/BUILDENV/s/ check/ !check/g' /etc/makepkg.conf
sed -i '/OPTIONS/s/ debug/ !debug/g' /etc/makepkg.conf

echo '# Written by systemd-localed(8) or systemd-firstboot(1), read by systemd-localed' > /etc/vconsole.conf
echo '# and systemd-vconsole-setup(8). Use localectl(1) to update this file.' >> /etc/vconsole.conf
echo 'KEYMAP=it2' >> /etc/vconsole.conf
echo 'XKBLAYOUT=it' >> /etc/vconsole.conf

pacman -S --noconfirm mkinitcpio

mkdir -p /boot/EFI/Linux

mkdir -p /etc/mkinitcpio.d/
echo '# mkinitcpio preset file for the 'linux' package' >> /etc/mkinitcpio.d/linux.preset
echo '' >> /etc/mkinitcpio.d/linux.preset
echo '#ALL_config="/etc/mkinitcpio.conf"' >> /etc/mkinitcpio.d/linux.preset
echo 'ALL_kver="/boot/vmlinuz-linux"' >> /etc/mkinitcpio.d/linux.preset
echo '' >> /etc/mkinitcpio.d/linux.preset
echo 'PRESETS=('default')' >> /etc/mkinitcpio.d/linux.preset
echo '' >> /etc/mkinitcpio.d/linux.preset
echo '#default_config="/etc/mkinitcpio.conf"' >> /etc/mkinitcpio.d/linux.preset
echo '#default_image="/boot/initramfs-linux.img"' >> /etc/mkinitcpio.d/linux.preset
echo 'default_uki="/boot/EFI/Linux/linux.efi"' >> /etc/mkinitcpio.d/linux.preset
echo 'default_options="--splash=/usr/share/systemd/bootctl/splash-arch.bmp"' >> /etc/mkinitcpio.d/linux.preset

mkdir -p /etc/cmdline.d/
echo '# enable apparmor' > /etc/cmdline.d/security.conf
echo 'lsm=landlock,lockdown,yama,integrity,apparmor,bpf audit=1 audit_backlog_limit=256' >> /etc/cmdline.d/security.conf

cat /etc/mkinitcpio.conf

pacman -S --noconfirm linux linux-headers

EOF

# Create the manifest file for the deployment.
mkdir -p $deployment_rootfs_dir/usr/share/embuer
echo "{" > $deployment_rootfs_dir/usr/share/embuer/manifest.json
echo "    \"version\": \"$deployment_name\"," >> $deployment_rootfs_dir/usr/share/embuer/manifest.json
echo "    \"readonly\": true" >> $deployment_rootfs_dir/usr/share/embuer/manifest.json
echo "}" >> $deployment_rootfs_dir/usr/share/embuer/manifest.json

# pacstrap leaves the gpg-agent running in the background, which prevents us from unmounting the deployment rootfs:
# we need to kill it before we can unmount.
readonly pgp_user=$(ps aux | grep $deployment_rootfs_dir | grep "gpg-agent" | head -n 1 | awk '{print $2}')
if [ -n "$pgp_user" ]; then
    echo "Found gpg-agent process with PID: $pgp_user, killing it..."
    kill -9 "$pgp_user"
else
    echo "No gpg-agent process found for deployment rootfs: $deployment_rootfs_dir"
fi

# The deployment is booted by running the executable in /boot/bzImage
# We can simply hardlink the kernel uki to /boot/bzImage
if [ -f "$deployment_rootfs_dir/boot/EFI/Linux/linux.efi" ]; then
    ln "$deployment_rootfs_dir/boot/EFI/Linux/linux.efi" "$deployment_rootfs_dir/boot/bzImage"
    echo "Boot file linked successfully: $deployment_rootfs_dir/boot/bzImage -> $deployment_rootfs_dir/boot/EFI/Linux/linux.efi"
else
    echo "Error: kernel uki not found at $deployment_rootfs_dir/boot/EFI/Linux/linux.efi"
    exit 1
fi
