//! Zwei-Panel-Ansicht (§8.1): links Quelle als Tabelle mit Auswahl und
//! Komponenten-Spalten, rechts Ziel.

use std::collections::{HashMap, HashSet};

use eframe::egui;
use egui_extras::{Column, TableBuilder};

use super::{GameRow, LibraryView};
use crate::i18n::{tr, trf};
use crate::mover::plan::ComponentChoice;
use crate::util::human_size;

/// Welche Komponenten-Spalte per Kopfklick umgeschaltet werden soll.
#[derive(Clone, Copy)]
enum ToggleCol {
    Compat,
    Workshop,
    Shader,
}

/// Feste Höhe der Toolbar-Zeile über der Tabelle, damit Quelle und Ziel exakt
/// auf gleicher Höhe beginnen (per `set_height`, unabhängig vom Inhalt).
const TOOLBAR_H: f32 = 28.0;
/// Höhe der (fixierten) Tabellen-Kopfzeile.
const HEADER_H: f32 = 24.0;

/// Library-Dropdown. `exclude` blendet eine Bibliothek aus (die auf der anderen
/// Seite gewählte), damit Quelle und Ziel nicht identisch werden können.
fn library_combo(
    ui: &mut egui::Ui,
    id: &str,
    libraries: &[LibraryView],
    idx: &mut usize,
    exclude: Option<usize>,
) {
    let current = libraries.get(*idx).map(|l| l.label.as_str()).unwrap_or("—");
    egui::ComboBox::from_id_salt(id)
        .width(ui.available_width() - 8.0)
        .selected_text(current)
        .show_ui(ui, |ui| {
            for (i, lib) in libraries.iter().enumerate() {
                if Some(i) == exclude {
                    continue;
                }
                ui.selectable_value(idx, i, lib.label.as_str());
            }
        });
}

fn disk_line(ui: &mut egui::Ui, lib: &LibraryView) {
    match lib.disk {
        Some((total, avail)) if total > 0 => {
            let used = total.saturating_sub(avail);
            let pct = (used as f64 / total as f64 * 100.0).round() as u32;
            ui.label(trf(
                "{} / {} belegt ({} %) · {} frei",
                "{} / {} used ({} %) · {} free",
                &[
                    &human_size(used),
                    &human_size(total),
                    &pct.to_string(),
                    &human_size(avail),
                ],
            ));
        }
        _ => {
            ui.label(tr("Speicherplatz unbekannt", "disk space unknown"));
        }
    }
}

/// Cover-Miniatur (lokaler Cache, §3.5). Feste 2:3-Größe für ein einheitliches
/// Bild in jeder Zeile.
fn cover_cell(ui: &mut egui::Ui, cover: &Option<String>) {
    if let Some(uri) = cover {
        ui.add(egui::Image::from_uri(uri.clone()).fit_to_exact_size(egui::vec2(24.0, 36.0)));
    }
}

