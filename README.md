# barge

*English · [Deutsch](README.de.md)*

**Move Steam games between libraries, safely and at your own pace.**

A *barge* is a slow cargo vessel: it moves huge loads deliberately slowly. That
is exactly the point here — not faster than Steam, but *throttled*.

## Problem

Steam's built-in “move install folder” copies with high parallelism and a deep
queue depth. On certain hardware combinations — specifically an NVMe in an
external USB4 enclosure — that load profile causes the tunnelled PCIe device to
drop its link, freezing the whole system.

`barge` deliberately mimics the load profile of a conventional file manager:
**a single worker thread, sequential, throttled bandwidth, periodic `fsync`** —
plus the Steam metadata (manifest, Proton prefix, Workshop mods) that is easy to
forget when moving things by hand.

## Features

- **Two-panel interface** (source/target) with cover art (list **or** tile
  view), click-to-select, sizes and disk usage.
- **Throttled, sequential copying** (adjustable max rate, `copy_file_range`,
  sparse-file preservation, `fsync`) — moves *all* per-game components: game
  data, manifest, `compatdata` (savegames + Proton prefix), Workshop mods;
  shadercache/downloading as chosen.
- **Transactional**: `.partial` → atomic `rename` → only then delete the source.
  A crash mid-move is harmless; **recovery right inside the GUI** (resume /
  discard / finish).
- **Safety preconditions** before every move (Steam not running, enough space,
  target registered, no conflict …) and a **dry run**.
- **Fast verification** after copying, a **queue** for several games, **prefix
  fix** for Proton drive letters.
- Persistent settings (window size, zoom, rate, theme, split, source/target),
  **themes** (dark/light/contrast), **DE/EN** language switch, log/config access.
- Fully usable as a **CLI** too (`list`, `copy`, `move`, `recover`).

<!-- Screenshot: docs/screenshot.png (placeholder) -->

## Installation

Prebuilt packages are attached to the [GitHub releases](https://github.com/sbstn-0x2a/barge/releases)
(built by CI when a version is tagged):

- **AppImage** — `chmod +x barge-*.AppImage && ./barge-*.AppImage`, runs across
  distributions.
- **Tarball** (`.tar.gz`) — portable binary plus icon/desktop file.
- **`.deb`** (Debian/Ubuntu) and **`.rpm`** (Fedora/openSUSE).
- **AUR** (Arch): `PKGBUILD` under `packaging/`.

## Building from source

Rust (edition 2021). GUI build dependencies: OpenGL, Wayland/X11 and
`libxkbcommon` development packages. The core engine is intentionally
dependency-light; the GUI/extras use `eframe`, `egui_extras`, `serde`, `image`,
`rfd`, `ureq`.

```bash
cargo build --release
cargo run                        # start the graphical interface (default)
cargo run -- list                # list all detected libraries + games (CLI)
cargo run -- list <PATH>         # list a specific library root (or steamapps/)
cargo run -- copy <SRC> <DST>    # standalone copy engine (default max 250 MB/s)
cargo run -- move <SRC-LIB> <DST-LIB> <APPID>…  # full move with journal
cargo run -- move <SRC-LIB> <DST-LIB> <APPID> --dry-run   # plan + checks, no change
cargo run -- recover             # show / resume unfinished jobs
cargo test                       # unit tests
```

The interface language follows your locale and can be switched (DE/EN) in the
window; it is remembered.

## Command-line interface

barge also works headlessly. The CLI is English-only (a fallback); the GUI is
bilingual (DE/EN).

- **`barge`** (or `barge gui`) — start the graphical interface (default).
- **`barge list [PATH …]`** — list detected libraries and their games with real
  on-disk size and install state (tools/runtimes shown separately). With paths,
  list those specific library roots (or a `steamapps/` folder).
  ```bash
  barge list
  barge list /mnt/Games/SteamLibrary
  ```
- **`barge copy <SRC> <DST> [--limit MB/s | --unlimited]`** — standalone copy
  engine: throttled, sequential, with `fsync`. Copies any directory tree (not a
  full Steam move). Default 250 MB/s.
  ```bash
  barge copy "$HOME/Games/SteamLibrary/steamapps/common/Some Game" /mnt/scratch --limit 100
  ```
- **`barge move <SRC-LIB> <DST-LIB> <APPID …> [options]`** — full transactional
  move of one or more games (queue) with §5 preconditions, journal and crash
  recovery. AppIDs come from `barge list`. Options: `--dry-run`, `--limit MB/s`,
  `--keep-shadercache`, `--no-verify`.
  ```bash
  # Dry run: show the plan and all checks, touch nothing
  barge move "$HOME/Games/SteamLibrary" /mnt/Games/SteamLibrary 2784470 --dry-run
  # Move two games at 250 MB/s
  barge move "$HOME/Games/SteamLibrary" /mnt/Games/SteamLibrary 2784470 960090 --limit 250
  ```
- **`barge recover [cleanup|resume|finish <ID>]`** — list unfinished jobs, or
  clean up / resume / finish one by ID (the GUI can do this too).
- **`barge --help`** — this help.

## Detected Steam locations

barge checks the standard Steam roots and then follows `libraryfolders.vdf` to
find every registered library:

- `~/.local/share/Steam`, `~/.steam/steam`, `~/.steam/root`
- `~/.var/app/com.valvesoftware.Steam/.local/share/Steam` — **Flatpak Steam**

All paths are canonicalized (symlinks resolved) and de-duplicated. Additional
libraries on other drives are discovered via `libraryfolders.vdf`; you can also
add folders manually in the GUI. `libraryfolders.vdf` is only ever read, never
written.

## Platforms

barge is Linux-only. Its core (Proton `compatdata`, `copy_file_range`,
`posix_fadvise`, `statvfs`, `/proc` scanning, symlink/hardlink handling) is
Unix-specific; a Windows build would be a separate port.

## License

GPL-3.0
