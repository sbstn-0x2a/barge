//! Steam-Roots finden, kanonisieren, deduplizieren (§3.2, §3.3).
//!
//! `~/.steam/steam` und `~/.steam/root` sind Symlinks auf
//! `~/.local/share/Steam`; ohne `canonicalize()` erschiene dieselbe Library
//! mehrfach (§3.3). Alle Pfade werden daher vor dem Dedup aufgelöst.

use std::path::{Path, PathBuf};

use super::library::{self, Library};

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Standard-Orte einer Steam-Installation (§3.3), relativ zum Home.
fn candidate_roots() -> Vec<PathBuf> {
    let Some(h) = home() else {
        return Vec::new();
    };
    [
        ".local/share/Steam",
        ".steam/steam",
        ".steam/root",
        ".var/app/com.valvesoftware.Steam/.local/share/Steam",
    ]
    .iter()
    .map(|rel| h.join(rel))
    .collect()
}

/// Fügt einen Pfad kanonisiert und dedupliziert hinzu, sofern er wie eine
/// Library aussieht (enthält `steamapps/`).
fn push_root(out: &mut Vec<PathBuf>, path: &Path) {
    let canon = match std::fs::canonicalize(path) {
        Ok(c) => c,
        Err(_) => return,
    };
    if !canon.join("steamapps").is_dir() {
        return;
    }
    if !out.contains(&canon) {
        out.push(canon);
    }
}

/// Erkennt alle Libraries: von den Standard-Orten ausgehend das
/// `libraryfolders.vdf` einlesen und die dort registrierten Libraries
/// ergänzen. Ergebnis ist kanonisiert und dedupliziert, Reihenfolge stabil
/// (zuerst gefundener Root zuerst).
pub fn discover() -> Vec<Library> {
    let mut roots: Vec<PathBuf> = Vec::new();

    // 1) Standard-Orte.
    for cand in candidate_roots() {
        push_root(&mut roots, &cand);
    }

    // 2) In jedem gefundenen Root registrierte Libraries ergänzen. Wir
    //    iterieren über einen Snapshot, da wir `roots` dabei erweitern.
    let mut idx = 0;
    while idx < roots.len() {
        let steamapps = roots[idx].join("steamapps");
        if let Ok(paths) = library::parse_libraryfolders(&steamapps) {
            for p in paths {
                push_root(&mut roots, &p);
            }
        }
        idx += 1;
    }

    roots.into_iter().map(Library::new).collect()
}

/// Normalisiert einen vom Nutzer gewählten Pfad auf den Library-Root (§3.2).
///
/// - Enthält der Pfad selbst ein `steamapps/` → er *ist* der Root.
/// - Heißt der Pfad `steamapps` und enthält `appmanifest_*.acf` oder `common/`
///   → der Parent ist der Root.
///
/// Gibt den kanonisierten Root zurück oder `None`, wenn der Pfad keine Library
/// ist.
pub fn normalize_root(path: &Path) -> Option<PathBuf> {
    let canon = std::fs::canonicalize(path).ok()?;

    if canon.join("steamapps").is_dir() {
        return Some(canon);
    }

    if canon.file_name().map(|n| n == "steamapps").unwrap_or(false) {
        let looks_like_steamapps = canon.join("common").is_dir()
            || std::fs::read_dir(&canon)
                .map(|rd| {
                    rd.flatten().any(|e| {
                        let n = e.file_name();
                        let n = n.to_string_lossy();
                        n.starts_with("appmanifest_") && n.ends_with(".acf")
                    })
                })
                .unwrap_or(false);
        if looks_like_steamapps {
            return canon.parent().map(Path::to_path_buf);
        }
    }

    None
}