/// Kachel-Ansicht (§8): Cover-Grid. `selected = Some(..)` macht die Kacheln
/// klickbar (Quelle); `None` = schreibgeschützte Vorschau (Ziel).
///
/// Feste Spaltenzahl aus der verfügbaren Breite (garantierter Umbruch, statt
/// `horizontal_wrapped`, das hier nicht zuverlässig umbrach).
fn game_grid(ui: &mut egui::Ui, id: &str, games: &[GameRow], mut selected: Option<&mut HashSet<u32>>) {
    const COVER_W: f32 = 104.0;
    const COVER_H: f32 = 156.0;
    const CELL_W: f32 = 120.0;

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .id_salt(id)
        .show(ui, |ui| {
            let cols = ((ui.available_width() / CELL_W).floor() as usize).max(1);
            egui::Grid::new(format!("{}_grid", id))
                .num_columns(cols)
                .spacing([6.0, 10.0])
                .show(ui, |ui| {
                    for (i, row) in games.iter().enumerate() {
                        let is_sel =
                            selected.as_ref().map_or(false, |s| s.contains(&row.appid));
                        let inner = ui.scope(|ui| {
                            ui.set_width(COVER_W);
                            let fill = if is_sel {
                                ui.visuals().selection.bg_fill
                            } else {
                                egui::Color32::TRANSPARENT
                            };
                            egui::Frame::none()
                                .fill(fill)
                                .inner_margin(4.0)
                                .rounding(4.0)
                                .show(ui, |ui| {
                                    ui.vertical_centered(|ui| {
                                        match &row.cover {
                                            Some(uri) => {
                                                // Feste Zielgröße: alle Cover sind 2:3, daher
                                                // keine Verzerrung — aber garantiert einheitlich
                                                // (unabhängig von der Spaltenbreite/Ladezustand).
                                                ui.add(egui::Image::from_uri(uri.clone())
                                                    .fit_to_exact_size(egui::vec2(COVER_W, COVER_H)));
                                            }
                                            None => {
                                                let (rect, _) = ui.allocate_exact_size(
                                                    egui::vec2(COVER_W, COVER_H),
                                                    egui::Sense::hover(),
                                                );
                                                ui.painter().rect_filled(
                                                    rect,
                                                    4.0,
                                                    ui.visuals().extreme_bg_color,
                                                );
                                            }
                                        }
                                        ui.add_sized(
                                            [COVER_W, 30.0],
                                            egui::Label::new(egui::RichText::new(&row.name).small())
                                                .truncate(),
                                        );
                                    });
                                });
                        });

                        if let Some(sel) = selected.as_deref_mut() {
                            if row.blocked_reason.is_none() {
                                let resp = inner.response.interact(egui::Sense::click());
                                if resp.clicked() {
                                    if is_sel {
                                        sel.remove(&row.appid);
                                    } else {
                                        sel.insert(row.appid);
                                    }
                                }
                            }
                        }

                        if (i + 1) % cols == 0 {
                            ui.end_row();
                        }
                    }
                });
        });
}

/// Zelle einer Komponenten-Spalte: Checkbox falls Komponente vorhanden, sonst
/// ein dezenter Strich — mittig in der Spalte.
fn comp_cell(ui: &mut egui::Ui, active: bool, val: &mut bool) {
    // Beide Achsen zentrieren (horizontal + vertikal in der Zeilenhöhe).
    ui.with_layout(
        egui::Layout::centered_and_justified(egui::Direction::TopDown),
        |ui| {
            if active {
                ui.checkbox(val, "");
            } else {
                ui.weak("–");
            }
        },
    );
}

/// Schaltet eine Komponenten-Spalte für alle ausgewählten Spiele (mit dieser
/// Komponente) um: sind alle an, werden alle aus — sonst alle an.
fn toggle_all(
    games: &[GameRow],
    selected: &HashSet<u32>,
    choices: &mut HashMap<u32, ComponentChoice>,
    present: fn(&GameRow) -> bool,
    get: fn(&ComponentChoice) -> bool,
    set: fn(&mut ComponentChoice, bool),
) {
    let ids: Vec<u32> = games
        .iter()
        .filter(|r| selected.contains(&r.appid) && present(r))
        .map(|r| r.appid)
        .collect();
    if ids.is_empty() {
        return;
    }
    let all_on = ids.iter().all(|id| get(choices.entry(*id).or_default()));
    for id in ids {
        set(choices.entry(id).or_default(), !all_on);
    }
}

