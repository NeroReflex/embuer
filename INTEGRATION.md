# Integration Guide

This guide explains how to integrate Embuer into your applications using different programming languages.

**Platform**: Embuer is designed exclusively for **Linux systems** with btrfs filesystem support.

## Table of Contents

- [D-Bus Interface](#dbus-interface)
- [CLI Client](#cli-client)
- [Rust Library](#rust-library)
- [C Library](#c-library)
- [C++ Integration](#c-integration)
- [Python Integration](#python-integration)

## D-Bus Interface

Embuer exposes its functionality via D-Bus, making it accessible from any language with D-Bus bindings.

### Service Details

- **Bus**: System bus
- **Service Name**: `org.neroreflex.embuer`
- **Object Path**: `/org/neroreflex/embuer`
- **Interface**: `org.neroreflex.embuer1`

### Methods

#### GetUpdateStatus

Get the current update status.

```
GetUpdateStatus() → (status: s, details: s, progress: i)
```

Returns:
- `status`: Current status string (e.g., "Idle", "Downloading", "Installing")
- `details`: Additional details about the status
- `progress`: Progress percentage (0-100, or -1 if N/A)

#### InstallUpdateFromFile

Install an update from a local file.

```
InstallUpdateFromFile(file_path: s) → result: s
```

Parameters:
- `file_path`: Path to the update file

Returns:
- `result`: Confirmation message

#### InstallUpdateFromUrl

Install an update from a URL.

```
InstallUpdateFromUrl(url: s) → result: s
```

Parameters:
- `url`: URL to download the update from

Returns:
- `result`: Confirmation message

### Signals

#### UpdateStatusChanged

Emitted when the update status changes.

```
UpdateStatusChanged(status: s, details: s, progress: i)
```

Parameters:
- `status`: New status string
- `details`: Status details
- `progress`: Progress percentage (0-100, or -1 if N/A)

## CLI Client

The simplest way to interact with Embuer is via the CLI client.

### Commands

```sh
# Get current status
embuer-client status

# Watch for updates in real-time
embuer-client watch

# Install from file
embuer-client install-file /path/to/update.tar.gz

# Install from URL
embuer-client install-url https://example.com/update.tar.gz
```

### Scripting

Use the CLI client in scripts:

```bash
#!/bin/bash

# Check if an update is in progress
status=$(embuer-client status | grep "Status:" | cut -d: -f2)

if [ "$status" != "Idle" ]; then
    echo "Update in progress: $status"
    exit 1
fi

# Trigger update
embuer-client install-url https://example.com/update.tar.gz
```

## Rust Library

Use Embuer directly from Rust applications.

### Adding the Dependency

Add to `Cargo.toml`:

```toml
[dependencies]
embuer = { path = "/path/to/embuer" }
tokio = { version = "1", features = ["full"] }
zbus = "5"
```

### Example: Query Status

```rust
use embuer::dbus::EmbuerDBusProxy;
use zbus::Connection;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Connect to the system bus
    let connection = Connection::system().await?;

    // Create proxy to the Embuer service
    let proxy = EmbuerDBusProxy::new(&connection).await?;
    
    // Get current status
    let (status, details, progress) = proxy.get_update_status().await?;
    println!("Status: {}", status);
    println!("Details: {}", details);
    if progress >= 0 {
        println!("Progress: {}%", progress);
    }

    Ok(())
}
```

### Example: Install Update

```rust
use embuer::dbus::EmbuerDBusProxy;
use zbus::Connection;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let connection = Connection::system().await?;
    let proxy = EmbuerDBusProxy::new(&connection).await?;
    
    // Install from URL
    let result = proxy.install_update_from_url(
        "https://example.com/update.tar.gz".to_string()
    ).await?;

    println!("{}", result);

    Ok(())
}
```

### Example: Watch for Status Changes

```rust
use embuer::dbus::EmbuerDBusProxy;
use zbus::Connection;
use futures_util::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let connection = Connection::system().await?;
    let proxy = EmbuerDBusProxy::new(&connection).await?;

    // Subscribe to status change signals
    let mut stream = proxy.receive_update_status_changed().await?;

    println!("Watching for status changes...");

    while let Some(signal) = stream.next().await {
        let args = signal.args()?;
        println!(
            "Status: {} - {} ({}%)",
            args.status, args.details, args.progress
        );
    }

    Ok(())
}
```

## C Library

The C library provides a convenient FFI interface for C and C++ applications.

### Header File

Include the header:

```c
#include <embuer.h>
```

### API Overview

```c
// Client lifecycle
embuer_client_t* embuer_client_new(void);
void embuer_client_free(embuer_client_t* client);

// Operations
int embuer_get_status(embuer_client_t*, char**, char**, int*);
int embuer_install_from_file(embuer_client_t*, const char*, char**);
int embuer_install_from_url(embuer_client_t*, const char*, char**);
int embuer_watch_status(embuer_client_t*, StatusCallback, void*);

// Memory management
void embuer_free_string(char* s);
```

### Example: Query Status

```c
#include <stdio.h>
#include <embuer.h>

int main() {
    // Create client
    embuer_client_t* client = embuer_client_new();
    if (!client) {
        fprintf(stderr, "Failed to create client\n");
        return 1;
    }

    // Get status
    char* status = NULL;
    char* details = NULL;
    int progress = 0;

    int result = embuer_get_status(client, &status, &details, &progress);

    if (result == EMBUER_OK) {
        printf("Status: %s\n", status);
        printf("Details: %s\n", details);
        if (progress >= 0) {
            printf("Progress: %d%%\n", progress);
        }

        // Free strings
        embuer_free_string(status);
        embuer_free_string(details);
    } else {
        fprintf(stderr, "Error: %d\n", result);
    }

    // Clean up
    embuer_client_free(client);

    return 0;
}
```

### Example: Install Update

```c
#include <stdio.h>
#include <embuer.h>

int main() {
    embuer_client_t* client = embuer_client_new();
    if (!client) return 1;

    char* result = NULL;
    int code = embuer_install_from_url(
        client,
        "https://example.com/update.tar.gz",
        &result
    );

    if (code == EMBUER_OK) {
        printf("Success: %s\n", result);
        embuer_free_string(result);
    } else {
        fprintf(stderr, "Failed: %d\n", code);
    }

    embuer_client_free(client);
    return 0;
}
```

### Example: Watch Status

```c
#include <stdio.h>
#include <embuer.h>

void on_status_change(
    const char* status,
    const char* details,
    int progress,
    void* user_data
) {
    printf("Status: %s - %s", status, details);
    if (progress >= 0) {
        printf(" (%d%%)", progress);
    }
    printf("\n");
}

int main() {
    embuer_client_t* client = embuer_client_new();
    if (!client) return 1;

    printf("Watching for updates...\n");

    // This blocks until interrupted
    embuer_watch_status(client, on_status_change, NULL);

    embuer_client_free(client);
    return 0;
}
```

### Compiling C Programs

```sh
gcc -o myapp myapp.c -lembuer -lpthread -ldl -lm
```

Or with explicit library path:

```sh
gcc -o myapp myapp.c -L/usr/local/lib -lembuer -lpthread -ldl -lm
```

## C++ Integration

C++ can use the C library directly:

```cpp
#include <iostream>
#include <memory>
#include <embuer.h>

// RAII wrapper for embuer_client_t
class EmbuerClient_RAII {
    embuer_client_t* client_;
public:
    EmbuerClient_RAII() : client_(embuer_client_new()) {
        if (!client_) {
            throw std::runtime_error("Failed to create Embuer client");
        }
    }

    ~EmbuerClient_RAII() {
        if (client_) {
            embuer_client_free(client_);
        }
    }

    embuer_client_t* get() { return client_; }
};

// RAII wrapper for strings
class EmbuerString {
    char* str_;
public:
    EmbuerString() : str_(nullptr) {}
    ~EmbuerString() {
        if (str_) {
            embuer_free_string(str_);
        }
    }

    char** ptr() { return &str_; }
    const char* get() const { return str_; }
};

int main() {
    try {
        EmbuerClient_RAII client;

        EmbuerString status, details;
        int progress = 0;

        int result = embuer_get_status(
            client.get(),
            status.ptr(),
            details.ptr(),
            &progress
        );

        if (result == EMBUER_OK) {
            std::cout << "Status: " << status.get() << "\n";
            std::cout << "Details: " << details.get() << "\n";
            if (progress >= 0) {
                std::cout << "Progress: " << progress << "%\n";
            }
        }
    } catch (const std::exception& e) {
        std::cerr << "Error: " << e.what() << "\n";
        return 1;
    }

    return 0;
}
```

Compile with:

```sh
g++ -o myapp myapp.cpp -lembuer -lpthread -ldl -lm
```

## Python Integration

Python can interact with Embuer via D-Bus using `pydbus` or `dbus-python`.

### Using pydbus

Install pydbus:

```sh
pip install pydbus
```

Example:

```python
from pydbus import SystemBus
from gi.repository import GLib

# Connect to the service
bus = SystemBus()
embuer = bus.get("org.neroreflex.embuer", "/org/neroreflex/embuer")

# Get status
status, details, progress = embuer.GetUpdateStatus()
print(f"Status: {status}")
print(f"Details: {details}")
if progress >= 0:
    print(f"Progress: {progress}%")

# Install update
result = embuer.InstallUpdateFromUrl("https://example.com/update.tar.gz")
print(result)

# Watch for status changes
def on_status_changed(status, details, progress):
    print(f"Status changed: {status} - {details} ({progress}%)")

embuer.onUpdateStatusChanged = on_status_changed

# Run event loop
loop = GLib.MainLoop()
loop.run()
```

### Using dbus-python

```python
import dbus
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib

DBusGMainLoop(set_as_default=True)

bus = dbus.SystemBus()
proxy = bus.get_object(
    "org.neroreflex.embuer",
    "/org/neroreflex/embuer"
)

interface = dbus.Interface(proxy, "org.neroreflex.embuer1")

# Get status
status, details, progress = interface.GetUpdateStatus()
print(f"Status: {status}")

# Install update
result = interface.InstallUpdateFromFile("/path/to/update.tar.gz")
print(result)
```

## Error Handling

### C Library Error Codes

```c
#define EMBUER_OK                0
#define EMBUER_ERR_NULL_PTR     -1
#define EMBUER_ERR_CONNECTION   -2
#define EMBUER_ERR_DBUS         -3
#define EMBUER_ERR_INVALID_STRING -4
#define EMBUER_ERR_RUNTIME      -5
```

Always check return values:

```c
int result = embuer_get_status(client, &status, &details, &progress);
if (result != EMBUER_OK) {
    fprintf(stderr, "Error code: %d\n", result);
    // Handle error
}
```

## Best Practices

1. **Connection Management**: Reuse client connections rather than creating new ones for each operation
2. **Memory Management**: Always free strings returned by the C library
3. **Error Checking**: Always check return values and handle errors appropriately
4. **Signal Handling**: Use signals for real-time status updates instead of polling
5. **Permissions**: Remember that the service requires root privileges

## Complete Examples

### Full-Featured Example

See `examples/embuer_example.c` for a complete, working example demonstrating all features.

```sh
# Build and run the example
make example
make run-example

# Or manually
gcc -o embuer_example examples/embuer_example.c -I. -L./target/release -lembuer -lpthread -ldl -lm
LD_LIBRARY_PATH=./target/release ./embuer_example
```

### Status Monitor Example

See `examples/status_monitor.c` for a focused example that demonstrates real-time status monitoring with a beautiful terminal UI.

Features:
- Real-time status updates with color-coded output
- Progress bar visualization
- Timestamp for each update
- Session statistics (duration, update count, update rate)
- Graceful shutdown with Ctrl+C

```sh
# Build and run the status monitor
make status-monitor
make run-monitor

# Or manually
gcc -o status_monitor examples/status_monitor.c -I. -L./target/release -lembuer -lpthread -ldl -lm
LD_LIBRARY_PATH=./target/release ./status_monitor
```

### Building All Examples

```sh
make examples
```

