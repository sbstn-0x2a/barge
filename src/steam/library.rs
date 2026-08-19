//! Library-Modell und `libraryfolders.vdf`-Auswertung (§3.4).
//!
//! Eine Library ist ein Wurzelverzeichnis mit einem `steamapps/`-Unterordner.
//! `libraryfolders.vdf` (nur in der Haupt-Library) listet die Pfade aller bei
//! Steam registrierten Libraries. Wir lesen die Datei **nur** — geschrieben
//! wird sie nie (§3.4).

use std::path::{Path, PathBuf};

use super::game::Game;
use super::{manifest, vdf};

#[derive(Debug, Clone)]
pub struct Library {
    /// Kanonischer Library-Root (enthält `steamapps/`).
    pub path: PathBuf,
    /// Optionales Label aus `libraryfolders.vdf` (für die GUI-Dropdowns, §8.1).
    #[allow(dead_code)]
    pub label: Option<String>,
}

impl Library {
    pub fn new(path: PathBuf) -> Library {
        Library { path, label: None }
    }

    pub fn steamapps(&self) -> PathBuf {
        self.path.join("steamapps")
    }

    /// Gesamt- und verfügbarer Platz des Dateisystems der Library (Bytes).
    pub fn disk_space(&self) -> Option<(u64, u64)> {
        crate::util::disk_space(&self.path)
    }

    /// Alle installierten Spiele: `appmanifest_*.acf` unter `steamapps/` scannen
    /// und zu `Game`-Modellen ausbauen (§3.5). AppID `0` (Steam Linux Runtime,
    /// §4.1) wird über die Manifest-Route ohnehin nicht auftauchen, wir filtern
    /// sie aber defensiv heraus.
    ///
    /// Gibt Spiele plus eine Liste nicht lesbarer Manifeste (Pfad + Grund)
    /// zurück, damit die CLI Probleme sichtbar machen kann statt sie zu
    /// verschlucken.
    pub fn games(&self) -> (Vec<Game>, Vec<(PathBuf, String)>) {
        let steamapps = self.steamapps();
        let mut games = Vec::new();
        let mut errors = Vec::new();

        let entries = match std::fs::read_dir(&steamapps) {
            Ok(rd) => rd,
            Err(_) => return (games, errors),
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !(name.starts_with("appmanifest_") && name.ends_with(".acf")) {
                continue;
            }
            match manifest::read(&path) {
                Ok(m) if m.appid == 0 => {} // §4.1: Platzhalter ignorieren
                Ok(m) => games.push(Game::from_manifest(m, &self.path, &steamapps)),
                Err(e) => errors.push((path, e)),
            }
        }

        games.sort_by(|a, b| a.manifest.name.to_lowercase().cmp(&b.manifest.name.to_lowercase()));
        (games, errors)
    }
}

/// Liest die in `libraryfolders.vdf` registrierten Library-Pfade aus dem
/// `steamapps/`-Verzeichnis einer Haupt-Library. Unterstützt das aktuelle
/// verschachtelte Format (`"0" { "path" "..." }`) und das alte flache Format
/// (`"1" "/pfad"`) (§3.4).
pub fn parse_libraryfolders(steamapps: &Path) -> Result<Vec<PathBuf>, String> {
    let file = steamapps.join("libraryfolders.vdf");
    let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {}", file.display(), e))?;
    let root = vdf::parse(&text)?;
    let lf = root
        .get("libraryfolders")
        .ok_or("kein libraryfolders-Block")?;

    let mut paths = Vec::new();
    for (_key, value) in lf.entries() {
        match value {
            // Neues Format: Objekt mit "path".
            vdf::Value::Obj(_) => {
                if let Some(p) = value.str("path") {
                    paths.push(PathBuf::from(p));
                }
            }
            // Altes flaches Format: der Wert ist direkt der Pfad.
            vdf::Value::Str(p) => paths.push(PathBuf::from(p)),
        }
    }
    Ok(paths)
}
