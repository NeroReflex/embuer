# Embuer

Embuer is a fast, easy-to-use and quick to integrate update daemon especially suited for *embedded* linux systems.

This update daemon can download from a configured url, or prompted via DBus a file that will be used to install the update as a new subvolume in a preconfigured directory as a btrfs snapshot.

The downloaded file is never written to the disk and will never be fitted entirely in RAM,
making it suitable for operation in smaller CPUs: the update is streamed directly to the decompression
algorithm and then to btrfs receive.

Before applying the update and making it bootable a check is performed against the shipped key
ansuring only approved and signed software can be installed and executed.

## Security

The security model runs on a single assumption: the user cannot use root/sudo to modify the public
key used to verify update packages.

### Generating the keypair

The keypair can be generated using openssl:

```sh
openssl genrsa -out private_key.pem 2048
```

From the private key the public key must be generated in PEM format:

```sh
openssl rsa -in private_key.pem -pubout -outform PEM -RSAPublicKey_out -out public_key_pkcs1.pem
```

