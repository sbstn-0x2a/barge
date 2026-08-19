//! barge — Move Steam games between libraries, safely and at your own pace.
//!
//! Stufe 1 (Handout §11): Discovery + Parsing mit reiner CLI-Ausgabe. Findet
//! Steam-Libraries, listet die installierten Spiele mit realer On-Disk-Größe
//! und Installationszustand. Damit ist das Datenmodell validierbar, bevor
//! Kopier-Engine (§6) und GUI (§8) folgen.

mod config;
mod i18n;
mod mover;
mod steam;
mod ui;
mod util;

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use mover::copy::{Copier, Stats};
use mover::journal::{JobState, Journal};
use mover::plan::MovePlan;
use steam::game::Game;
use steam::library::Library;
use i18n::{tr, trf};
use steam::manifest;
use util::{dir_real_size, human_size};

#[derive(Default)]
struct Totals {
    games: usize,
    tools: usize,
}

fn main() {
    // Die CLI ist immer Englisch (nur ein Fallback). Die GUI überschreibt die
    // Sprache in BargeApp::new aus der gespeicherten Config.
    i18n::set_lang(i18n::Lang::En);

    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "-h" || a == "--help") {
        print_usage();
        return;
    }

    // Ohne Argumente: GUI (§8). Subcommands bleiben als CLI erhalten.
    match args.first().map(String::as_str) {
        None | Some("gui") => run_gui(),
        Some("copy") => cmd_copy(&args[1..]),
        Some("move") => cmd_move(&args[1..]),
        Some("recover") => cmd_recover(&args[1..]),
        Some("list") => {
            warn_incomplete_jobs();
            cmd_list(&args[1..]);
        }
        Some(other) => {
            eprintln!("{}", trf("Unbekannter Befehl: {}. Siehe `barge --help`.", "Unknown command: {}. See `barge --help`.", &[other]));
            std::process::exit(2);
        }
    }
}

fn run_gui() {
    if let Err(e) = ui::run() {
        eprintln!("GUI failed to start: {}", e);
        eprintln!("No display? Use the CLI — see `barge --help`.");
        std::process::exit(1);
    }
}

/// Beim Start auf unvollendete Jobs hinweisen (§7.2).
fn warn_incomplete_jobs() {
    let open = Journal::scan_incomplete();
    if !open.is_empty() {
        eprintln!(
            "{}\n",
            trf(
                "(!) {} unvollendete(r) Move-Job(s) gefunden. Details/Recovery: `barge recover`",
                "(!) {} unfinished move job(s) found. Details/recovery: `barge recover`",
                &[&open.len().to_string()],
            )
        );
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
                None => eprintln!("Not a Steam library: {}", arg),
            }
        }
        libs
    };

    if libraries.is_empty() {
        eprintln!("No Steam libraries found.");
        eprintln!("Standard locations checked (§3.3): ~/.local/share/Steam, ~/.steam/steam, ...");
        std::process::exit(1);
    }

    println!("barge — Steam libraries\n");

    let mut totals = Totals::default();
    for (idx, lib) in libraries.iter().enumerate() {
        print_library(idx, lib, &mut totals);
        println!();
    }

    println!(
        "{} library/libraries · {} game(s) · {} tools/runtimes.",
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
                "{} / {} used ({} %), {} free",
                human_size(used),
                human_size(total),
                pct,
                human_size(avail)
            )
        }
        None => "disk space unknown".to_string(),
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
        println!("  (no games installed)");
    }

    if !real.is_empty() {
        println!("  Games (by size):");
        for (game, size) in &real {
            totals.games += 1;
            print_row(game, *size);
        }
    }

    if !tools.is_empty() {
        println!("  Tools & runtimes:");
        for (game, size) in &tools {
            totals.tools += 1;
            print_row(game, *size);
        }
    }

    for (path, err) in &errors {
        eprintln!("  ! Manifest unreadable: {} ({})", path.display(), err);
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
                eprintln!("Unknown argument: {}", other);
                std::process::exit(2);
            }
        }
    }

    let (Some(src), Some(dst)) = (src, dst) else {
        eprintln!("Usage: barge copy <SRC> <DST> [--limit MB/s | --unlimited]");
        std::process::exit(2);
    };

    if !src.exists() {
        eprintln!("Source does not exist: {}", src.display());
        std::process::exit(2);
    }
    if dst.exists() {
        // §5.5 sinngemäß: nicht überschreiben.
        eprintln!("Target already exists, not overwriting: {}", dst.display());
        std::process::exit(3);
    }

    let total = dir_real_size(&src);
    let limit_label = if limit_mbps == 0 {
        "unlimited (no throttle — for comparison measurements only)".to_string()
    } else {
        format!("max. {} MB/s", limit_mbps)
    };
    println!("barge copy — copy engine");
    println!("  Source : {}", src.display());
    println!("  Target : {}", dst.display());
    println!("  Size   : {} (real, on-disk)", human_size(total));
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
            eprintln!("Cannot create target directory: {}", e);
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
            eprintln!("\nCopy error: {}", e);
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
        "  Files {}, directories {}, symlinks {}, hardlinks {}",
        st.files, st.dirs, st.symlinks, st.hardlinks
    );
    println!(
        "  copy_file_range: {} ok, {} Fallbacks",
        st.cfr_ok, st.cfr_fallback
    );
    println!(
        "  {} in {:.1} s  →  measured {:.1} MB/s",
        human_size(st.bytes),
        secs,
        copier.measured_mbps()
    );
}