/// Linkes Panel: Library-Auswahl + Spieltabelle mit Auswahl- und
/// Komponenten-Spalten.
#[allow(clippy::too_many_arguments)]
pub fn source_panel(
    ui: &mut egui::Ui,
    libraries: &[LibraryView],
    source_idx: &mut usize,
    target_idx: usize,
    grid: bool,
    selected: &mut HashSet<u32>,
    comp_choice: &mut HashMap<u32, ComponentChoice>,
) {
    ui.heading(tr("Quelle", "Source"));
    if libraries.is_empty() {
        ui.label(tr("Keine Steam-Libraries gefunden.", "No Steam libraries found."));
        return;
    }
    // Bei mehreren Bibliotheken das Ziel aus der Quell-Auswahl ausblenden.
    let exclude = (libraries.len() > 1).then_some(target_idx);
    library_combo(ui, "source_lib", libraries, source_idx, exclude);
    let lib = &libraries[*source_idx];
    disk_line(ui, lib);

    ui.horizontal(|ui| {
        ui.set_height(TOOLBAR_H);
        if ui.small_button(tr("Alle", "All")).clicked() {
            for r in lib.games.iter().filter(|r| r.blocked_reason.is_none()) {
                selected.insert(r.appid);
            }
        }
        if ui.small_button(tr("Keine", "None")).clicked() {
            selected.clear();
        }
        ui.weak(if grid {
            tr("· Bild anklicken zum Auswählen", "· click a cover to select")
        } else {
            tr("· Spaltenkopf schaltet die Auswahl um", "· click a column header to toggle")
        });
    });

    if grid {
        game_grid(ui, "grid_src", &lib.games, Some(selected));
        return;
    }

    // Kopfklick nur vormerken (die Body-Closure hält die &mut-Borrows auf
    // selected/comp_choice — der Header darf sie nicht gleichzeitig anfassen).
    let mut toggle: Option<ToggleCol> = None;

    TableBuilder::new(ui)
        .striped(true)
        .id_salt("source_table")
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::exact(30.0)) // Cover
        .column(Column::auto()) // Auswahl
        .column(Column::remainder().at_least(120.0)) // Spiel (skaliert mit)
        .column(Column::auto()) // Größe
        .column(Column::auto()) // compat
        .column(Column::auto()) // workshop
        .column(Column::auto()) // shader
        .header(HEADER_H, |mut header| {
            header.col(|ui| {
                ui.strong("");
            });
            header.col(|ui| {
                ui.strong("");
            });
            header.col(|ui| {
                ui.strong(tr("Spiel", "Game"));
            });
            header.col(|ui| {
                ui.strong(tr("Größe", "Size"));
            });
            header.col(|ui| {
                ui.vertical_centered(|ui| {
                    if ui
                        .button("compat")
                        .on_hover_text(tr(
                            "compatdata (Savegames + Prefix). An = mitnehmen, aus = in Quelle belassen. Klick: für alle Ausgewählten umschalten",
                            "compatdata (savegames + prefix). On = move, off = leave in source. Click: toggle for all selected",
                        ))
                        .clicked()
                    {
                        toggle = Some(ToggleCol::Compat);
                    }
                });
            });
            header.col(|ui| {
                ui.vertical_centered(|ui| {
                    if ui
                        .button("workshop")
                        .on_hover_text(tr(
                            "Workshop-Mods. An = mitnehmen, aus = belassen. Klick: für alle Ausgewählten umschalten",
                            "Workshop mods. On = move, off = leave. Click: toggle for all selected",
                        ))
                        .clicked()
                    {
                        toggle = Some(ToggleCol::Workshop);
                    }
                });
            });
            header.col(|ui| {
                ui.vertical_centered(|ui| {
                    if ui
                        .button("shader")
                        .on_hover_text(tr(
                            "shadercache (wird neu erzeugt). An = mitnehmen, aus = löschen. Klick: für alle Ausgewählten umschalten",
                            "shadercache (regenerated). On = move, off = delete. Click: toggle for all selected",
                        ))
                        .clicked()
                    {
                        toggle = Some(ToggleCol::Shader);
                    }
                });
            });
        })
        .body(|mut body| {
            for row in &lib.games {
                body.row(42.0, |mut tr| {
                    let enabled = row.blocked_reason.is_none();

                    tr.col(|ui| cover_cell(ui, &row.cover));
                    tr.col(|ui| {
                        let mut sel = selected.contains(&row.appid);
                        if ui
                            .add_enabled(enabled, egui::Checkbox::new(&mut sel, ""))
                            .changed()
                        {
                            if sel {
                                selected.insert(row.appid);
                            } else {
                                selected.remove(&row.appid);
                            }
                        }
                    });
                    tr.col(|ui| {
                        ui.horizontal(|ui| {
                            match &row.blocked_reason {
                                Some(reason) => {
                                    ui.add(egui::Label::new(egui::RichText::new(&row.name).weak()).truncate())
                                        .on_hover_text(reason);
                                }
                                None => {
                                    ui.add(egui::Label::new(&row.name).truncate());
                                }
                            }
                            if row.is_tool {
                                ui.weak("[Tool]");
                            }
                        });
                    });
                    tr.col(|ui| {
                        ui.monospace(human_size(row.size));
                    });
                    let choice = comp_choice.entry(row.appid).or_default();
                    tr.col(|ui| comp_cell(ui, enabled && row.has_compatdata, &mut choice.compatdata));
                    tr.col(|ui| comp_cell(ui, enabled && row.has_workshop, &mut choice.workshop));
                    tr.col(|ui| comp_cell(ui, enabled && row.has_shadercache, &mut choice.move_shadercache));
                });
            }
        });

    // Kopfklick jetzt anwenden (Borrows der Tabelle sind freigegeben).
    match toggle {
        Some(ToggleCol::Compat) => toggle_all(&lib.games, selected, comp_choice,
            |r| r.has_compatdata, |c| c.compatdata, |c, v| c.compatdata = v),
        Some(ToggleCol::Workshop) => toggle_all(&lib.games, selected, comp_choice,
            |r| r.has_workshop, |c| c.workshop, |c, v| c.workshop = v),
        Some(ToggleCol::Shader) => toggle_all(&lib.games, selected, comp_choice,
            |r| r.has_shadercache, |c| c.move_shadercache, |c, v| c.move_shadercache = v),
        None => {}
    }
}

