//! Einstellungsleiste (§8.1): Auswahl-Zusammenfassung, max. Rate, Optionen.

use eframe::egui;

use super::SelectionSummary;
use crate::util::human_size;

pub fn bar(ui: &mut egui::Ui, limit_mbps: &mut u64, dry_run: &mut bool, sel: &SelectionSummary) {
    // Zeile 1: Auswahl-Zusammenfassung, prominent.
    ui.label(
        egui::RichText::new(format!(
            "Auswahl: {} Spiel(e) · {}",
            sel.count,
            human_size(sel.bytes)
        ))
        .size(18.0)
        .strong(),
    );

    // Zeile 2: max. Rate + Optionen.
    ui.horizontal(|ui| {
        // §6.1: Das Label heißt bewusst „max. Rate“, nicht „Rate“.
        ui.label(egui::RichText::new("max. Rate:").size(15.0));
        let mut unlimited = *limit_mbps == 0;
        let mut val = if *limit_mbps == 0 { 250 } else { *limit_mbps };
        ui.add_enabled(
            !unlimited,
            egui::Slider::new(&mut val, 50..=2000).suffix(" MB/s"),
        );
        ui.checkbox(&mut unlimited, "unbegrenzt");
        *limit_mbps = if unlimited { 0 } else { val };
        if unlimited {
            ui.colored_label(egui::Color32::LIGHT_RED, "⚠ ohne Drossel");
        }
        ui.separator();
        ui.checkbox(dry_run, egui::RichText::new("Trockenlauf").size(15.0))
            .on_hover_text("Alle Prüfungen und der vollständige Plan, ohne eine Datei anzufassen (§8.4)");
    });
}
