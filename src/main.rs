//! barge — Move Steam games between libraries, safely and at your own pace.
//!
//! Stufe 1 (Handout §11): Discovery + Parsing mit reiner CLI-Ausgabe. Findet
//! Steam-Libraries, listet die installierten Spiele mit realer On-Disk-Größe
//! und Installationszustand. Damit ist das Datenmodell validierbar, bevor
//! Kopier-Engine (§6) und GUI (§8) folgen.

mod mover;
mod steam;
mod util;

use std::path::PathBuf;
use std::time::Instant;

use mover::copy::{Copier, Stats};
use steam::game::Game;
use steam::library::Library;
use util::{dir_real_size, human_size};

#[derive(Default)]
struct Totals {
    games: usize,
    tools: usize,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "-h" || a == "--help") {
        print_usage();
        return;
    }

    // Subcommands. Ohne Subcommand: Library-Listing (Stufe 1).
    match args.first().map(String::as_str) {
        Some("copy") => cmd_copy(&args[1..]),
        Some("list") => cmd_list(&args[1..]),
        _ => cmd_list(&args),
    }
}

fn cmd_list(args: &[String]) {
    // Ohne Argumente: Standard-Discovery. Mit Pfad-Argumenten: diese Pfade auf
    // Library-Roots normalisieren (§3.2) und einzeln auflisten.
    let libraries: Vec<Library> = if args.is_empty() {
        steam::discovery::discover()
    } else {
        let mut libs = Vec::new();
        for arg in args {
            match steam::discovery::normalize_root(&PathBuf::from(arg)) {
                Some(root) => libs.push(Library::new(root)),
                None => eprintln!("Keine Steam-Library: {}", arg),
            }
        }
        libs
    };

    if libraries.is_empty() {
        eprintln!("Keine Steam-Libraries gefunden.");
        eprintln!("Standard-Orte geprüft (§3.3): ~/.local/share/Steam, ~/.steam/steam, ...");
        std::process::exit(1);
    }

    println!("barge — Steam-Bibliotheken\n");

    let mut totals = Totals::default();
    for (idx, lib) in libraries.iter().enumerate() {
        print_library(idx, lib, &mut totals);
        println!();
    }

    println!(
        "{} Library/Libraries · {} Spiel(e) · {} Tools/Runtimes.",
        libraries.len(),
        totals.games,
        totals.tools
    );
}

fn print_library(idx: usize, lib: &Library, totals: &mut Totals) {
    let disk = match lib.disk_space() {
        Some((total, avail)) => {
            let used = total.saturating_sub(avail);
            let pct = if total > 0 {
                (used as f64 / total as f64 * 100.0).round() as u32
            } else {
                0
            };
            format!(
                "belegt {} / {} ({} %), frei {}",
                human_size(used),
                human_size(total),
                pct,
                human_size(avail)
            )
        }
        None => "Speicherplatz unbekannt".to_string(),
    };

    println!("Library {}: {}", idx, lib.path.display());
    println!("  {}", disk);

    let (games, errors) = lib.games();

    // Größen einmal berechnen (der Baum-Walk ist teuer), dann Spiele von
    // Tools/Runtimes trennen und beide Gruppen nach Größe absteigend sortieren.
    let mut sized: Vec<(Game, u64)> = games
        .into_iter()
        .map(|g| {
            let size = g.moved_size();
            (g, size)
        })
        .collect();
    let (mut tools, mut real): (Vec<_>, Vec<_>) =
        sized.drain(..).partition(|(g, _)| g.manifest.is_tool());
    real.sort_by(|a, b| b.1.cmp(&a.1));
    tools.sort_by(|a, b| b.1.cmp(&a.1));

    if real.is_empty() && tools.is_empty() {
        println!("  (keine installierten Spiele)");
    }

    if !real.is_empty() {
        println!("  Spiele (nach Größe):");
        for (game, size) in &real {
            totals.games += 1;
            print_row(game, *size);
        }
    }

    if !tools.is_empty() {
        println!("  Tools & Runtimes:");
        for (game, size) in &tools {
            totals.tools += 1;
            print_row(game, *size);
        }
    }

    for (path, err) in &errors {
        eprintln!("  ! Manifest unlesbar: {} ({})", path.display(), err);
    }
}

fn print_row(game: &Game, size: u64) {
    let m = &game.manifest;

    // Kürzel der vorhandenen, beweglichen Komponenten (§4).
    let comps: Vec<&str> = game
        .components
        .iter()
        .filter(|c| c.present && c.kind.is_moved())
        .map(|c| c.kind.label())
        .collect();
    let comps = comps.join("+");

    match m.blocked_reason() {
        None => println!(
            "  [✓] {:>10}  {:<40} {:>12}  {}",
            m.appid,
            truncate(&m.name, 40),
            human_size(size),
            comps
        ),
        Some(reason) => println!(
            "  [✗] {:>10}  {:<40} {:>12}  ⚠ {}",
            m.appid,
            truncate(&m.name, 40),
            human_size(size),
            reason
        ),
    }
}

/// Kürzt einen Anzeigenamen auf `max` Zeichen (mit Ellipse), zählt Unicode-
/// Zeichen korrekt.
fn truncate(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        s.to_string()
    } else {
        let keep: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{}…", keep)
    }
}

