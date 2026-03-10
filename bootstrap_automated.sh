#!/bin/bash

if [ "$EUID" -ne 0 ]; then
    echo "Please run as root"
    exit
fi

if [ ! -f "private_key.pem" ]; then
    echo "Generating test update key..."
    ./target/debug/embuer-genkeys --private-key-pem private_key.pem --public-key-pem public_key_pkcs1.pem
else
    echo "Test update key already exists, skipping generation."
fi

./target/debug/embuer-installer -i test.img --arch "amd64" --bootloader "refind" --deployment-name "archlinux" --deployment-source "manual" --manual-script "./create_archlinux.sh" --generate-deployment "./"

# The deployment subvolume is made read-only by the embuer-installer executable when we are finished

./target/debug/embuer-genupdate --private-key-pem private_key.pem --path "./"