/// `barge move <QUELL-LIB> <ZIEL-LIB> <APPID>… [Optionen]`
///
/// Vollständiger, transaktionaler Move mit voller Vorbedingungsprüfung (§5),
/// Journal + Crash-Recovery (§7) und optionalem Trockenlauf (§8.4). Mehrere
/// AppIDs werden als Warteschlange nacheinander verarbeitet (§14).
fn cmd_move(args: &[String]) {
    let mut positional: Vec<&String> = Vec::new();
    let mut limit_mbps: u64 = 250;
    let mut delete_shadercache = true;
    let mut crash_after_mb: u64 = 0;
    let mut dry_run = false;
    let mut verify = true;

    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--unlimited" => limit_mbps = 0,
            "--keep-shadercache" => delete_shadercache = false,
            "--no-verify" => verify = false,
            "--dry-run" => dry_run = true,
            "--limit" => limit_mbps = parse_num(it.next(), "--limit"),
            // Nur für Tests: nach N MB hart abbrechen (simuliert kill -9, §12).
            "--crash-after-mb" => crash_after_mb = parse_num(it.next(), "--crash-after-mb"),
            other if other.starts_with("--") => {
                eprintln!("Unknown option: {}", other);
                std::process::exit(2);
            }
            _ => positional.push(a),
        }
    }

    if positional.len() < 3 {
        eprintln!("Usage: barge move <SRC-LIB> <DST-LIB> <APPID>… [--dry-run] [--limit MB/s] [--keep-shadercache]");
        std::process::exit(2);
    }
    let source = normalize_lib_or_exit(positional[0]);
    let target = normalize_lib_or_exit(positional[1]);
    let src_apps = source.join("steamapps");

    // --- Warteschlange aufbauen: je AppID Spiel laden und Plan bilden (§14).
    let mut queue: Vec<(Game, MovePlan)> = Vec::new();
    for raw in &positional[2..] {
        let appid: u32 = match raw.parse() {
            Ok(v) => v,
            Err(_) => {
                eprintln!("AppID must be a number: {}", raw);
                std::process::exit(2);
            }
        };
        let manifest_path = src_apps.join(format!("appmanifest_{}.acf", appid));
        let m = match manifest::read(&manifest_path) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("Game {} not in source library: {}", appid, e);
                std::process::exit(2);
            }
        };
        let game = Game::from_manifest(m, &source, &src_apps);
        let choice = mover::plan::ComponentChoice {
            compatdata: true,
            workshop: true,
            move_shadercache: !delete_shadercache,
        };
        let plan = MovePlan::new(&game, &target, choice);
        if plan.items.is_empty() {
            eprintln!("AppID {}: nothing to move (no components found).", appid);
            std::process::exit(2);
        }
        queue.push((game, plan));
    }

    let limit_label = if limit_mbps == 0 {
        "unbegrenzt".to_string()
    } else {
        format!("max. {} MB/s", limit_mbps)
    };
    println!("barge move{}", if dry_run { " — DRY RUN (§8.4, no change)" } else { "" });
    println!("  Source : {}", source.display());
    println!("  Target : {}", target.display());
    println!("  Limit  : {}", limit_label);
    println!("  Queue  : {} game(s)\n", queue.len());

    // --- Vorbedingungen je Spiel (§5) und Plan anzeigen.
    let mut all_ok = true;
    for (game, plan) in &queue {
        print_plan(plan);
        let report = mover::preconditions::check(game, plan);
        print_report(&report);
        if !report.all_passed() {
            all_ok = false;
        }
        println!();
    }

    if dry_run {
        println!(
            "Dry run finished — {}.",
            if all_ok { "all preconditions met" } else { "there are unmet preconditions (see ✗)" }
        );
        std::process::exit(if all_ok { 0 } else { 2 });
    }

    // --- Echte Ausführung: je Spiel Vorbedingungen unmittelbar davor erneut
    //     prüfen (Freiplatz ändert sich in der Queue), dann verschieben.
    let rate = limit_mbps.saturating_mul(1_000_000);
    let mut done = 0;
    for (idx, (game, plan)) in queue.iter().enumerate() {
        println!("\n[{}/{}] {} (AppID {})", idx + 1, queue.len(), plan.name, plan.appid);

        let report = mover::preconditions::check(game, plan);
        if !report.all_passed() {
            eprintln!("  skipped — preconditions not met:");
            for c in report.failures() {
                eprintln!("    ✗ {}: {}", c.name, c.detail);
            }
            continue;
        }

        if let Err(code) = run_single_move(plan, rate, verify, crash_after_mb) {
            std::process::exit(code);
        }
        done += 1;
    }

    println!("\nDone: {} of {} game(s) moved.", done, queue.len());
}

