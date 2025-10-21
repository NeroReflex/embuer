# Embuer C Library Examples

This directory contains example programs demonstrating how to use the Embuer C library.

## Examples

### 1. embuer_example.c - Full-Featured Example

A comprehensive example demonstrating all features of the Embuer C library.

**Features:**
- Query current update status
- Install updates from files
- Install updates from URLs
- Watch for status changes in real-time
- Proper error handling and memory management

**Usage:**
```bash
# Build
make example
# or
gcc -o embuer_example embuer_example.c -I.. -L../target/release -lembuer -lpthread -ldl -lm

# Run - Get status
LD_LIBRARY_PATH=../target/release ./embuer_example

# Install from file
LD_LIBRARY_PATH=../target/release ./embuer_example --install-file /path/to/update.tar.gz

# Install from URL
LD_LIBRARY_PATH=../target/release ./embuer_example --install-url https://example.com/update.tar.gz

# Watch for status changes
LD_LIBRARY_PATH=../target/release ./embuer_example --watch
```

### 2. status_monitor.c - Status Monitoring Example

A focused example demonstrating real-time status monitoring with a polished terminal UI.

**Features:**
- **Color-coded status display** - Different colors for different states (Idle, Downloading, Installing, etc.)
- **Progress bar visualization** - Visual representation of download/install progress
- **Timestamps** - Every status update includes a timestamp
- **Session statistics** - Duration, update count, and update rate
- **Graceful shutdown** - Handles Ctrl+C cleanly
- **User-friendly interface** - Clear, organized output

**Usage:**
```bash
# Build
make status-monitor
# or
gcc -o status_monitor status_monitor.c -I.. -L../target/release -lembuer -lpthread -ldl -lm

# Run (press Ctrl+C to exit)
LD_LIBRARY_PATH=../target/release ./status_monitor
# or
make run-monitor
```

**Output Example:**
```
┌─────────────────────────────────────────────────────────────────────────────┐
│                    Embuer Update Status Monitor                            │
├─────────────────────────────────────────────────────────────────────────────┤
│ Press Ctrl+C to exit                                                        │
└─────────────────────────────────────────────────────────────────────────────┘

Current Status:
───────────────
[2025-10-21 14:52:15] Idle            │ No update in progress                    │                      N/A

Monitoring for updates...
────────────────────────────────────────────────────────────────────────────────
[2025-10-21 14:52:30] Downloading     │ Fetching update from server              │ [████████████░░░░░░░]  60%
[2025-10-21 14:52:45] Installing      │ Extracting update files                  │ [███████████████░░░░]  75%
[2025-10-21 14:53:00] Completed       │ Update installed successfully            │                      N/A
```

## Building All Examples

```bash
# Build all examples at once
make examples

# Or build individually
make example         # Build embuer_example
make status-monitor  # Build status_monitor
```

## Prerequisites

1. **Build the Embuer library first:**
   ```bash
   cd ..
   cargo build --release
   ```

2. **Ensure the service is running:**
   ```bash
   sudo ../target/release/embuer-service
   ```

3. **System requirements:**
   - GCC or Clang compiler
   - D-Bus system bus
   - Embuer service running

## Running Examples

### Option 1: Using Make (Recommended)

```bash
# Run the full-featured example
make run-example

# Run the status monitor
make run-monitor
```

### Option 2: Manual Execution

Set the library path before running:

```bash
export LD_LIBRARY_PATH=../target/release:$LD_LIBRARY_PATH

# Then run examples
./embuer_example
./status_monitor
```

### Option 3: System-Wide Installation

If you've installed the library system-wide:

```bash
cd ..
sudo make install
cd examples

# Now you can run directly without LD_LIBRARY_PATH
./embuer_example
./status_monitor
```

## Cleaning Up

```bash
# Remove built examples
make clean

# Or manually
rm -f embuer_example status_monitor
```

## Learning Path

If you're new to the Embuer C library, we recommend this learning order:

1. **Start with `embuer_example.c`**
   - Read through the code to understand the basic API
   - Run it to see status queries and update installations
   - Try the `--watch` flag to see status monitoring

2. **Study `status_monitor.c`**
   - See how to build a polished monitoring application
   - Learn about callback handling and user data
   - Understand signal handling for clean shutdown

3. **Build your own application**
   - Use these examples as templates
   - Refer to `../embuer.h` for complete API reference
   - Check `../INTEGRATION.md` for integration patterns

## Troubleshooting

### "Failed to create Embuer client"

**Problem:** Cannot connect to the service

**Solutions:**
1. Make sure the service is running: `sudo ../target/release/embuer-service`
2. Check D-Bus is running: `systemctl status dbus`
3. Verify you have D-Bus access permissions

### "cannot find -lembuer"

**Problem:** Linker cannot find the library

**Solutions:**
1. Ensure you built the library: `cd .. && cargo build --release`
2. Use `LD_LIBRARY_PATH`: `export LD_LIBRARY_PATH=../target/release:$LD_LIBRARY_PATH`
3. Or install system-wide: `cd .. && sudo make install`

### Compilation errors

**Problem:** Header not found or compilation fails

**Solutions:**
1. Use `-I..` to include the header directory
2. Ensure `embuer.h` exists in the parent directory
3. Check you have GCC or Clang installed

## API Reference

See the main header file for complete API documentation:

```bash
cat ../embuer.h
```

Or read the integration guide:

```bash
cat ../INTEGRATION.md
```

## Contributing

Found a bug or have an improvement? These examples are part of the main Embuer project. Please report issues or submit improvements to the main repository.

## License

These examples are part of the Embuer project and share the same license.

