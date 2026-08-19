//! Fortschrittsanzeige während eines Jobs (§8.2).

use eframe::egui;

use super::job::RunningJob;
use crate::util::human_size;

pub fn view(ui: &mut egui::Ui, r: &mut RunningJob) {
    let content_w = (ui.available_width() * 0.7).clamp(360.0, 900.0);
    ui.add_space(4.0);

    // Alles in einer zentrierten, breitenbegrenzten Spalte (auch das Log).
    ui.vertical_centered(|ui| {
        ui.set_max_width(content_w);

        // Queue-Position.
        let pos = (r.queue_done + 1).min(r.queue_total.max(1));
        ui.label(
            egui::RichText::new(format!("Verschiebe Spiel {} von {}", pos, r.queue_total))
                .size(14.0)
                .weak(),
        );

        // Aktuelles Spiel prominent.
        ui.label(
            egui::RichText::new(if r.current_name.is_empty() {
                "…"
            } else {
                &r.current_name
            })
            .size(20.0)
            .strong(),
        );
        ui.add_space(4.0);

        // Breiter Fortschrittsbalken (füllt die Spalte).
        let frac = if r.bytes_total > 0 {
            (r.bytes_done as f32 / r.bytes_total as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        ui.add_sized(
            [ui.available_width(), 26.0],
            egui::ProgressBar::new(frac)
                .show_percentage()
                .text(format!("{} / {}", human_size(r.bytes_done), human_size(r.bytes_total))),
        );
        ui.add_space(2.0);

        // Rate links, Abbrechen rechts.
        ui.horizontal(|ui| {
            // §6.1: die *gemessene* Rate anzeigen.
            ui.label(egui::RichText::new(format!("gemessen: {:.0} MB/s", r.rate_mbps)).size(14.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if r.cancelling {
                    ui.colored_label(egui::Color32::from_rgb(0xd0, 0x90, 0x30), "wird abgebrochen…");
                } else if ui
                    .add(egui::Button::new("Abbrechen").min_size(egui::vec2(120.0, 28.0)))
                    .clicked()
                {
                    r.request_cancel();
                }
            });
        });

        // Ruhiges Log, eingerahmt und zentriert (füllt die Spalte).
        if !r.log.is_empty() {
            ui.add_space(4.0);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::ScrollArea::vertical()
                    .max_height(84.0)
                    .stick_to_bottom(true)
                    .id_salt("job_log")
                    .show(ui, |ui| {
                        for line in &r.log {
                            ui.monospace(egui::RichText::new(line).size(12.0));
                        }
                    });
            });
        }
    });
}
