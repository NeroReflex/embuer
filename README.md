# Embuer

Embuer is a fast, easy-to-use and quick to integrate update daemon especially suited for *embedded Linux systems*.

**Platform**: Linux-only (requires btrfs filesystem)

This update daemon can download from a configured url, or prompted via DBus a file that will be used to install the update as a new subvolume in a preconfigured directory as a btrfs snapshot.

The downloaded file is never written to the disk and will never be fitted entirely in RAM,
making it suitable for operation in smaller CPUs: the update is streamed directly to the decompression
algorithm and then to btrfs receive.

Before applying the update and making it bootable a check is performed against the shipped key
ansuring only approved and signed software can be installed and executed.

## Requirements

Embuer relies on a few key components in order to run on the target:
  - xz: decompress the btrfs snapshot as a stream
  - btrfs: uses *btrfs receive* as deploments are just the (compressed) result of *btrfs send*
  - bash: used to execute certain post-installation scripts

Any system configured this way should be able to run embuer and handle every update size
(assuming the rootfs is large enough) and the system is correctly configured.

## Project Structure

Embuer is organized as follows:

- **embuer-service**: A D-Bus service binary that runs as a daemon and manages system updates
- **embuer-client**: A CLI client to interact with the service (query status, trigger updates)
- **libembuer**: A library with three variants:
  - Rust library (`rlib`) for Rust applications
  - C shared library (`cdylib`) for dynamic linking
  - C static library (`staticlib`) for static linking

## Building

Build all components:

```sh
cargo build --release
```

This will produce:
- `target/release/embuer-service` - The D-Bus service daemon
- `target/release/embuer-client` - The CLI client
- `target/release/libembuer.so` - Shared library
- `target/release/libembuer.a` - Static library

## Usage

### Running the Service

The service must be run as root:

```sh
sudo ./target/release/embuer-service
```

### Using the CLI Client

Query the current status:

```sh
embuer-client status
```

Watch for status updates in real-time:

```sh
embuer-client watch
```

Install an update from a file:

```sh
embuer-client install-file /path/to/update.tar.gz
```

Install an update from a URL:

```sh
embuer-client install-url https://example.com/update.tar.gz
```

**When AwaitingConfirmation appears:**
```bash
# View changelog and details
embuer-client pending-update

# Accept the update
embuer-client accept

# OR reject the update
embuer-client reject
```

### Manual Testing Scenario

1. Set `auto_install_updates: false` in config
2. Start the service: `sudo embuer-service`
3. In another terminal, run: `embuer-client watch`
4. Trigger an update (via periodic checker or `embuer-client install-url`)
5. Observe status change to "AwaitingConfirmation"
6. Run `embuer-client pending-update` to view details
7. Run `embuer-client accept` to approve
8. Observe installation progress in watch terminal

### Using the C Library

Include the header file and link against the library:

```c
#include "embuer.h"

int main() {
    embuer_client_t* client = embuer_client_new();
    if (!client) return 1;
    
    char* status = NULL;
    char* details = NULL;
    int progress = 0;
    
    if (embuer_get_status(client, &status, &details, &progress) == EMBUER_OK) {
        printf("Status: %s\n", status);
        embuer_free_string(status);
        embuer_free_string(details);
    }
    
    embuer_client_free(client);
    return 0;
}
```

Compile with:

```sh
gcc -o myapp myapp.c -L./target/release -lembuer -lpthread -ldl -lm
```

See `examples/embuer_example.c` for a complete example, or `examples/status_monitor.c` for a focused status monitoring example.

### Using the Rust Library

Add to your `Cargo.toml`:

```toml
[dependencies]
embuer = { path = "/path/to/embuer" }
```

Use in your code:

```rust
use embuer::dbus::EmbuerDBusProxy;
use zbus::Connection;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let connection = Connection::system().await?;
    let proxy = EmbuerDBusProxy::new(&connection).await?;
    
    let (status, details, progress) = proxy.get_update_status().await?;
    println!("Status: {}", status);
    
    Ok(())
}
```

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

### Generating the signature

In order for the install to be applied a valid signature for the included public key must be provided in the update package.

```sh
openssl dgst -sha512 -sign private_key.pem -out update.signature update.btrfs.xz
```

Note: Make sure the private key (private_key.pem) is in PKCS#1 format.
If you have a PKCS#8 format key, you can convert it:

```sh
# If you have a PKCS#8 private key, convert it to PKCS#1:
openssl rsa -in private_key_pkcs8.pem -out private_key.pem
```

To verify it works before packaging, you can test with:

```sh
openssl dgst -sha512 -verify public_key.pem -signature update.signature update.btrfs.xz
```

This should output "Verified OK" if the signature is valid.

### Generating the package

Once you have update.btrfs.xz, update.signature and CHANGELOG files you can generate the
update package:

```sh
tar -cf update.tar CHANGELOG update.btrfs.xz update.signature
```
