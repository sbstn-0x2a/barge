//! Persistente UI-Konfiguration unter `$XDG_CONFIG_HOME/barge/config.json`.
//! Aktuell nur der Schrift-/Zoom-Faktor (für hochauflösende Displays).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// egui-Zoomfaktor (1.0 = 100 %).
    pub zoom_factor: f32,
    /// Zuletzt genutzte Fenster-Innengröße in Punkten.
    pub window_w: f32,
    pub window_h: f32,
    /// Manuell hinzugefügte Library-Pfade (§8.3); werden beim Laden zusätzlich
    /// zu den erkannten Libraries berücksichtigt.
    pub extra_libraries: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            zoom_factor: 1.0,
            window_w: 980.0,
            window_h: 660.0,
            extra_libraries: Vec::new(),
        }
    }
}

impl Config {
    fn path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("barge").join("config.json"))
    }

    pub fn load() -> Config {
        let Some(p) = Self::path() else {
            return Config::default();
        };
        std::fs::read_to_string(&p)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .map(|mut c: Config| {
                c.clamp();
                c
            })
            .unwrap_or_default()
    }

    /// Best-effort-Speichern; Fehler werden bewusst ignoriert.
    pub fn save(&self) {
        let Some(p) = Self::path() else {
            return;
        };
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&p, s);
        }
    }

    pub fn clamp(&mut self) {
        if !self.zoom_factor.is_finite() {
            self.zoom_factor = 1.0;
        }
        self.zoom_factor = self.zoom_factor.clamp(0.7, 2.5);
        if !self.window_w.is_finite() || self.window_w < 400.0 {
            self.window_w = 980.0;
        }
        if !self.window_h.is_finite() || self.window_h < 300.0 {
            self.window_h = 660.0;
        }
        self.window_w = self.window_w.min(20000.0);
        self.window_h = self.window_h.min(20000.0);
    }
}
