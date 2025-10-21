# DBus Status Monitoring

## Overview

The embuer service now exposes update status via DBus without requiring polling. Clients (GUI applications, taskbar icons, etc.) can:

1. **Listen for status change signals** - Get notified automatically when update status changes
2. **Query current status** - Get the current status on demand

## DBus Interface

**Service Name:** `org.neroreflex.embuer`  
**Object Path:** `/org/neroreflex/login_ng_service`  
**Interface:** `org.neroreflex.embuer1`

## Update Statuses

- **Idle** - No update in progress
- **Checking** - Checking for updates (reserved for future use)
- **Downloading** - Downloading an update from a URL
- **Installing** - Installing an update
- **Completed** - Update successfully installed
- **Failed** - Update failed

## Methods

### 1. `GetUpdateStatus() -> (status: String, details: String, progress: i32)`

Get the current update status with progress information.

**Returns:**
- `status`: Current status (Idle, Downloading, Installing, Completed, Failed)
- `details`: Additional information (e.g., URL being downloaded, error message)
- `progress`: Progress percentage (0-100) for Downloading/Installing, or -1 if not applicable or size unknown

**Example (Python with pydbus):**
```python
from pydbus import SystemBus

bus = SystemBus()
embuer = bus.get("org.neroreflex.embuer", "/org/neroreflex/login_ng_service")

status, details, progress = embuer.GetUpdateStatus()
print(f"Status: {status}, Details: {details}, Progress: {progress}%")
```

### 2. `InstallUpdateFromFile(file_path: String) -> String`

Queue an update from a local file.

### 3. `InstallUpdateFromUrl(url: String) -> String`

Queue an update from a URL.

## Signals

### `UpdateStatusChanged(status: String, details: String, progress: i32)`

Emitted automatically when the update status changes. **No polling required!**

**Arguments:**
- `status`: Current status (Idle, Downloading, Installing, Completed, Failed)
- `details`: Additional information about the status
- `progress`: Progress percentage (0-100) for active downloads/installs, -1 otherwise

**Example (Python with pydbus):**
```python
from pydbus import SystemBus
from gi.repository import GLib

bus = SystemBus()
embuer = bus.get("org.neroreflex.embuer", "/org/neroreflex/login_ng_service")

def on_status_changed(status, details, progress):
    print(f"Update status: {status} - {details} ({progress}%)")
    # Update your GUI/taskbar icon and progress bar here
    if status in ["Downloading", "Installing"] and progress >= 0:
        # Update progress bar to show progress%
        print(f"Progress: {progress}%")
    elif status == "Completed":
        # Show success notification
        print("Update completed!")
    elif status == "Failed":
        # Show error notification
        print(f"Update failed: {details}")

# Subscribe to the signal
embuer.UpdateStatusChanged.connect(on_status_changed)

# Run the event loop
loop = GLib.MainLoop()
loop.run()
```

**Example (D-Feet / command line monitoring):**
```bash
dbus-monitor --system "type='signal',interface='org.neroreflex.embuer1',member='UpdateStatusChanged'"
```

## Starting the Status Monitor

In your main application, after creating the DBus interface, start the status monitor:

```rust
use zbus::Connection;
use embuer::dbus::EmbuerDBus;

// ... create your service ...

let connection = Connection::system().await?;
let embuer_dbus = EmbuerDBus::new(service.clone());

// Serve the DBus interface
connection
    .object_server()
    .at("/org/neroreflex/login_ng_service", embuer_dbus)
    .await?;

// Get the signal emitter for this object path
let signal_emitter = connection
    .object_server()
    .interface::<_, EmbuerDBus>("/org/neroreflex/login_ng_service")
    .await?;

// Start monitoring status changes and emitting signals
EmbuerDBus::start_status_monitor(service.clone(), signal_emitter).await;
```

## GUI Application Example

Your GUI application should:

1. **Connect to the DBus service** on startup
2. **Subscribe to `UpdateStatusChanged` signal**
3. **Update UI** when signal is received

No polling needed! The service will push notifications to your GUI automatically.

## Taskbar Icon Example

Similar to GUI:

1. Connect to DBus
2. Listen for signals
3. Update icon based on status (e.g., spinner during download/install, checkmark on completion, error icon on failure)

## Setup

### 1. Install DBus Configuration

Copy the DBus policy file to the system:

```bash
sudo cp rootfs/usr/share/dbus-1/system.d/org.neroreflex.embuer.conf \
        /usr/share/dbus-1/system.d/
```

### 2. Reload DBus Configuration

```bash
sudo systemctl reload dbus
# or
sudo killall -HUP dbus-daemon
```

## Testing

### Monitor Signals (requires sudo)

```bash
# Monitor all signals from the embuer service
sudo dbus-monitor --system "type='signal',interface='org.neroreflex.embuer1',member='UpdateStatusChanged'"
```

**Note:** Monitoring the system bus requires elevated privileges. The "AccessDenied" error you saw is normal for regular users.

### Trigger Updates

```bash
# Trigger an update from a file
dbus-send --system --print-reply \
  --dest=org.neroreflex.embuer \
  /org/neroreflex/login_ng_service \
  org.neroreflex.embuer1.InstallUpdateFromFile \
  string:"/path/to/update.btrfs.xz"

# Trigger an update from a URL
dbus-send --system --print-reply \
  --dest=org.neroreflex.embuer \
  /org/neroreflex/login_ng_service \
  org.neroreflex.embuer1.InstallUpdateFromUrl \
  string:"http://example.com/update.btrfs.xz"

# Get current status
dbus-send --system --print-reply \
  --dest=org.neroreflex.embuer \
  /org/neroreflex/login_ng_service \
  org.neroreflex.embuer1.GetUpdateStatus
```

