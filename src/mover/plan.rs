//! Move-Planung (§4, §7.1): was pro Spiel wohin bewegt bzw. gelöscht wird.
//!
//! Der Plan ist rein beschreibend — er fasst je Komponente Quell- und
//! Zielpfad, `.partial`-Namen und Aktion zusammen. [`super::execute`] führt ihn
//! aus. Später (Stufe 4) hängen hier Vorbedingungen (§5) und der Trockenlauf
//! (§8.4) an.

use std::path::{Path, PathBuf};

use crate::steam::game::{ComponentKind, Game};
use crate::util::dir_real_size;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Verzeichnis nach `.partial` kopieren, dann atomar umbenennen (§7.1).
    MoveDir,
    /// Datei ans Ziel schreiben (Manifest, Workshop-ACF).
    MoveFile,
    /// Nur in der Quelle löschen (shadercache Default, downloading).
    DeleteSource,
}

#[derive(Debug, Clone)]
pub struct PlanItem {
    pub kind: ComponentKind,
    pub action: Action,
    pub src: PathBuf,
    /// Endgültiger Zielpfad (bei `DeleteSource` ungenutzt).
    pub dst_final: PathBuf,
    /// `.partial`-Zwischenpfad bei `MoveDir`.
    pub dst_partial: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct MovePlan {
    pub appid: u32,
    pub name: String,
    pub installdir: String,
    pub source_library: PathBuf,
    pub target_library: PathBuf,
    pub items: Vec<PlanItem>,
    pub bytes_total: u64,
}

impl MovePlan {
    /// Baut den Plan aus einem Spiel und der Ziel-Library. Nur real vorhandene
    /// Komponenten werden aufgenommen. `delete_shadercache` (§4) bestimmt, ob
    /// der Shadercache mitgenommen (dann `MoveDir`) oder in der Quelle gelöscht
    /// wird (Default).
    pub fn new(game: &Game, target_library: &Path, delete_shadercache: bool) -> MovePlan {
        let appid = game.manifest.appid;
        let installdir = game.manifest.installdir.clone();
        let src_apps = game.library.join("steamapps");
        let dst_apps = target_library.join("steamapps");

        let mut items = Vec::new();
        let mut bytes_total = 0u64;

        for comp in &game.components {
            if !comp.present {
                continue;
            }
            let kind = comp.kind;
            let src = kind.path_in(&src_apps, appid, &installdir);
            let dst_final = kind.path_in(&dst_apps, appid, &installdir);

            let action = match kind {
                ComponentKind::Shadercache if delete_shadercache => Action::DeleteSource,
                ComponentKind::Downloading => Action::DeleteSource,
                _ if kind.is_dir() => Action::MoveDir,
                _ => Action::MoveFile,
            };

            let dst_partial = if action == Action::MoveDir {
                Some(partial_path(&dst_final))
            } else {
                None
            };

            if action == Action::MoveDir {
                bytes_total += dir_real_size(&src);
            }

            items.push(PlanItem {
                kind,
                action,
                src,
                dst_final,
                dst_partial,
            });
        }

        MovePlan {
            appid,
            name: game.manifest.name.clone(),
            installdir,
            source_library: game.library.clone(),
            target_library: target_library.to_path_buf(),
            items,
            bytes_total,
        }
    }

    /// Baut den Plan aus einem Journal neu, indem das (noch vorhandene)
    /// Quell-Manifest gelesen wird — für die Wiederaufnahme (§7.2).
    pub fn rebuild_from_source(
        journal: &crate::mover::journal::Journal,
        delete_shadercache: bool,
    ) -> std::io::Result<MovePlan> {
        let src_apps = journal.source_library.join("steamapps");
        let manifest_path = src_apps.join(format!("appmanifest_{}.acf", journal.appid));
        let manifest = crate::steam::manifest::read(&manifest_path)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let game = Game::from_manifest(manifest, &journal.source_library, &src_apps);
        Ok(MovePlan::new(&game, &journal.target_library, delete_shadercache))
    }

    /// Namen der Komponenten, die tatsächlich bewegt werden (für das Journal).
    pub fn moved_component_labels(&self) -> Vec<&'static str> {
        self.items
            .iter()
            .filter(|i| i.action != Action::DeleteSource)
            .map(|i| i.kind.label())
            .collect()
    }
}

/// `.name.partial` neben dem endgültigen Zielpfad (§7.1).
pub(crate) fn partial_path(final_path: &Path) -> PathBuf {
    let name = final_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let parent = final_path.parent().unwrap_or_else(|| Path::new("."));
    parent.join(format!(".{}.partial", name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::steam::manifest::Manifest;

    fn game_with(installdir: &str, library: &Path) -> Game {
        let m = Manifest {
            appid: 1234567,
            name: "Test Game".into(),
            installdir: installdir.into(),
            state_flags: 4,
            size_on_disk: 0,
            last_updated: 0,
        };
        // steamapps existiert nicht -> alle Komponenten present=false; wir
        // setzen sie im Test künstlich, indem wir die Struktur direkt bauen.
        Game::from_manifest(m, library, &library.join("steamapps"))
    }

    #[test]
    fn partial_pfad_korrekt() {
        let p = partial_path(Path::new("/dst/steamapps/common/My Game"));
        assert_eq!(p, PathBuf::from("/dst/steamapps/common/.My Game.partial"));
    }

    #[test]
    fn plan_leer_wenn_nichts_vorhanden() {
        let lib = std::env::temp_dir().join(format!("barge_plan_{}", std::process::id()));
        let g = game_with("My Game", &lib);
        let plan = MovePlan::new(&g, Path::new("/dst"), true);
        assert_eq!(plan.appid, 1234567);
        assert!(plan.items.is_empty()); // nichts existiert physisch
    }
}
