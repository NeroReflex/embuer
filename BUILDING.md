# Building Embuer

This document provides detailed instructions for building all components of Embuer.

**Note**: Embuer is designed exclusively for **Linux systems** with btrfs filesystem support. It requires a Linux environment to build and run.

## Prerequisites

### Rust Build Environment

- Rust toolchain (1.70 or later)
- Cargo build system

Install via rustup:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### System Dependencies (Linux)

The following Linux system libraries are required:

- D-Bus development libraries (`libdbus-1-dev` on Debian/Ubuntu)
- OpenSSL development libraries (`libssl-dev` on Debian/Ubuntu)
- btrfs-progs (btrfs tools)
- pkg-config

On Debian/Ubuntu:

```sh
sudo apt-get install libdbus-1-dev libssl-dev pkg-config
```

On Fedora/RHEL:

```sh
sudo dnf install dbus-devel openssl-devel pkgconfig
```

### For C Development (Optional)

To build C programs using libembuer:

- GCC or Clang compiler
- Standard C library development files

## Building

### Quick Build

Build all components (service, client, and libraries):

```sh
cargo build --release
```

This produces:

- `target/release/embuer-service` - D-Bus service daemon
- `target/release/embuer-client` - CLI client
- `target/release/libembuer.so` - Shared library (Linux)
- `target/release/libembuer.a` - Static library

### Individual Components

Build only the service:

```sh
cargo build --release --bin embuer-service
```

Build only the client:

```sh
cargo build --release --bin embuer-client
```

Build only the library:

```sh
cargo build --release --lib
```

### Debug Build

For development with debug symbols:

```sh
cargo build
```

### Using Make

A Makefile is provided for convenience:

```sh
# Build in release mode
make build-release

# Build and run tests
make test

# Build the C example
make example

# Run the C example
make run-example

# Clean build artifacts
make clean
```

## Library Types

Embuer provides three library variants:

### 1. Rust Library (rlib)

This is the standard Rust library format used when linking Rust code.

Usage in `Cargo.toml`:

```toml
[dependencies]
embuer = { path = "/path/to/embuer" }
```

### 2. C Dynamic Library (cdylib)

Shared library for dynamic linking from C/C++ applications.

- **File**: `libembuer.so`
- **Link with**: `-lembuer`

### 3. C Static Library (staticlib)

Static library for embedding in C/C++ applications.

- File: `libembuer.a`

Link with: `-lembuer -lpthread -ldl -lm`

## Cross-Compilation for Linux Embedded Systems

Embuer can be cross-compiled for different Linux architectures, commonly used in embedded systems.

### For ARM (32-bit) Embedded Linux

Install the cross-compilation target:

```sh
rustup target add armv7-unknown-linux-gnueabihf
```

Build:

```sh
cargo build --release --target armv7-unknown-linux-gnueabihf
```

### For ARM64 (64-bit) Embedded Linux

```sh
rustup target add aarch64-unknown-linux-gnu
cargo build --release --target aarch64-unknown-linux-gnu
```

**Note**: All these targets are Linux-based. Embuer requires Linux and btrfs support.

## Installation

### System-Wide Installation

Install to `/usr/local`:

```sh
sudo make install
```

This installs:
- Binaries to `/usr/local/bin/`
- Libraries to `/usr/local/lib/`
- Header file to `/usr/local/include/`

### Manual Installation

```sh
# Install binaries
sudo install -m 755 target/release/embuer-service /usr/local/bin/
sudo install -m 755 target/release/embuer-client /usr/local/bin/

# Install libraries
sudo install -m 644 target/release/libembuer.so /usr/local/lib/
sudo install -m 644 target/release/libembuer.a /usr/local/lib/

# Install header
sudo install -m 644 embuer.h /usr/local/include/

# Update library cache
sudo ldconfig
```

## Building C Applications

### Using the Shared Library

```sh
gcc -o myapp myapp.c -lembuer
```

### Using the Static Library

```sh
gcc -o myapp myapp.c -L./target/release -lembuer -lpthread -ldl -lm
```

### Build the Example

```sh
cd examples
gcc -o embuer_example embuer_example.c \
    -I.. \
    -L../target/release \
    -lembuer \
    -lpthread -ldl -lm
```

Run with:

```sh
LD_LIBRARY_PATH=../target/release ./embuer_example
```

## Testing

Run the test suite:

```sh
cargo test
```

Run with verbose output:

```sh
cargo test -- --nocapture
```

## Troubleshooting

### "cannot find -lembuer"

Ensure the library is in the linker path:

```sh
export LD_LIBRARY_PATH=/path/to/embuer/target/release:$LD_LIBRARY_PATH
```

Or install the library system-wide with `sudo make install`.

### D-Bus Connection Errors

Ensure D-Bus is running:

```sh
systemctl status dbus
```

### Permission Errors

The service requires root privileges:

```sh
sudo ./target/release/embuer-service
```

### Build Failures

1. Update Rust: `rustup update`
2. Clean build: `cargo clean && cargo build`
3. Check dependencies are installed
4. Verify pkg-config can find libraries:

```sh
pkg-config --libs dbus-1
pkg-config --libs openssl
```

## Development Tips

### Faster Incremental Builds

Use `cargo check` for quick syntax checking:

```sh
cargo check
```

### Watch Mode

Install cargo-watch:

```sh
cargo install cargo-watch
```

Auto-rebuild on changes:

```sh
cargo watch -x build
```

### Code Formatting

```sh
cargo fmt
```

### Linting

```sh
cargo clippy
```

