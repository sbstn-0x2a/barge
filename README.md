# barge

**Move Steam games between libraries, safely and at your own pace.**

Eine *Barge* ist ein Lastkahn: bewegt riesige Mengen, langsam, und zwar
absichtlich langsam. Genau das ist das Alleinstellungsmerkmal — nicht schneller
als Steam, sondern gedrosselter.

## Problem

Steams eingebaute „Installationsordner verschieben"-Funktion kopiert mit hoher
Parallelität und maximaler Queue Depth. Auf bestimmten Hardware-Kombinationen —
konkret NVMe in einem externen USB4-Gehäuse — führt dieses Lastprofil zum
Link-Verlust des getunnelten PCIe-Geräts und damit zum kompletten System-Freeze.

`barge` bildet bewusst das Lastprofil eines konventionellen Dateimanagers nach:
**ein Worker-Thread, sequenziell, gedrosselte Bandbreite, periodischer `fsync`** —
ergänzt um die Steam-Metadaten (Manifest, Proton-Prefix, Workshop-Mods), die beim
manuellen Verschieben leicht vergessen werden.

## Status

In Arbeit. Umsetzung in Stufen (siehe `docs/design.md`, §11):

- [x] **Stufe 1 — Discovery + Parsing (CLI).** Libraries finden, Spiele mit
      realer On-Disk-Größe und Installationszustand auflisten. *Nutzbar.*
- [x] **Stufe 2 — Kopier-Engine standalone.** Sequenziell, gedrosselt
      (Token-Pacer), periodischer `fsync`, `copy_file_range` mit `read/write`-
      Fallback, **Sparse-Erhalt** (`SEEK_HOLE`/`SEEK_DATA`), Hardlinks,
      Symlinks, Permissions + mtime. Als `barge copy`-Subcommand testbar.
- [x] **Stufe 3 — Journal + Crash-Recovery.** Transaktionaler Move (`.partial`
      → atomares `rename` → erst dann Quelle löschen), fsync-verankertes Journal
      unter `$XDG_STATE_HOME/barge/jobs/`, Recovery (`cleanup`/`resume`/`finish`),
      Prefix-Fix (§4.3). Crash-getestet (`kill`/`abort` mitten im Job).
- [x] **Stufe 4 — Move-Orchestrierung.** Voller Vorbedingungssatz (§5: Steam
      aus, StateFlags, Ziel registriert, beschreibbar, Freiplatz + 5 %, kein
      Konflikt), Trockenlauf (`--dry-run`, §8.4) und Warteschlange für mehrere
      AppIDs (§14).
- [x] **Stufe 5 — GUI (eframe/egui).** Zwei-Panel-Ansicht (§8.1): Quelle mit
      Auswahl-Checkboxen, Ziel, max.-Rate-Slider, Optionen, Trockenlauf.
      Verschieben läuft im Worker-Thread mit Live-Fortschritt (§8.2) und
      Abbrechen (§8.2). Start ohne Argumente (`barge`).
- [x] **Stufe 6 — Feinschliff.** Persistenz (Fenstergröße/Zoom/Limit/Panel/
      Quelle-Ziel/Theme), Library manuell hinzufügen (§8.3), schnelle
      Verifikation (§7.3), Cover-Bilder + CDN-Cache (§3.5), Kachel-/Listenansicht,
      Theme-Schalter (Dunkel/Hell/Kontrast), Recovery in der GUI, Log-/Config-/
      Jobs-Knöpfe, App-Icon, Distribution (AppImage/Tarball/.deb/.rpm/AUR via CI).

## Installation

Fertige Pakete hängen an den [GitHub-Releases](https://github.com/sbstn-0x2a/barge/releases)
(gebaut per CI beim Taggen einer Version):

- **AppImage** — `chmod +x barge-*.AppImage && ./barge-*.AppImage`, läuft
  distributionsübergreifend.
- **Tarball** (`.tar.gz`) — portables Binary plus Icon/Desktop-Datei.
- **`.deb`** (Debian/Ubuntu) und **`.rpm`** (Fedora/openSUSE).
- **AUR** (Arch): `PKGBUILD` unter `packaging/`.

## Build & Run

Reines Rust, Stufe 1 ist dependency-frei (nur `std` + libc-Syscalls):

```bash
cargo build
cargo run                        # grafische Oberfläche starten (Standard)
cargo run -- list                # alle erkannten Libraries + Spiele auflisten (CLI)
cargo run -- list <PFAD>         # bestimmten Library-Root (oder steamapps/) auflisten
cargo run -- copy <SRC> <DST>    # Kopier-Engine standalone (Default max. 250 MB/s)
cargo run -- move <QUELL-LIB> <ZIEL-LIB> <APPID>…  # vollständiger Move mit Journal
cargo run -- move <QUELL-LIB> <ZIEL-LIB> <APPID> --dry-run   # Plan + §5-Prüfung, ohne Änderung
cargo run -- recover             # unvollendete Jobs anzeigen / fortsetzen
cargo test                       # Unit-Tests
```

## Lizenz

GPL-3.0