/// Rechtes Panel: Ziel-Library + (schreibgeschützte) Spieleliste. Gleicher
/// Aufbau wie die Quelle (Toolbar-Höhe + Kopfzeile), damit beide Tabellen auf
/// derselben Höhe beginnen.
pub fn target_panel(
    ui: &mut egui::Ui,
    libraries: &[LibraryView],
    target_idx: &mut usize,
    source_idx: usize,
    grid: bool,
) {
    ui.heading(tr("Ziel", "Target"));
    if libraries.is_empty() {
        return;
    }
    // Bei mehreren Bibliotheken die Quelle aus der Ziel-Auswahl ausblenden.
    let exclude = (libraries.len() > 1).then_some(source_idx);
    library_combo(ui, "target_lib", libraries, target_idx, exclude);
    let lib = &libraries[*target_idx];
    disk_line(ui, lib);

    // Toolbar-Zeile exakt gleicher Höhe wie in der Quelle (statt Separator).
    ui.horizontal(|ui| {
        ui.set_height(TOOLBAR_H);
        if *target_idx == source_idx {
            ui.weak(tr("Nur eine Bibliothek — kein Ziel zum Verschieben", "Only one library — no target to move to"));
        } else {
            ui.weak(tr("Vorschau — bereits im Ziel installierte Spiele", "Preview — games already installed in the target"));
        }
    });

    if grid {
        game_grid(ui, "grid_dst", &lib.games, None);
        return;
    }

    TableBuilder::new(ui)
        .striped(true)
        .id_salt("target_table")
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::exact(30.0)) // Cover
        .column(Column::remainder().at_least(120.0))
        .column(Column::auto())
        .header(HEADER_H, |mut header| {
            header.col(|ui| {
                ui.strong("");
            });
            header.col(|ui| {
                ui.strong(tr("Spiel", "Game"));
            });
            header.col(|ui| {
                ui.strong(tr("Größe", "Size"));
            });
        })
        .body(|mut body| {
            for row in &lib.games {
                body.row(42.0, |mut tr| {
                    tr.col(|ui| cover_cell(ui, &row.cover));
                    tr.col(|ui| {
                        ui.horizontal(|ui| {
                            ui.add(egui::Label::new(&row.name).truncate());
                            if row.is_tool {
                                ui.weak("[Tool]");
                            }
                        });
                    });
                    tr.col(|ui| {
                        ui.monospace(human_size(row.size));
                    });
                });
            }
        });
}
