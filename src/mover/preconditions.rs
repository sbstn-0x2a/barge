//! Sicherheits-Vorbedingungen vor einem Move (§5).
//!
//! Alle Prüfungen liefern ein strukturiertes Ergebnis, damit sie sowohl den
//! echten Move blockieren als auch im Trockenlauf (§8.4) angezeigt werden
//! können. Keine Prüfung fasst eine Datei an.

use std::path::Path;

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
    checks.push(if discovery::steam_running() {
        fail("Steam läuft nicht", "Steam ist aktiv — bitte beenden (kein „trotzdem fortfahren“)")
    } else {
        pass("Steam läuft nicht", "ok")
    });

    // §5.6 — Spiel-Zustand sauber (StateFlags == 4).
    checks.push(match game.manifest.blocked_reason() {
        None => pass("Spiel vollständig installiert", "StateFlags = 4"),
        Some(reason) => fail("Spiel vollständig installiert", &reason),
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
    if registered {
        pass("Ziel-Library registriert", "in libraryfolders.vdf gefunden")
    } else {
        fail(
            "Ziel-Library registriert",
            "nicht in libraryfolders.vdf — in Steam unter Einstellungen → \
             Speicherplatz zuerst hinzufügen (barge schreibt die Datei nicht)",
        )
    }
}

fn check_writable(target: &Path) -> Check {
    // Effektive Rechte testen, indem wir wirklich eine Datei anlegen (§5.3:
    // nicht über root/uid raten). steamapps/ existiert bei einer Library immer.
    let probe = target
        .join("steamapps")
        .join(format!(".barge_write_test_{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            pass("Ziel beschreibbar", "ok")
        }
        Err(e) => fail("Ziel beschreibbar", &format!("nicht beschreibbar: {}", e)),
    }
}

fn check_free_space(target: &Path, bytes_total: u64) -> Check {
    let needed = (bytes_total as f64 * SPACE_MARGIN) as u64;
    match disk_space(target) {
        Some((_total, avail)) => {
            if avail >= needed {
                pass(
                    "Genug Freiplatz",
                    &format!(
                        "{} frei ≥ {} nötig (inkl. 5 %)",
                        crate::util::human_size(avail),
                        crate::util::human_size(needed)
                    ),
                )
            } else {
                fail(
                    "Genug Freiplatz",
                    &format!(
                        "{} frei < {} nötig (inkl. 5 %) — es fehlen {}",
                        crate::util::human_size(avail),
                        crate::util::human_size(needed),
                        crate::util::human_size(needed - avail)
                    ),
                )
            }
        }
        None => fail("Genug Freiplatz", "statvfs auf das Ziel fehlgeschlagen"),
    }
}

fn check_no_conflict(plan: &MovePlan) -> Check {
    for item in &plan.items {
        if item.action != Action::DeleteSource && item.dst_final.exists() {
            return fail(
                "Kein Zielkonflikt",
                &format!("existiert bereits: {}", item.dst_final.display()),
            );
        }
    }
    pass("Kein Zielkonflikt", "ok")
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
