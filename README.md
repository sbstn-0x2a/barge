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
- [ ] Stufe 2 — Kopier-Engine standalone (Throttle, `fsync`, `copy_file_range`)
- [ ] Stufe 3 — Journal + Crash-Recovery
- [ ] Stufe 4 — Move-Orchestrierung (Vorbedingungen, Prefix-Fix, Trockenlauf)
- [ ] Stufe 5 — GUI (eframe/egui)
- [ ] Stufe 6 — Feinschliff, AppImage

## Build & Run

Reines Rust, Stufe 1 ist dependency-frei (nur `std` + libc-Syscalls):

```bash
cargo build
cargo run              # alle erkannten Libraries + Spiele auflisten
cargo run -- <PFAD>    # bestimmten Library-Root (oder steamapps/) auflisten
cargo test             # Unit-Tests (VDF-Parser, Größenformatierung)
```

## Lizenz

GPL-3.0
