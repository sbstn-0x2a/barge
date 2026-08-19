//! Ausführung eines Moves als Transaktion (§7.1) und die Recovery-Operationen
//! (§7.2).
//!
//! Reihenfolge (§7.1): Journal STARTED → Dir-Komponenten nach `.partial` →
//! fsync → Prefix-Fix → atomares `rename` auf die Endnamen → Manifest ans Ziel
//! → **COMMITTED** → Quelle abräumen → Journal löschen. Bis zum `rename`
//! (Schritt 6) ist die Quelle unangetastet; ein Absturz davor ist folgenlos.

use std::fs::{self, File};
use std::io;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::mover::copy::{fsync_dir, Copier, Stats};
use crate::mover::journal::{ComponentState, JobState, Journal};
use crate::mover::plan::{partial_path, Action, MovePlan};
use crate::mover::prefix::fix_prefix;
use crate::steam::game::ComponentKind;

/// Führt den Move aus. `resume=true` überspringt bereits kopierte Dateien
/// (§7.2). `progress` wird von der Kopier-Engine mit laufenden Stats und der
/// gemessenen MB/s aufgerufen.
pub fn execute<F: FnMut(&Stats, f64)>(
    plan: &MovePlan,
    rate_bytes: u64,
    resume: bool,
    cancel: Arc<AtomicBool>,
    journal: &mut Journal,
    progress: F,
) -> io::Result<Stats> {
    journal.set_state(JobState::Copying)?;
    let mut copier = Copier::new(rate_bytes, progress)
        .skip_existing(resume)
        .cancel(cancel);

    // 2/3) Dir-Komponenten nach .partial kopieren, compatdata-Prefix fixen.
    for item in plan.items.iter().filter(|i| i.action == Action::MoveDir) {
        let partial = item.dst_partial.as_ref().expect("MoveDir hat .partial");

        // Bereits vollständig verschoben (Resume nach Teil-Commit)? Überspringen.
        if item.dst_final.exists() {
            journal.set_component(item.kind.label(), ComponentState::Done)?;
            continue;
        }

        journal.set_component(item.kind.label(), ComponentState::InProgress)?;
        copier.copy_tree(&item.src, partial)?;

        if item.kind == ComponentKind::Compatdata {
            // §4.3 / §7.1 Schritt 5 — im .partial, vor dem rename.
            fix_prefix(partial, &plan.source_library, &plan.target_library)?;
        }

        journal.set_bytes_done(copier.stats().bytes);
        journal.set_component(item.kind.label(), ComponentState::Done)?;
    }

    // 6) .partial -> Endname (atomar), Verzeichnis-fsync (§7.1 Schritt 6).
    for item in plan.items.iter().filter(|i| i.action == Action::MoveDir) {
        let partial = item.dst_partial.as_ref().unwrap();
        if item.dst_final.exists() && !partial.exists() {
            continue; // schon umbenannt
        }
        fs::rename(partial, &item.dst_final)?;
        if let Some(parent) = item.dst_final.parent() {
            let _ = fsync_dir(parent);
        }
    }

    // 7) Dateien (Manifest, Workshop-ACF) ans Ziel schreiben.
    for item in plan.items.iter().filter(|i| i.action == Action::MoveFile) {
        journal.set_component(item.kind.label(), ComponentState::InProgress)?;
        if let Some(parent) = item.dst_final.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&item.src, &item.dst_final)?;
        if let Ok(f) = File::open(&item.dst_final) {
            let _ = f.sync_all();
        }
        if let Some(parent) = item.dst_final.parent() {
            let _ = fsync_dir(parent);
        }
        journal.set_component(item.kind.label(), ComponentState::Done)?;
    }

    // 8) Punkt ohne Rückkehr überschritten (§7.1 Schritt 8).
    journal.set_state(JobState::Committed)?;

    // 9/10) Quelle abräumen — Manifest zuerst (gilt dann als deinstalliert).
    delete_source_components(&plan.source_library, plan.appid, &plan.installdir)?;

    // 11) Journal löschen.
    journal.remove()?;

    Ok(copier.stats().clone())
}

/// Recovery „Aufräumen" (§7.2): Ziel-`.partial` löschen, Quelle bleibt intakt.
/// Nur für Jobs *vor* COMMITTED sinnvoll.
pub fn cleanup_target_partials(journal: &Journal) -> io::Result<()> {
    let dst_apps = journal.target_library.join("steamapps");
    for kind in [
        ComponentKind::Common,
        ComponentKind::Compatdata,
        ComponentKind::WorkshopContent,
    ] {
        let final_p = kind.path_in(&dst_apps, journal.appid, &journal.installdir);
        let partial = partial_path(&final_p);
        if partial.exists() {
            fs::remove_dir_all(&partial)?;
        }
    }
    journal.remove()
}

/// Recovery-Abschluss eines bereits COMMITTED-Jobs: das Ziel ist vollständig,
/// nur die Quell-Bereinigung wurde unterbrochen. Quelle abräumen, Journal weg.
pub fn finish_committed(journal: &Journal) -> io::Result<()> {
    delete_source_components(&journal.source_library, journal.appid, &journal.installdir)?;
    journal.remove()
}

/// Löscht alle Quell-Komponenten eines Spiels (Manifest zuerst, §7.1 Schritt 9,
/// dann Verzeichnisse Schritt 10). Fehlende Pfade werden ignoriert.
fn delete_source_components(source_library: &Path, appid: u32, installdir: &str) -> io::Result<()> {
    let src_apps = source_library.join("steamapps");

    // Manifest zuerst.
    remove_any(&ComponentKind::Manifest.path_in(&src_apps, appid, installdir))?;

    for kind in ComponentKind::ALL {
        if kind == ComponentKind::Manifest {
            continue;
        }
        remove_any(&kind.path_in(&src_apps, appid, installdir))?;
    }
    Ok(())
}

fn remove_any(path: &Path) -> io::Result<()> {
    let md = match fs::symlink_metadata(path) {
        Ok(md) => md,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if md.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}
