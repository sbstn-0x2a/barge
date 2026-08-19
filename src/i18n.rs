//! Minimale Zweisprachigkeit (DE/EN) ohne externe Crate.
//!
//! Übersetzungen stehen direkt am Aufrufort: `tr("Deutsch", "English")` gibt je
//! nach aktueller Sprache den passenden String zurück. Für Formatstrings (die
//! Rust' `format!` nur als Literal akzeptiert) füllt `trf` `{}`-Platzhalter zur
//! Laufzeit.

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    De,
    En,
}

static CURRENT: AtomicU8 = AtomicU8::new(0); // 0 = De, 1 = En

pub fn set_lang(lang: Lang) {
    CURRENT.store(if lang == Lang::En { 1 } else { 0 }, Ordering::Relaxed);
}

pub fn lang() -> Lang {
    if CURRENT.load(Ordering::Relaxed) == 1 {
        Lang::En
    } else {
        Lang::De
    }
}

/// Sprachcode ("de"/"en") für die Persistenz.
pub fn code() -> &'static str {
    match lang() {
        Lang::De => "de",
        Lang::En => "en",
    }
}

/// Setzt die Sprache aus einem Code; unbekannt/leer → Auto-Erkennung per Locale.
pub fn set_from_code(code: &str) {
    match code {
        "de" => set_lang(Lang::De),
        "en" => set_lang(Lang::En),
        _ => set_lang(detect_from_env()),
    }
}

/// Erkennt die Sprache aus den Locale-Umgebungsvariablen (Default Englisch).
pub fn detect_from_env() -> Lang {
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(var) {
            let v = v.to_ascii_lowercase();
            if v.starts_with("de") {
                return Lang::De;
            }
            if !v.is_empty() && v != "c" && v != "posix" {
                return Lang::En;
            }
        }
    }
    Lang::En
}

/// Wählt den String der aktuellen Sprache.
pub fn tr(de: &'static str, en: &'static str) -> &'static str {
    match lang() {
        Lang::De => de,
        Lang::En => en,
    }
}

/// Wie [`tr`], füllt aber `{}`-Platzhalter der Reihe nach mit `args`.
pub fn trf(de: &str, en: &str, args: &[&str]) -> String {
    let tmpl = if lang() == Lang::En { en } else { de };
    let mut parts = tmpl.split("{}");
    let mut out = String::new();
    if let Some(first) = parts.next() {
        out.push_str(first);
    }
    for (i, part) in parts.enumerate() {
        out.push_str(args.get(i).copied().unwrap_or(""));
        out.push_str(part);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tr_und_trf() {
        set_lang(Lang::En);
        assert_eq!(tr("Hallo", "Hello"), "Hello");
        assert_eq!(trf("{} Spiele", "{} games", &["3"]), "3 games");
        set_lang(Lang::De);
        assert_eq!(tr("Hallo", "Hello"), "Hallo");
        assert_eq!(trf("{} von {}", "{} of {}", &["1", "2"]), "1 von 2");
    }
}
