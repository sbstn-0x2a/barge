//! Einstellungsleiste (§8.1): Auswahl-Zusammenfassung, max. Rate, Optionen.

use eframe::egui;

use super::SelectionSummary;
use crate::util::human_size;

pub fn bar(
    ui: &mut egui::Ui,
    limit_mbps: &mut u64,
    delete_shadercache: &mut bool,
    dry_run: &mut bool,
    sel: &SelectionSummary,
) {
    ui.horizontal(|ui| {
        ui.strong(format!("Auswahl: {} Spiel(e)", sel.count));
        ui.label(format!("· {}", human_size(sel.bytes)));
    });

    // Aggregat der enthaltenen Zusatzkomponenten über die Auswahl (§4).
    if sel.count > 0 {
        let mut parts: Vec<String> = Vec::new();
        if sel.compatdata > 0 {
            parts.push(format!("compatdata ×{} (Savegames)", sel.compatdata));
        }
        if sel.workshop > 0 {
            parts.push(format!("workshop ×{} (Mods)", sel.workshop));
        }
        if sel.shadercache > 0 {
            let what = if *delete_shadercache { "werden gelöscht" } else { "werden mitgenommen" };
            parts.push(format!("shadercache ×{} ({})", sel.shadercache, what));
        }
        let text = if parts.is_empty() {
            "enthält: nur Spieldaten".to_string()
        } else {
            format!("enthält: {}", parts.join(" · "))
        };
        ui.label(egui::RichText::new(text).weak());
    }

    ui.horizontal(|ui| {
        // §6.1: Das Label heißt bewusst „max. Rate“, nicht „Rate“.
        ui.label("max. Rate:");
        let mut unlimited = *limit_mbps == 0;
        let mut val = if *limit_mbps == 0 { 250 } else { *limit_mbps };
        ui.add_enabled(!unlimited, egui::Slider::new(&mut val, 50..=2000).suffix(" MB/s"));
        ui.checkbox(&mut unlimited, "unbegrenzt");
        *limit_mbps = if unlimited { 0 } else { val };
        if unlimited {
            ui.colored_label(egui::Color32::LIGHT_RED, "⚠ ohne Drossel");
        }
    });

    ui.horizontal(|ui| {
        ui.checkbox(delete_shadercache, "Shadercache löschen");
        ui.checkbox(dry_run, "Trockenlauf")
            .on_hover_text("Alle Prüfungen und der vollständige Plan, ohne eine Datei anzufassen (§8.4)");
    });
}
