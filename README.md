# US Explorer

A ground-level exploration game of the real United States: real terrain and imagery
streamed around you, under the weather that is happening there right now.

Written in Rust on the [Bevy](https://bevy.org) engine. Linux AppImage releases;
the code also stays Windows-compatible.

- **Download:** [latest release](https://github.com/goglesquirmintontheiii/us-explorer/releases/latest), then `chmod +x US_Explorer-*.AppImage` and run it. The game updates itself when a new version is out.
- **Progress:** [progress.md](progress.md) (current phase) and [progress/](progress) (earlier phases).

## Controls (until the player character arrives)

| Key | Action |
| --- | --- |
| Right mouse / Tab | Look around |
| W A S D | Move |
| Space / Ctrl | Up / down |
| Shift | Faster |
| Mouse wheel | Change speed |
| T | Go to a place (search or coordinates) |
| Esc | Menu |
| F3 | Info overlay |
| [ / ] | Move the sun an hour (\ resets) |

## Building

```sh
sudo apt install libasound2-dev libudev-dev libwayland-dev libxkbcommon-dev   # Debian/Ubuntu
sudo dnf install alsa-lib-devel systemd-devel wayland-devel libxkbcommon-devel # Fedora
cargo run --release
```

## Layout

- `crates/usx-core`: globe and tile math, disk cache, rate-limited downloads, settings, updater, geocoder
- `crates/usx-weather`: the weather service process
- `crates/us-explorer`: the game
- `packaging/`: AppImage build and the screenshot list used by CI
