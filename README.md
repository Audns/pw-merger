# pw-merger

Merge two PipeWire audio sinks into one persistent virtual output — no
drag-and-drop, no re-wiring after pausing media.

## How it works

`pw-merger` creates a virtual null sink and directly links its monitor
ports to each target device's playback ports.  The link proxies are held
open so the connections survive media pauses and device reconnects.

## Dependencies

```bash
# Arch Linux
sudo pacman -S pipewire pipewire-audio

# Fedora
sudo dnf install pipewire-devel

# Debian / Ubuntu
sudo apt install libpipewire-0.3-dev
```

Rust toolchain: install via [rustup.rs](https://rustup.rs).

## Build

```bash
cargo build --release
# Binary: target/release/pw-merger
```

## Quick start

```bash
# 1. List available sinks
pw-merger --list

# 2. Merge two sinks by ID
pw-merger 55 61

# 3. (Optional) Give it a nice name
pw-merger -o "Speakers + HDMI" 55 61
```

Then in pavucontrol (or your player), select **"Speakers + HDMI"** as the
playback device.  Audio will play on both devices simultaneously.

Press `Ctrl-C` to stop.

## Usage

```text
pw-merger [OPTIONS] <DEVICE_A> <DEVICE_B>

Arguments:
  <DEVICE_A>  Sink ID or node name (see --list)
  <DEVICE_B>  Sink ID or node name (see --list)

Options:
  -l, --list              List available sinks and exit
  -o, --output <NAME>     Name for the merged sink [default: Merged Output]
      --media-role <ROLE>  Media role: Music, Movie, Game [default: Music]
  -v, --verbose           Verbose logging
  -h, --help              Print help
```

## Examples

```bash
# Merge by numeric ID (easiest)
pw-merger 55 61

# Merge by full node name
pw-merger alsa_output.pci-0000_08_00.1.hdmi-stereo \
          alsa_output.pci-0000_0a_00.4.iec958-stereo

# Custom sink name
pw-merger -o "Whole House Audio" 55 61

# Debug logging
pw-merger -v 55 61
RUST_LOG=debug pw-merger 55 61
```

## Autostart with systemd (user session)

Install the service file:

```bash
mkdir -p ~/.config/systemd/user
cp contrib/pw-merger.service ~/.config/systemd/user/

# Edit it to fill in your device IDs:
$EDITOR ~/.config/systemd/user/pw-merger.service

systemctl --user daemon-reload
systemctl --user enable --now pw-merger
systemctl --user status pw-merger
```

## Troubleshooting

**"no audio sink with ID"** — Run `pw-merger --list` to see valid IDs.
Device IDs can change across reboots.

**Audio plays on only one device** — Check `pw-merger --list` to confirm
both sinks are available.  Look for "links established" in the log output.

**Latency / xruns** — Increase the PipeWire quantum:
`pw-metadata -n settings 0 clock.force-quantum 2048`

**HDMI disconnects on suspend** — This is expected.  pw-merger will
automatically re-link when the device reappears in the registry.
