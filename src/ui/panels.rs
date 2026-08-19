//! Zwei-Panel-Ansicht (§8.1): links Quelle mit Auswahl, rechts Ziel.

use std::collections::HashSet;

use eframe::egui;

use super::{GameRow, LibraryView};
use crate::util::human_size;

/// Kleine, dezente Komponenten-Marker rechts neben der Größe (§4).
fn component_tags(ui: &mut egui::Ui, row: &GameRow) {
    let tag = |ui: &mut egui::Ui, text: &str, hover: &str| {
        ui.weak(text).on_hover_text(hover);
    };
    if row.has_shadercache {
        tag(ui, "shader", "shadercache vorhanden (Default: löschen)");
    }
    if row.has_workshop {
        tag(ui, "workshop", "Workshop-Mods vorhanden");
    }
    if row.has_compatdata {
        tag(ui, "compat", "compatdata vorhanden (Savegames + Proton-Prefix)");
    }
}

fn library_combo(ui: &mut egui::Ui, id: &str, libraries: &[LibraryView], idx: &mut usize) {
    let current = libraries.get(*idx).map(|l| l.label.as_str()).unwrap_or("—");
    egui::ComboBox::from_id_salt(id)
        .width(ui.available_width() - 8.0)
        .selected_text(current)
        .show_ui(ui, |ui| {
            for (i, lib) in libraries.iter().enumerate() {
                ui.selectable_value(idx, i, lib.label.as_str());
            }
        });
}

fn disk_line(ui: &mut egui::Ui, lib: &LibraryView) {
    match lib.disk {
        Some((total, avail)) if total > 0 => {
            let used = total.saturating_sub(avail);
            let pct = (used as f64 / total as f64 * 100.0).round() as u32;
            ui.label(format!(
                "{} / {} belegt ({} %) · {} frei",
                human_size(used),
                human_size(total),
                pct,
                human_size(avail)
            ));
        }
        _ => {
            ui.label("Speicherplatz unbekannt");
        }
    }
}

/// Linkes Panel: Library-Auswahl + Spieleliste mit Auswahl-Checkboxen.
pub fn source_panel(
    ui: &mut egui::Ui,
    libraries: &[LibraryView],
    source_idx: &mut usize,
    selected: &mut HashSet<u32>,
) {
    ui.heading("Quelle");
    if libraries.is_empty() {
        ui.label("Keine Steam-Libraries gefunden.");
        return;
    }
    library_combo(ui, "source_lib", libraries, source_idx);
    let lib = &libraries[*source_idx];
    disk_line(ui, lib);

    ui.horizontal(|ui| {
        if ui.small_button("Alle").clicked() {
            for r in lib.games.iter().filter(|r| r.blocked_reason.is_none()) {
                selected.insert(r.appid);
            }
        }
        if ui.small_button("Keine").clicked() {
            for r in &lib.games {
                selected.remove(&r.appid);
            }
        }
    });
    ui.separator();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for row in &lib.games {
                ui.horizontal(|ui| {
                    let enabled = row.blocked_reason.is_none();
                    let mut checked = selected.contains(&row.appid);
                    if ui
                        .add_enabled(enabled, egui::Checkbox::new(&mut checked, ""))
                        .changed()
                    {
                        if checked {
                            selected.insert(row.appid);
                        } else {
                            selected.remove(&row.appid);
                        }
                    }

                    let name = if row.is_tool {
                        format!("🔧 {}", row.name)
                    } else {
                        row.name.clone()
                    };
                    ui.add(egui::Label::new(name).truncate());

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        match &row.blocked_reason {
                            Some(reason) => {
                                ui.weak(format!("⚠ {}", reason));
                            }
                            None => {
                                ui.monospace(human_size(row.size));
                                // Marker links neben der Größe (§4).
                                component_tags(ui, row);
                            }
                        }
                    });
                });
            }
        });
}

/// Rechtes Panel: Ziel-Library + (schreibgeschützte) Spieleliste.
pub fn target_panel(
    ui: &mut egui::Ui,
    libraries: &[LibraryView],
    target_idx: &mut usize,
    source_idx: usize,
) {
    ui.heading("Ziel");
    if libraries.is_empty() {
        return;
    }
    library_combo(ui, "target_lib", libraries, target_idx);
    let lib = &libraries[*target_idx];
    disk_line(ui, lib);
    if *target_idx == source_idx {
        ui.colored_label(egui::Color32::LIGHT_RED, "= Quelle (bitte anderes Ziel wählen)");
    }
    ui.separator();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .id_salt("target_scroll")
        .show(ui, |ui| {
            for row in &lib.games {
                ui.horizontal(|ui| {
                    let name = if row.is_tool {
                        format!("🔧 {}", row.name)
                    } else {
                        row.name.clone()
                    };
                    ui.add(egui::Label::new(name).truncate());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.monospace(human_size(row.size));
                    });
                });
            }
        });
}
