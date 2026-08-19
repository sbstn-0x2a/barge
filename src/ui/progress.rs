//! Fortschrittsanzeige während eines Jobs (§8.2).

use eframe::egui;

use super::job::RunningJob;
use crate::util::human_size;

pub fn view(ui: &mut egui::Ui, r: &mut RunningJob) {
    ui.horizontal(|ui| {
        ui.spinner();
        ui.strong(format!(
            "Verschiebe: {}  ({}/{})",
            if r.current_name.is_empty() { "…" } else { &r.current_name },
            (r.queue_done + 1).min(r.queue_total.max(1)),
            r.queue_total
        ));
    });

    let frac = if r.bytes_total > 0 {
        (r.bytes_done as f32 / r.bytes_total as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    ui.add(
        egui::ProgressBar::new(frac)
            .show_percentage()
            .text(format!("{} / {}", human_size(r.bytes_done), human_size(r.bytes_total))),
    );

    ui.horizontal(|ui| {
        // §6.1: die *gemessene* Rate anzeigen.
        ui.label(format!("gemessen: {:.0} MB/s", r.rate_mbps));
        if r.cancelling {
            ui.colored_label(egui::Color32::from_rgb(0xd0, 0x90, 0x30), "wird abgebrochen…");
        } else if ui.button("Abbrechen").clicked() {
            r.request_cancel();
        }
    });

    if !r.log.is_empty() {
        egui::ScrollArea::vertical()
            .max_height(90.0)
            .stick_to_bottom(true)
            .id_salt("job_log")
            .show(ui, |ui| {
                for line in &r.log {
                    ui.monospace(line);
                }
            });
    }
}
