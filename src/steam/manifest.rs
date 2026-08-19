//! Lesen von `appmanifest_<AppID>.acf` (§3.5).
//!
//! Das ACF ist Valve-KeyValues mit einer Wurzel `"AppState"`. Wir extrahieren
//! die für das Verschieben relevanten Skalarfelder. Anzeigenamen kommen
//! komplett aus dem ACF — keine externe DB nötig, das Tool bleibt offline (§3.5).

use std::path::Path;

use super::vdf;

/// Installationszustand laut `StateFlags`. `4` bedeutet vollständig installiert;
/// jeder andere Wert (Update, Download, Validierung) muss den Move blockieren
/// (§3.5, §5).
pub const STATE_FULLY_INSTALLED: u32 = 4;

#[derive(Debug, Clone)]
pub struct Manifest {
    pub appid: u32,
    pub name: String,
    pub installdir: String,
    pub state_flags: u32,
    /// Größe laut ACF — nur als grobe Schätzung nutzbar, laut §5.4 kann der
    /// Wert massiv daneben liegen. Nie für Platzprüfungen verwenden.
    /// (Wird ab Stufe 4 für die Trockenlauf-Anzeige herangezogen.)
    #[allow(dead_code)]
    pub size_on_disk: u64,
    /// Unix-Timestamp der letzten Aktualisierung (für spätere Sortierung, §11.6).
    #[allow(dead_code)]
    pub last_updated: u64,
}

impl Manifest {
    /// Vollständig installiert und damit für einen Move zugelassen (§5).
    pub fn is_fully_installed(&self) -> bool {
        self.state_flags == STATE_FULLY_INSTALLED
    }

    /// Grobe, offline-taugliche Heuristik, ob dieser Eintrag ein Steam-Tool
    /// bzw. eine Runtime ist (Proton, Steam Linux Runtime, Redistributables)
    /// statt eines echten Spiels. Das ACF enthält kein zuverlässiges Typfeld;
    /// der App-Typ steht nur in der binären `appinfo.vdf`. Solange die nicht
    /// geparst wird, entscheidet der Anzeigename.
    pub fn is_tool(&self) -> bool {
        let n = self.name.to_lowercase();
        n.starts_with("proton")
            || n.starts_with("steam linux runtime")
            || n.starts_with("steamworks common")
            || n.contains("easyanticheat runtime")
    }

    /// Menschable Begründung, falls das Spiel *nicht* verschoben werden darf.
    pub fn blocked_reason(&self) -> Option<String> {
        if self.is_fully_installed() {
            None
        } else {
            Some(format!(
                "nicht vollständig installiert (StateFlags={})",
                self.state_flags
            ))
        }
    }
}

/// Liest und parst ein appmanifest. Fehlende Pflichtfelder (`appid`,
/// `installdir`) sind ein Fehler — ohne sie ist der Eintrag unbrauchbar.
pub fn read(path: &Path) -> Result<Manifest, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("{}: {}", path.display(), e))?;
    let root = vdf::parse(&text).map_err(|e| format!("{}: {}", path.display(), e))?;
    let app = root
        .get("AppState")
        .ok_or_else(|| format!("{}: kein AppState-Block", path.display()))?;

    let appid = app
        .str("appid")
        .and_then(|s| s.trim().parse::<u32>().ok())
        .ok_or_else(|| format!("{}: appid fehlt oder ungültig", path.display()))?;

    let installdir = app
        .str("installdir")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{}: installdir fehlt", path.display()))?
        .to_string();

    Ok(Manifest {
        appid,
        name: app.str("name").unwrap_or("(ohne Namen)").to_string(),
        installdir,
        state_flags: app.str("StateFlags").and_then(|s| s.trim().parse().ok()).unwrap_or(0),
        size_on_disk: app.str("SizeOnDisk").and_then(|s| s.trim().parse().ok()).unwrap_or(0),
        last_updated: app.str("LastUpdated").and_then(|s| s.trim().parse().ok()).unwrap_or(0),
    })
}