/// Führt einen einzelnen, bereits geprüften Move aus (Journal + Transaktion).
/// Gibt bei Fehler den gewünschten Exit-Code zurück.
fn run_single_move(plan: &MovePlan, rate: u64, verify: bool, crash_after_mb: u64) -> Result<(), i32> {
    let labels = plan.moved_component_labels();
    let mut journal = match Journal::create(
        plan.appid,
        &plan.name,
        &plan.installdir,
        &plan.source_library,
        &plan.target_library,
        &labels,
        plan.bytes_total,
    ) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("Cannot create journal: {}", e);
            return Err(1);
        }
    };

    let progress = make_progress(plan.bytes_total, crash_after_mb.saturating_mul(1_000_000));
    let cancel = Arc::new(AtomicBool::new(false));
    match mover::execute::execute(plan, rate, false, verify, cancel, &mut journal, progress) {
        Ok(st) => {
            eprintln!();
            print_move_stats(&st);
            Ok(())
        }
        Err(e) => {
            eprintln!("\nError: {}", e);
            let _ = journal.set_state(JobState::Failed);
            eprintln!("Job marked FAILED; source untouched. Recovery: `barge recover`");
            Err(1)
        }
    }
}

fn print_plan(plan: &MovePlan) {
    println!(
        "  ▸ {} (AppID {}) — {} across {} component(s):",
        plan.name,
        plan.appid,
        human_size(plan.bytes_total),
        plan.items.len()
    );
    for item in &plan.items {
        let verb = match item.action {
            mover::plan::Action::MoveDir | mover::plan::Action::MoveFile => "move",
            mover::plan::Action::DeleteSource => "delete (source)",
        };
        println!("      {:<12} {}", item.kind.label(), verb);
    }
}

fn print_report(report: &mover::preconditions::Report) {
    println!("    Preconditions (§5):");
    for c in &report.checks {
        println!("      {} {}: {}", if c.passed { "✓" } else { "✗" }, c.name, c.detail);
    }
}

/// `barge recover [cleanup|resume|finish <ID>]`
fn cmd_recover(args: &[String]) {
    let open = Journal::scan_incomplete();

    match args.first().map(String::as_str) {
        None => {
            if open.is_empty() {
                println!("No unfinished move jobs.");
                return;
            }
            println!("Unfinished move jobs:\n");
            for j in &open {
                println!("  ID     : {}", j.id);
                println!("  Game   : {} (AppID {})", j.name, j.appid);
                println!("  State: {:?}", j.state);
                println!("  {} → {}", j.source_library.display(), j.target_library.display());
                println!("  Progress: {} / {}", human_size(j.bytes_done), human_size(j.bytes_total));
                let hint = match j.state {
                    JobState::Committed => "finish  (target complete, only source cleanup left)",
                    _ => "cleanup (discard target .partial, source stays) OR resume",
                };
                println!("  → barge recover {} {}\n", hint.split_whitespace().next().unwrap(), j.id);
                println!("     recommended: {}\n", hint);
            }
        }
        Some(action) => {
            let id = match args.get(1) {
                Some(id) => id,
                None => {
                    eprintln!("Usage: barge recover {} <ID>", action);
                    std::process::exit(2);
                }
            };
            let job = match open.iter().find(|j| &j.id == id) {
                Some(j) => j.clone(),
                None => {
                    eprintln!("No job with ID {}", id);
                    std::process::exit(2);
                }
            };
            match action {
                "cleanup" => match mover::execute::cleanup_target_partials(&job) {
                    Ok(()) => println!("Cleaned up: target .partial removed, source intact. Job {}", id),
                    Err(e) => { eprintln!("Error while cleaning up: {}", e); std::process::exit(1); }
                },
                "finish" => match mover::execute::finish_committed(&job) {
                    Ok(()) => println!("Finished: source cleaned up. Job {}", id),
                    Err(e) => { eprintln!("Error while finishing: {}", e); std::process::exit(1); }
                },
                "resume" => {
                    let plan = match MovePlan::rebuild_from_source(&job) {
                        Ok(p) => p,
                        Err(e) => { eprintln!("Cannot resume (source unreadable?): {}", e); std::process::exit(1); }
                    };
                    let mut journal = job;
                    let total = plan.bytes_total;
                    println!("Resuming job {} ({})…", id, plan.name);
                    let progress = make_progress(total, 0);
                    let cancel = Arc::new(AtomicBool::new(false));
                    match mover::execute::execute(&plan, 250 * 1_000_000, true, true, cancel, &mut journal, progress) {
                        Ok(st) => { eprintln!(); println!("\n--- Resume finished ---"); print_move_stats(&st); }
                        Err(e) => { eprintln!("\nError: {}", e); std::process::exit(1); }
                    }
                }
                other => {
                    eprintln!("Unknown action: {} (cleanup|resume|finish)", other);
                    std::process::exit(2);
                }
            }
        }
    }
}

