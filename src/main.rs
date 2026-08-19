//! barge — Move Steam games between libraries, safely and at your own pace.
//!
//! Stufe 1 (Handout §11): Discovery + Parsing mit reiner CLI-Ausgabe. Findet
//! Steam-Libraries, listet die installierten Spiele mit realer On-Disk-Größe
//! und Installationszustand. Damit ist das Datenmodell validierbar, bevor
//! Kopier-Engine (§6) und GUI (§8) folgen.

mod steam;
mod util;

use std::path::PathBuf;

use steam::game::Game;
use steam::library::Library;
use util::human_size;

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

    // Ohne Argumente: Standard-Discovery. Mit Pfad-Argumenten: diese Pfade auf
    // Library-Roots normalisieren (§3.2) und einzeln auflisten.
    let libraries: Vec<Library> = if args.is_empty() {
        steam::discovery::discover()
    } else {
        let mut libs = Vec::new();
        for arg in &args {
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

fn print_usage() {
    println!(
        "barge {} — Move Steam games between libraries, safely and at your own pace\n\n\
         Stufe 1: Steam-Libraries und installierte Spiele auflisten.\n\n\
         AUFRUF:\n\
         \x20 barge                 alle erkannten Libraries auflisten\n\
         \x20 barge <PFAD> [PFAD…]  bestimmte Library-Roots (oder steamapps/) auflisten\n\
         \x20 barge -h | --help     diese Hilfe\n",
        env!("CARGO_PKG_VERSION")
    );
}
