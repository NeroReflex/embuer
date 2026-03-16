# embuer-installer

`embuer-installer` is a standalone tool that:

- **Partitions and formats** a target disk or image with `btrfs`
- **Optionally installs a bootloader** (e.g. rEFInd)
- **Creates the root Embuer layout** (`deployments`, `deployments_data`, overlays)
- **Streams an initial deployment** from an Embuer update package (`update.tar`)

It is designed to work with the **same update package format** used by the running Embuer service:

- `CHANGELOG`
- `update.btrfs.xz`
- `update.signature`

The installer **never pre-downloads the whole package to disk**. For HTTP(S) sources, the `.tar` is streamed directly from the network, the `update.btrfs.xz` entry is extracted as a stream, and that stream is piped through decompression into `btrfs receive`.

---

## Basic usage

Build first:

```sh
cargo build --release
```

Run as root (you are partitioning/formatting disks):

```sh
sudo ./target/release/embuer-installer [options...]
```

Key options:

- **`-d, --device <PATH>`**: target block device (e.g. `/dev/sda`, `/dev/nvme0n1`)
- **`-i, --image <PATH>`**: target image file (loop-backed); created if missing
- **`--image-size <GiB>`**: size for a new image file when using `--image`
- **`-s, --deployment-source <SRC>`**: deployment source
  - local path to an **update package tar**
  - or an **HTTP(S) URL** pointing to an update package tar
  - or `"manual"` for fully manual population of the deployment rootfs
- **`-k, --deployment-name <NAME>`**: internal deployment name (subvolume name)
- **`-b, --bootloader <BOOT>`**: bootloader selection
  - `refind` – GPT + ESP partition + rEFInd installation
  - `imx8://<file>` – IMX8 bootloader from the given file (TODO in code)
- **`-a, --arch <ARCH>`**: architecture hint (e.g. `x86_64`, `arm64`, `imx8`)
- **`-n, --name <NAME>`**: human-readable installation name (used in boot menu)

For full options, run:

```sh
./target/release/embuer-installer --help
```

---

## Streaming a `.tar` over HTTP (no pre-download)

When `-s/--deployment-source` is set to an HTTP or HTTPS URL, `embuer-installer`:

1. Opens an HTTP(S) connection and obtains a **streaming body**.
2. Wraps that stream in a `tar` reader.
3. Scans entries until it finds:
   - `CHANGELOG` – read into memory and rendered in a **terminal UI** similar to `embuer-client pending-update`.
   - `update.btrfs.xz` – kept as a streaming reader.
4. Pipes `update.btrfs.xz` through decompression and into `btrfs receive` to create the deployment subvolume.

At no point is the entire `.tar` or `update.btrfs.xz` written to disk or loaded fully into RAM; everything is processed as a stream.

---

## Example: Install a distro from a remote update package

Assume you have published an Embuer-compatible update package at:

```text
https://updates.example.com/mydistro/embuer/update.tar
```

This `update.tar` contains:

- `CHANGELOG`
- `update.btrfs.xz`
- `update.signature`

To install this distro onto `/dev/sda` using rEFInd and architecture `x86_64`, run:

```sh
sudo ./target/release/embuer-installer \
  -d /dev/sda \
  -s https://updates.example.com/mydistro/embuer/update.tar \
  -k mydistro-1.0.0 \
  -n "MyDistro 1.0.0" \
  -b refind \
  -a x86_64
```

What this does:

- Partitions `/dev/sda` (creating an ESP and a `btrfs` rootfs partition for rEFInd).
- Formats the rootfs partition as `btrfs` and mounts it.
- Sets up the Embuer deployment layout on that filesystem.
- Streams the remote `update.tar` over HTTP, shows the **changelog TUI**, and then streams `update.btrfs.xz` into `btrfs receive` to create the deployment.
- Installs rEFInd configured to boot the new deployment.

After it completes successfully, `/dev/sda` should be a bootable disk with your Embuer-managed distro installed.