/// Fortschritts-Closure für Move/Resume; optional Crash-Injektion nach
/// `crash_bytes` (Test, simuliert kill -9).
fn make_progress(total: u64, crash_bytes: u64) -> impl FnMut(&Stats, f64) {
    let mut last_print = Instant::now();
    move |st: &Stats, avg_mbps: f64| {
        if crash_bytes > 0 && st.bytes >= crash_bytes {
            eprintln!("\n[TEST] Crash-Injektion nach {} — abort()", human_size(st.bytes));
            std::process::abort();
        }
        let now = Instant::now();
        if now.duration_since(last_print).as_millis() < 250 {
            return;
        }
        last_print = now;
        let pct = if total > 0 {
            (st.bytes as f64 / total as f64 * 100.0).min(100.0)
        } else {
            0.0
        };
        eprint!(
            "\r  {:>5.1} %  {} / {}  ·  {:.0} MB/s   ",
            pct, human_size(st.bytes), human_size(total), avg_mbps
        );
        let _ = std::io::Write::flush(&mut std::io::stderr());
    }
}

fn print_move_stats(st: &Stats) {
    println!(
        "  Files {} ({} skipped), directories {}, symlinks {}, hardlinks {}",
        st.files, st.skipped_files, st.dirs, st.symlinks, st.hardlinks
    );
    println!("  {} copied, {} holes preserved", human_size(st.bytes), st.holes);
}

fn parse_num(v: Option<&String>, flag: &str) -> u64 {
    match v.and_then(|s| s.parse::<u64>().ok()) {
        Some(n) => n,
        None => {
            eprintln!("{} erwartet eine Zahl", flag);
            std::process::exit(2);
        }
    }
}

fn normalize_lib_or_exit(arg: &str) -> PathBuf {
    match steam::discovery::normalize_root(Path::new(arg)) {
        Some(root) => root,
        None => {
            eprintln!("Not a Steam library: {}", arg);
            std::process::exit(2);
        }
    }
}

fn print_usage() {
    let body = tr(
        "AUFRUF:\n\
         \x20 barge                     grafische Oberfläche starten (Standard)\n\
         \x20 barge list                alle erkannten Libraries + Spiele auflisten\n\
         \x20 barge list <PFAD>…        bestimmte Library-Roots (oder steamapps/) auflisten\n\
         \x20 barge copy <QUELLE> <ZIEL> [--limit MB/s | --unlimited]\n\
         \x20                           Kopier-Engine standalone: gedrosselt, sequenziell, fsync\n\
         \x20 barge move <QUELL-LIB> <ZIEL-LIB> <APPID>… [--dry-run] [--limit MB/s]\n\
         \x20                           [--keep-shadercache] [--no-verify]\n\
         \x20                           vollständiger Move mit §5-Prüfung, Journal, Recovery\n\
         \x20 barge recover [cleanup|resume|finish <ID>]\n\
         \x20                           unvollendete Jobs anzeigen / aufräumen / fortsetzen\n\
         \x20 barge -h | --help         diese Hilfe",
        "USAGE:\n\
         \x20 barge                     start the graphical interface (default)\n\
         \x20 barge list                list all detected libraries + games\n\
         \x20 barge list <PATH>…        list specific library roots (or steamapps/)\n\
         \x20 barge copy <SRC> <DST> [--limit MB/s | --unlimited]\n\
         \x20                           standalone copy engine: throttled, sequential, fsync\n\
         \x20 barge move <SRC-LIB> <DST-LIB> <APPID>… [--dry-run] [--limit MB/s]\n\
         \x20                           [--keep-shadercache] [--no-verify]\n\
         \x20                           full move with §5 checks, journal, recovery\n\
         \x20 barge recover [cleanup|resume|finish <ID>]\n\
         \x20                           show / clean up / resume unfinished jobs\n\
         \x20 barge -h | --help         this help",
    );
    println!(
        "barge {} — Move Steam games between libraries, safely and at your own pace\n\n{}",
        env!("CARGO_PKG_VERSION"),
        body
    );
}
