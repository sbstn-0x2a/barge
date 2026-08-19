//! Sicherheits-Vorbedingungen vor einem Move (§5).
//!
//! Alle Prüfungen liefern ein strukturiertes Ergebnis, damit sie sowohl den
//! echten Move blockieren als auch im Trockenlauf (§8.4) angezeigt werden
//! können. Keine Prüfung fasst eine Datei an.

use std::path::Path;

use crate::i18n::{tr, trf};
use crate::mover::plan::{Action, MovePlan};
use crate::steam::discovery;
use crate::steam::game::Game;
use crate::util::disk_space;

/// Sicherheitsmarge auf den Freiplatz (§5.4): 5 %.
const SPACE_MARGIN: f64 = 1.05;

#[derive(Debug, Clone)]
pub struct Check {
    pub name: &'static str,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub checks: Vec<Check>,
}

impl Report {
    pub fn all_passed(&self) -> bool {
        self.checks.iter().all(|c| c.passed)
    }

    pub fn failures(&self) -> impl Iterator<Item = &Check> {
        self.checks.iter().filter(|c| !c.passed)
    }
}

/// Führt alle Vorbedingungen aus §5 gegen einen konkreten Plan aus. `game`
/// liefert den Installationszustand, `plan` Ziel und Größe.
pub fn check(game: &Game, plan: &MovePlan) -> Report {
    let mut checks = Vec::new();

    // §5.1 — Steam darf nicht laufen.
    let steam_name = tr("Steam läuft nicht", "Steam is not running");
    checks.push(if discovery::steam_running() {
        fail(steam_name, tr("Steam ist aktiv — bitte beenden (kein „trotzdem fortfahren“)", "Steam is running — please quit it (no „proceed anyway“)"))
    } else {
        pass(steam_name, "ok")
    });

    // §5.6 — Spiel-Zustand sauber (StateFlags == 4).
    let inst_name = tr("Spiel vollständig installiert", "Game fully installed");
    checks.push(match game.manifest.blocked_reason() {
        None => pass(inst_name, "StateFlags = 4"),
        Some(reason) => fail(inst_name, &reason),
    });

    // §5.2 — Ziel-Library bei Steam registriert (in libraryfolders.vdf).
    checks.push(check_target_registered(&plan.target_library));

    // §5.3 — Zielverzeichnis für den aktuellen Nutzer beschreibbar.
    checks.push(check_writable(&plan.target_library));

    // §5.4 — genug Freiplatz am Ziel (reale Größe + 5 %).
    checks.push(check_free_space(&plan.target_library, plan.bytes_total));

    // §5.5 — kein Zielkonflikt (nichts überschreiben).
    checks.push(check_no_conflict(plan));

    Report { checks }
}

fn check_target_registered(target: &Path) -> Check {
    // Eine Library gilt als registriert, wenn sie über libraryfolders.vdf
    // erkannt wird (discover() liest ausschließlich diese Datei aus).
    let registered = discovery::discover()
        .iter()
        .any(|lib| lib.path == target);
    let name = tr("Ziel-Library registriert", "Target library registered");
    if registered {
        pass(name, tr("in libraryfolders.vdf gefunden", "found in libraryfolders.vdf"))
    } else {
        fail(
            name,
            tr(
                "nicht in libraryfolders.vdf — in Steam unter Einstellungen → Speicherplatz zuerst hinzufügen (barge schreibt die Datei nicht)",
                "not in libraryfolders.vdf — add it first in Steam under Settings → Storage (barge does not write this file)",
            ),
        )
    }
}

fn check_writable(target: &Path) -> Check {
    // Effektive Rechte testen, indem wir wirklich eine Datei anlegen (§5.3:
    // nicht über root/uid raten). steamapps/ existiert bei einer Library immer.
    let probe = target
        .join("steamapps")
        .join(format!(".barge_write_test_{}", std::process::id()));
    let name = tr("Ziel beschreibbar", "Target writable");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            pass(name, "ok")
        }
        Err(e) => fail(name, &trf("nicht beschreibbar: {}", "not writable: {}", &[&e.to_string()])),
    }
}

fn check_free_space(target: &Path, bytes_total: u64) -> Check {
    let needed = (bytes_total as f64 * SPACE_MARGIN) as u64;
    let name = tr("Genug Freiplatz", "Enough free space");
    match disk_space(target) {
        Some((_total, avail)) => {
            if avail >= needed {
                pass(
                    name,
                    &trf(
                        "{} frei ≥ {} nötig (inkl. 5 %)",
                        "{} free ≥ {} needed (incl. 5 %)",
                        &[&crate::util::human_size(avail), &crate::util::human_size(needed)],
                    ),
                )
            } else {
                fail(
                    name,
                    &trf(
                        "{} frei < {} nötig (inkl. 5 %) — es fehlen {}",
                        "{} free < {} needed (incl. 5 %) — {} missing",
                        &[
                            &crate::util::human_size(avail),
                            &crate::util::human_size(needed),
                            &crate::util::human_size(needed - avail),
                        ],
                    ),
                )
            }
        }
        None => fail(name, tr("statvfs auf das Ziel fehlgeschlagen", "statvfs on the target failed")),
    }
}

fn check_no_conflict(plan: &MovePlan) -> Check {
    let name = tr("Kein Zielkonflikt", "No target conflict");
    for item in &plan.items {
        if item.action != Action::DeleteSource && item.dst_final.exists() {
            return fail(
                name,
                &trf("existiert bereits: {}", "already exists: {}", &[&item.dst_final.display().to_string()]),
            );
        }
    }
    pass(name, "ok")
}

fn pass(name: &'static str, detail: &str) -> Check {
    Check { name, passed: true, detail: detail.to_string() }
}

fn fail(name: &'static str, detail: &str) -> Check {
    Check { name, passed: false, detail: detail.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn freiplatz_check_rechnet_marge_ein() {
        // Ziel "/" hat real Platz; ein winziger Bedarf muss passen.
        let c = check_free_space(&PathBuf::from("/"), 1000);
        assert!(c.passed, "{}", c.detail);
        // Ein absurd großer Bedarf muss scheitern.
        let c = check_free_space(&PathBuf::from("/"), u64::MAX / 2);
        assert!(!c.passed);
    }

    #[test]
    fn writable_check_auf_tmp() {
        // /tmp ist beschreibbar, hat aber kein steamapps/ — wir legen es an.
        let dir = std::env::temp_dir().join(format!("barge_pc_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("steamapps")).unwrap();
        let c = check_writable(&dir);
        assert!(c.passed, "{}", c.detail);
        std::fs::remove_dir_all(&dir).ok();
    }
}
