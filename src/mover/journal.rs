//! Transaktionsjournal und Recovery (§7.2).
//!
//! Für jeden Move wird eine JSON-Datei unter
//! `$XDG_STATE_HOME/barge/jobs/<id>.json` geführt und **nach jedem
//! Zustandswechsel mit `fsync` verankert** (Schreiben in `.tmp`, `fsync`,
//! atomares `rename`, `fsync` des Verzeichnisses). Ein Absturz mitten im Move
//! hinterlässt dadurch ein konsistentes Journal, das beim nächsten Start
//! erkannt wird.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Grobzustand eines Jobs (§7.1). `Committed` bedeutet: der Punkt ohne Rückkehr
/// ist überschritten, das Spiel liegt vollständig am Ziel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum JobState {
    Started,
    Copying,
    Committed,
    Failed,
}

/// Zustand einer einzelnen Komponente (common, compatdata, ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ComponentState {
    Pending,
    InProgress,
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Journal {
    pub id: String,
    pub appid: u32,
    pub name: String,
    pub installdir: String,
    pub source_library: PathBuf,
    pub target_library: PathBuf,
    pub state: JobState,
    pub components: BTreeMap<String, ComponentState>,
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// Unix-Epoch-Sekunden. Bewusst kein ISO-String, um dependency-frei bei der
    /// Zeit zu bleiben.
    pub started_at: u64,

    /// Ablageort dieser Journal-Datei. Nicht serialisiert — wird beim Laden
    /// bzw. Anlegen gesetzt.
    #[serde(skip)]
    pub path: PathBuf,
}

impl Journal {
    /// Verzeichnis der Job-Journale: `$XDG_STATE_HOME/barge/jobs`, sonst
    /// `~/.local/state/barge/jobs`.
    pub fn jobs_dir() -> PathBuf {
        let base = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| {
                let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
                home.join(".local/state")
            });
        base.join("barge/jobs")
    }

    /// Legt ein neues Journal im Zustand `Started` an und schreibt es
    /// (fsync-verankert) auf die Platte.
    pub fn create(
        appid: u32,
        name: &str,
        installdir: &str,
        source_library: &Path,
        target_library: &Path,
        components: &[&str],
        bytes_total: u64,
    ) -> io::Result<Journal> {
        let dir = Self::jobs_dir();
        fs::create_dir_all(&dir)?;
        let id = random_id()?;
        let mut comps = BTreeMap::new();
        for c in components {
            comps.insert((*c).to_string(), ComponentState::Pending);
        }
        let j = Journal {
            id: id.clone(),
            appid,
            name: name.to_string(),
            installdir: installdir.to_string(),
            source_library: source_library.to_path_buf(),
            target_library: target_library.to_path_buf(),
            state: JobState::Started,
            components: comps,
            bytes_done: 0,
            bytes_total,
            started_at: now_epoch(),
            path: dir.join(format!("{}.json", id)),
        };
        j.persist()?;
        Ok(j)
    }

    /// Lädt ein Journal aus einer Datei.
    pub fn load(path: &Path) -> io::Result<Journal> {
        let text = fs::read_to_string(path)?;
        let mut j: Journal = serde_json::from_str(&text)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        j.path = path.to_path_buf();
        Ok(j)
    }

    /// Alle vorhandenen (= unvollendeten) Journale. Ein erfolgreicher Move
    /// löscht sein Journal, also ist jede gefundene Datei ein offener Job.
    pub fn scan_incomplete() -> Vec<Journal> {
        let dir = Self::jobs_dir();
        let mut jobs = Vec::new();
        if let Ok(rd) = fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().map(|x| x == "json").unwrap_or(false) {
                    if let Ok(j) = Self::load(&p) {
                        jobs.push(j);
                    }
                }
            }
        }
        jobs.sort_by_key(|j| j.started_at);
        jobs
    }

    pub fn set_state(&mut self, state: JobState) -> io::Result<()> {
        self.state = state;
        self.persist()
    }

    pub fn set_component(&mut self, name: &str, state: ComponentState) -> io::Result<()> {
        self.components.insert(name.to_string(), state);
        self.persist()
    }

    /// Aktualisiert `bytes_done` **ohne** fsync — für häufige Fortschritts-
    /// Updates; verankert wird nur bei Zustandswechseln (§7.2).
    pub fn set_bytes_done(&mut self, bytes: u64) {
        self.bytes_done = bytes;
    }

    /// Löscht die Journal-Datei (letzter Schritt eines erfolgreichen Moves).
    pub fn remove(&self) -> io::Result<()> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Schreibt das Journal atomar und fsync-verankert (§7.2).
    fn persist(&self) -> io::Result<()> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let tmp = self.path.with_extension("json.tmp");
        {
            let mut f = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&tmp)?;
            f.write_all(json.as_bytes())?;
            f.sync_all()?; // Inhalt der Temp-Datei sichern
        }
        fs::rename(&tmp, &self.path)?; // atomarer Austausch
        if let Some(parent) = self.path.parent() {
            // Verzeichnis-fsync, damit rename dauerhaft ist.
            let _ = File::open(parent).and_then(|d| d.sync_all());
        }
        Ok(())
    }
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 16 zufällige Bytes aus `/dev/urandom` als Hex — genügt als Datei-ID, ohne
/// den `uuid`-Crate zu ziehen.
fn random_id() -> io::Result<String> {
    use std::io::Read;
    let mut buf = [0u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut buf)?;
    let mut s = String::with_capacity(32);
    for b in buf {
        s.push_str(&format!("{:02x}", b));
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_state_home(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("barge_state_{}_{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn create_load_und_scan() {
        let home = tmp_state_home("journal");
        std::env::set_var("XDG_STATE_HOME", &home);

        let src = PathBuf::from("/src/lib");
        let dst = PathBuf::from("/dst/lib");
        let mut j = Journal::create(
            440,
            "Team Fortress 2",
            "Team Fortress 2",
            &src,
            &dst,
            &["common", "compatdata"],
            1000,
        )
        .unwrap();

        assert_eq!(j.state, JobState::Started);
        assert_eq!(j.components["common"], ComponentState::Pending);

        j.set_component("common", ComponentState::Done).unwrap();
        j.set_state(JobState::Copying).unwrap();

        // Neu einlesen -> Zustände persistiert.
        let loaded = Journal::load(&j.path).unwrap();
        assert_eq!(loaded.state, JobState::Copying);
        assert_eq!(loaded.components["common"], ComponentState::Done);
        assert_eq!(loaded.appid, 440);

        // Scan findet den offenen Job.
        let open = Journal::scan_incomplete();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].id, j.id);

        // Nach remove ist nichts mehr offen.
        j.remove().unwrap();
        assert!(Journal::scan_incomplete().is_empty());

        let _ = fs::remove_dir_all(&home);
    }
}
