# US Explorer

A ground-level exploration game of the real United States: real terrain, imagery and
roads streamed around you, under the weather that is happening there right now.

Written in Rust on the [Bevy](https://bevy.org) engine. Linux AppImage releases, and
Windows builds for some phases.

- **Download:** [latest release](https://github.com/goglesquirmintontheiii/us-explorer/releases/latest). Linux: `chmod +x US_Explorer-*.AppImage` and run it. Windows: run the `.exe`. The game updates itself when a new version is out.
- **Progress:** [progress.md](progress.md) (current phase) and [progress/](progress) (earlier phases).

## Controls

Keys can be changed in Settings → Controls; gamepads work too.

| Key | Action |
| --- | --- |
| Click / Tab | Lock the mouse to look around (Esc frees it) |
| W A S D | Move |
| Space | Jump; double-tap to fly or land; up while flying |
| Shift | Down while flying (dive while swimming) |
| Ctrl | Sprint |
| Mouse wheel | Fly speed (zoom in orbit view) |
| V or F5 | First person, third person, orbit |
| M | Full map (search, teleport, bookmarks) |
| T | Go to a place |
| B | Bookmark this spot |
| N | No-clip |
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

- `crates/usx-core`: globe and tile math, disk cache, rate-limited downloads, settings, updater, geocoder, OpenStreetMap
- `crates/usx-weather`: the weather service (runs as `us-explorer --weather-service`)
- `crates/us-explorer`: the game
- `packaging/`: AppImage build and the screenshot list used by CI
