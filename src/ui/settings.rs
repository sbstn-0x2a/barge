//! Einstellungsleiste (§8.1): Auswahl-Zusammenfassung, max. Rate, Optionen.

use eframe::egui;

use crate::util::human_size;

pub fn bar(
    ui: &mut egui::Ui,
    limit_mbps: &mut u64,
    delete_shadercache: &mut bool,
    dry_run: &mut bool,
    sel_count: usize,
    sel_bytes: u64,
) {
    ui.horizontal(|ui| {
        ui.strong(format!("Auswahl: {} Spiel(e)", sel_count));
        ui.label(format!("· {}", human_size(sel_bytes)));
    });

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
