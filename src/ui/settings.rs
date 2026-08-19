//! Optionszeile (§8.1): max. Rate und Optionen — oben, mittig im Kopfbereich.

use eframe::egui;

/// Zentrierte Optionszeile: max. Rate, „unbegrenzt“, „Trockenlauf“.
///
/// Zentriert über einen führenden `add_space` (die Zeilenbreite ist in Punkten
/// konstant, unabhängig vom Zoom, daher genügt eine feste Schätzbreite).
pub fn options_row(ui: &mut egui::Ui, limit_mbps: &mut u64, dry_run: &mut bool) {
    ui.horizontal(|ui| {
        const CONTENT_W: f32 = 480.0;
        let space = ((ui.available_width() - CONTENT_W) * 0.5).max(0.0);
        ui.add_space(space);

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
            ui.colored_label(egui::Color32::LIGHT_RED, "(!) ohne Drossel");
        }
        ui.separator();
        ui.checkbox(dry_run, egui::RichText::new("Trockenlauf").size(15.0))
            .on_hover_text("Alle Prüfungen und der vollständige Plan, ohne eine Datei anzufassen (§8.4)");
    });
}
