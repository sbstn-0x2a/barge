//! Das `Game`-Modell: verbindet ein Manifest mit den physischen Komponenten
//! einer Library (§4). Enthält alles, was ein späterer Move-Job braucht, ist
//! aber selbst rein beschreibend.

use std::path::{Path, PathBuf};

use super::manifest::Manifest;
use crate::util::dir_real_size;

/// Eine bewegliche Komponente eines Spiels (§4). `kind` benennt sie,
/// `path` ist der absolute Quellpfad innerhalb der Library.
#[derive(Debug, Clone)]
pub struct Component {
    pub kind: ComponentKind,
    pub path: PathBuf,
    pub present: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComponentKind {
    /// `steamapps/common/<InstallDir>` — die Spieldaten.
    Common,
    /// `steamapps/appmanifest_<AppID>.acf` — ohne das gilt das Spiel als weg.
    Manifest,
    /// `steamapps/compatdata/<AppID>` — Savegames + Registry (irreversibel).
    Compatdata,
    /// `steamapps/workshop/content/<AppID>` — Mods.
    WorkshopContent,
    /// `steamapps/workshop/appworkshop_<AppID>.acf` — Workshop-Manifest.
    WorkshopManifest,
    /// `steamapps/shadercache/<AppID>` — Wegwerf-Cache (Default: löschen).
    Shadercache,
    /// `steamapps/downloading/<AppID>` — Reste (immer löschen).
    Downloading,
}

impl ComponentKind {
    pub fn label(self) -> &'static str {
        match self {
            ComponentKind::Common => "common",
            ComponentKind::Manifest => "manifest",
            ComponentKind::Compatdata => "compatdata",
            ComponentKind::WorkshopContent => "workshop",
            ComponentKind::WorkshopManifest => "workshop-acf",
            ComponentKind::Shadercache => "shadercache",
            ComponentKind::Downloading => "downloading",
        }
    }

    /// Gehört die Komponente zu den Daten, die real ans Ziel verschoben werden?
    /// (`shadercache` und `downloading` werden per Default gelöscht, nicht bewegt.)
    pub fn is_moved(self) -> bool {
        matches!(
            self,
            ComponentKind::Common
                | ComponentKind::Manifest
                | ComponentKind::Compatdata
                | ComponentKind::WorkshopContent
                | ComponentKind::WorkshopManifest
        )
    }
}

#[derive(Debug, Clone)]
pub struct Game {
    pub manifest: Manifest,
    /// Library-Root, in dem das Spiel liegt (nicht das `steamapps/`).
    /// Quelle für die Move-Planung ab Stufe 4.
    #[allow(dead_code)]
    pub library: PathBuf,
    pub components: Vec<Component>,
}

impl Game {
    /// Baut das Spiel-Modell aus einem Manifest und dem `steamapps/`-Pfad der
    /// Library. Prüft für jede Komponente aus §4, ob sie physisch existiert.
    pub fn from_manifest(manifest: Manifest, library: &Path, steamapps: &Path) -> Game {
        let id = manifest.appid;
        let defs: [(ComponentKind, PathBuf); 7] = [
            (
                ComponentKind::Common,
                steamapps.join("common").join(&manifest.installdir),
            ),
            (
                ComponentKind::Manifest,
                steamapps.join(format!("appmanifest_{}.acf", id)),
            ),
            (
                ComponentKind::Compatdata,
                steamapps.join("compatdata").join(id.to_string()),
            ),
            (
                ComponentKind::WorkshopContent,
                steamapps.join("workshop").join("content").join(id.to_string()),
            ),
            (
                ComponentKind::WorkshopManifest,
                steamapps.join("workshop").join(format!("appworkshop_{}.acf", id)),
            ),
            (
                ComponentKind::Shadercache,
                steamapps.join("shadercache").join(id.to_string()),
            ),
            (
                ComponentKind::Downloading,
                steamapps.join("downloading").join(id.to_string()),
            ),
        ];

        let components = defs
            .into_iter()
            .map(|(kind, path)| {
                let present = path.exists();
                Component { kind, path, present }
            })
            .collect();

        Game {
            manifest,
            library: library.to_path_buf(),
            components,
        }
    }

    /// Reale On-Disk-Größe aller Komponenten, die tatsächlich verschoben werden
    /// (§4, §5.4). Läuft den Baum ab — bei großen Spielen entsprechend teuer.
    pub fn moved_size(&self) -> u64 {
        self.components
            .iter()
            .filter(|c| c.present && c.kind.is_moved())
            .map(|c| dir_real_size(&c.path))
            .sum()
    }
}