/// `barge copy <QUELLE> <ZIEL> [--limit MB/s | --unlimited]`
///
/// Standalone-Test der Kopier-Engine (§11 Stufe 2): kopiert einen beliebigen
/// Baum gedrosselt, sequenziell und mit periodischem `fsync` — genau das
/// Lastprofil, um das es im Projekt geht. Damit lässt sich die Engine gegen das
/// echte USB4-Szenario testen, bevor Journal und Move-Orchestrierung folgen.
fn cmd_copy(args: &[String]) {
    let mut src: Option<PathBuf> = None;
    let mut dst: Option<PathBuf> = None;
    let mut limit_mbps: u64 = 250; // §6.1 Default

    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--unlimited" => limit_mbps = 0,
            "--limit" => {
                match it.next().and_then(|v| v.parse::<u64>().ok()) {
                    Some(v) => limit_mbps = v,
                    None => {
                        eprintln!("--limit erwartet eine Zahl (MB/s)");
                        std::process::exit(2);
                    }
                }
            }
            _ if src.is_none() => src = Some(PathBuf::from(a)),
            _ if dst.is_none() => dst = Some(PathBuf::from(a)),
            other => {
                eprintln!("Unbekanntes Argument: {}", other);
                std::process::exit(2);
            }
        }
    }

    let (Some(src), Some(dst)) = (src, dst) else {
        eprintln!("Aufruf: barge copy <QUELLE> <ZIEL> [--limit MB/s | --unlimited]");
        std::process::exit(2);
    };

    if !src.exists() {
        eprintln!("Quelle existiert nicht: {}", src.display());
        std::process::exit(2);
    }
    if dst.exists() {
        // §5.5 sinngemäß: nicht überschreiben.
        eprintln!("Ziel existiert bereits, wird nicht überschrieben: {}", dst.display());
        std::process::exit(3);
    }

    let total = dir_real_size(&src);
    let limit_label = if limit_mbps == 0 {
        "unbegrenzt (⚠ ohne Drossel — nur für Vergleichsmessungen)".to_string()
    } else {
        format!("max. {} MB/s", limit_mbps)
    };
    println!("barge copy — Kopier-Engine (Stufe 2)");
    println!("  Quelle : {}", src.display());
    println!("  Ziel   : {}", dst.display());
    println!("  Größe  : {} (real, on-disk)", human_size(total));
    println!("  Limit  : {}", limit_label);
    match mover::copy::same_device(&src, &dst) {
        Ok(true) => println!(
            "  Hinweis: Quelle und Ziel auf demselben Gerät — ein echter Move nutzt\n\
             \x20          hier rename(2) (§6.4); dieser Test kopiert dennoch."
        ),
        _ => {}
    }
    println!();

    if let Some(parent) = dst.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("Zielverzeichnis nicht anlegbar: {}", e);
            std::process::exit(1);
        }
    }

    // Live-Fortschritt, auf ~4 Ausgaben/s begrenzt; gemessene Rate über ein
    // gleitendes Fenster zwischen zwei Callbacks (§8.2 sinngemäß).
    let start = Instant::now();
    let mut last_print = Instant::now();
    let mut last_bytes = 0u64;
    let mut last_t = Instant::now();
    let rate_bytes = limit_mbps.saturating_mul(1_000_000);

    let progress = |st: &Stats, avg_mbps: f64| {
        let now = Instant::now();
        if now.duration_since(last_print).as_millis() < 250 {
            return;
        }
        let dt = now.duration_since(last_t).as_secs_f64();
        let win_mbps = if dt > 0.0 {
            (st.bytes - last_bytes) as f64 / dt / 1_000_000.0
        } else {
            0.0
        };
        last_bytes = st.bytes;
        last_t = now;
        last_print = now;

        let pct = if total > 0 {
            (st.bytes as f64 / total as f64 * 100.0).min(100.0)
        } else {
            0.0
        };
        eprint!(
            "\r  {:>5.1} %  {} / {}  ·  {:.0} MB/s (Ø {:.0})   ",
            pct,
            human_size(st.bytes),
            human_size(total),
            win_mbps,
            avg_mbps,
        );
        let _ = std::io::Write::flush(&mut std::io::stderr());
    };

    let mut copier = Copier::new(rate_bytes, progress);
    match copier.copy_tree(&src, &dst) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("\nFehler beim Kopieren: {}", e);
            std::process::exit(1);
        }
    }

    // Verzeichnis-fsync, damit die neuen Einträge dauerhaft sind (§6.3).
    if let Some(parent) = dst.parent() {
        let _ = mover::copy::fsync_dir(parent);
    }

    let st = copier.stats().clone();
    let secs = start.elapsed().as_secs_f64();
    eprintln!(); // Fortschrittszeile abschließen
    println!("\n--- Ergebnis ---");
    println!(
        "  Dateien {}, Verzeichnisse {}, Symlinks {}, Hardlinks {}",
        st.files, st.dirs, st.symlinks, st.hardlinks
    );
    println!(
        "  copy_file_range: {} ok, {} Fallbacks",
        st.cfr_ok, st.cfr_fallback
    );
    println!(
        "  {} in {:.1} s  →  gemessen {:.1} MB/s",
        human_size(st.bytes),
        secs,
        copier.measured_mbps()
    );
}

fn print_usage() {
    println!(
        "barge {} — Move Steam games between libraries, safely and at your own pace\n\n\
         AUFRUF:\n\
         \x20 barge [list]              alle erkannten Libraries + Spiele auflisten\n\
         \x20 barge list <PFAD>…        bestimmte Library-Roots (oder steamapps/) auflisten\n\
         \x20 barge copy <QUELLE> <ZIEL> [--limit MB/s | --unlimited]\n\
         \x20                           Kopier-Engine standalone (Stufe 2): gedrosselt,\n\
         \x20                           sequenziell, mit fsync. Default 250 MB/s.\n\
         \x20 barge -h | --help         diese Hilfe\n",
        env!("CARGO_PKG_VERSION")
    );
}
