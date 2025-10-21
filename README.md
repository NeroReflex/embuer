# Embuer

Embuer is a fast, easy-to-use and quick to integrate update daemon especially suited for *embedded Linux systems*.

**Platform**: Linux-only (requires btrfs filesystem)

This update daemon can download from a configured url, or prompted via DBus a file that will be used to install the update as a new subvolume in a preconfigured directory as a btrfs snapshot.

The downloaded file is never written to the disk and will never be fitted entirely in RAM,
making it suitable for operation in smaller CPUs: the update is streamed directly to the decompression
algorithm and then to btrfs receive.

Before applying the update and making it bootable a check is performed against the shipped key
ansuring only approved and signed software can be installed and executed.

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

