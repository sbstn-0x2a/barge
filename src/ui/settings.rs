//! Optionszeile (§8.1): max. Rate und Optionen — oben, mittig im Kopfbereich.

use eframe::egui;

/// Zentrierte Optionszeile: max. Rate, „unbegrenzt“, „Verifizieren“,
/// „Trockenlauf“ — plus ganz rechts der „Bibliothek hinzufügen“-Knopf.
/// Gibt `true` zurück, wenn dieser geklickt wurde.
///
/// Die Optionen werden über einen führenden `add_space` zentriert (Zeilenbreite
/// in Punkten ist zoom-unabhängig), der Knopf rechtsbündig geschoben.
pub fn options_row(
    ui: &mut egui::Ui,
    limit_mbps: &mut u64,
    dry_run: &mut bool,
    verify: &mut bool,
) -> bool {
    let mut add_library = false;
    ui.horizontal(|ui| {
        const CONTENT_W: f32 = 600.0;
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
        ui.checkbox(verify, egui::RichText::new("Verifizieren").size(15.0))
            .on_hover_text("Nach dem Kopieren Dateizahl/Größen/mtimes vergleichen (§7.3)");
        ui.checkbox(dry_run, egui::RichText::new("Trockenlauf").size(15.0))
            .on_hover_text("Alle Prüfungen und der vollständige Plan, ohne eine Datei anzufassen (§8.4)");

        // Knopf exakt rechtsbündig: eigenes rechts-nach-links-Sub-UI im
        // restlichen Platz.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button("Bibliothek hinzufügen")
                .on_hover_text("Einen weiteren Steam-Library-Ordner hinzufügen (§8.3)")
                .clicked()
            {
                add_library = true;
            }
        });
    });
    add_library
}
