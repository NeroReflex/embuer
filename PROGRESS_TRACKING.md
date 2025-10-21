# Progress Tracking Implementation

## Overview

The update system now tracks and reports progress during downloads and installations, allowing your GUI applications to display accurate progress bars.

## How It Works

### 1. ProgressReader Wrapper

A custom `AsyncRead` wrapper that:
- Tracks bytes transferred in real-time
- Calculates percentage based on total size (when known)
- Updates the status automatically during read operations
- Throttles updates internally (updates happen on every read, which is frequent enough)

### 2. Size Detection

**For URL downloads:**
- Reads `Content-Length` header from HTTP response
- If present, enables accurate percentage tracking
- If absent, progress is reported as -1 (unknown)

**For file installations:**
- Reads file metadata to get total size
- Almost always available for local files
- Enables accurate percentage tracking

### 3. Progress Updates

Progress is updated as data flows through the stream:
- **URL downloads**: Progress during HTTP download phase
- **File installs**: Progress during file read and BTRFS receive
- Updates are emitted via DBus signals automatically
- No polling needed by clients!

## Progress Values

- **0-100**: Active download/install with known size
- **-1**: Size unknown or status is Idle/Completed/Failed

## Example Flow

```
Status: Idle, Progress: -1
   ↓ (update request received)
Status: Downloading, Progress: 0
   ↓ (bytes flowing...)
Status: Downloading, Progress: 25
   ↓
Status: Downloading, Progress: 50
   ↓
Status: Downloading, Progress: 75
   ↓
Status: Downloading, Progress: 100
   ↓ (download complete, starting install)
Status: Installing, Progress: 0
   ↓ (processing stream...)
Status: Installing, Progress: 45
   ↓
Status: Installing, Progress: 90
   ↓
Status: Completed, Progress: -1
```

## DBus Integration

### Signal Format

```
UpdateStatusChanged(status: String, details: String, progress: i32)
```

### Client Implementation (Python)

```python
def on_status_changed(status, details, progress):
    if progress >= 0:
        # Show progress bar
        progress_bar.set_value(progress)
        label.set_text(f"{status}: {progress}%")
    else:
        # Hide progress bar
        progress_bar.hide()
        label.set_text(f"{status}: {details}")
```

### Client Implementation (JavaScript/Electron)

```javascript
const { systemBus } = require('dbus-next');

const bus = systemBus();
const embuer = await bus.getProxyObject(
    'org.neroreflex.embuer',
    '/org/neroreflex/login_ng_service'
);
const iface = embuer.getInterface('org.neroreflex.embuer1');

iface.on('UpdateStatusChanged', (status, details, progress) => {
    if (progress >= 0) {
        // Update progress bar
        mainWindow.webContents.send('update-progress', {
            status,
            details,
            progress
        });
    } else {
        // Update status only
        mainWindow.webContents.send('update-status', {
            status,
            details
        });
    }
});
```

## Performance Notes

- Progress updates happen on every read operation (typically every few KB)
- DBus signals are only emitted when status actually changes
- The status monitor checks every 100ms for changes
- No performance impact on the update process itself
- Minimal overhead for progress tracking

## Testing

Monitor progress in real-time:

```bash
sudo dbus-monitor --system "type='signal',interface='org.neroreflex.embuer1',member='UpdateStatusChanged'"
```

You should see signals with increasing progress values during active downloads/installs.

## Known Limitations

1. **HTTP without Content-Length**: Some servers don't send Content-Length header
   - Progress will be -1 (unknown)
   - Status still updates (Downloading → Installing → Completed)

2. **Streaming compression**: When downloading compressed streams without knowing decompressed size
   - Progress tracks compressed bytes only
   - May show 100% before actual installation completes

3. **BTRFS receive**: No internal progress reporting from BTRFS
   - We track bytes read from our side
   - Accurate for measuring data transfer, not BTRFS processing time

