//! Grafische Oberfläche (§8), eframe/egui im Immediate Mode.
//!
//! Zwei-Panel-Ansicht (Quelle/Ziel) nach Norton-/Midnight-Commander-Vorbild,
//! darunter Einstellungen und der Move-Auslöser. Das Verschieben läuft in einem
//! Worker-Thread ([`job`]); die eigentliche Logik ist die aus den Stufen 1–4.

mod job;
mod panels;
mod progress;
mod settings;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::mover::plan::{ComponentChoice, MovePlan};
use crate::mover::preconditions;
use crate::steam::game::Game;

/// Eine Library samt geladener Spiele fürs Rendering.
pub struct LibraryView {
    pub path: PathBuf,
    pub label: String,
    pub disk: Option<(u64, u64)>, // (total, avail)
    pub games: Vec<GameRow>,
}

pub struct GameRow {
    pub appid: u32,
    pub name: String,
    pub size: u64,
    pub blocked_reason: Option<String>,
    pub is_tool: bool,
    /// Cover-URI (§3.5): `file://` aus dem lokalen Steam-Cache, sonst als
    /// Fallback das Steam-CDN (`https://…`). `None` nur bei Tools ohne Cover.
    pub cover: Option<String>,
    /// Vorhandene, sichtbar zu machende Zusatzkomponenten (§4).
    pub has_compatdata: bool,
    pub has_workshop: bool,
    pub has_shadercache: bool,
    pub game: Game,
}

enum Job {
    Idle,
    Running(job::RunningJob),
    Finished(String),
}

pub struct BargeApp {
    load_rx: Option<Receiver<Result<Vec<LibraryView>, String>>>,
    load_error: Option<String>,
    libraries: Vec<LibraryView>,
    source_idx: usize,
    target_idx: usize,
    selected: HashSet<u32>,
    /// Komponentenwahl je Spiel (§4); fehlt ein Eintrag, gilt der Default.
    comp_choice: HashMap<u32, ComponentChoice>,
    limit_mbps: u64,
    dry_run: bool,
    verify: bool,
    /// Kachel- statt Listenansicht (persistiert).
    grid_view: bool,
    /// Farbschema ("dark" | "light" | "contrast"), persistiert.
    theme: String,
    job: Job,
    incomplete_jobs: usize,
    /// Quelle/Ziel nur beim ersten Laden vorbelegen, danach die Wahl des
    /// Nutzers über Reloads hinweg erhalten.
    initialized: bool,
    /// Schrift-/Zoom-Faktor (persistiert, §4K-Displays).
    zoom_factor: f32,
    /// Fenster-Innengröße in **logischen Pixeln** (nicht egui-Punkten).
    window_size: (f32, f32),
    /// Breite des Quell-Panels (egui-Punkte), wenn der Trenner frei ist.
    panel_w: f32,
    /// Trenner mittig halten, bis gezogen (Doppelklick zentriert wieder).
    panel_centered: bool,
    /// Zuletzt gespeicherter Limit-Wert (zum Erkennen von Änderungen).
    last_limit: u64,
    /// Vorherige Quell-/Ziel-Auswahl (zum Erkennen von Änderungen).
    prev_source_idx: usize,
    prev_target_idx: usize,
    /// Ausstehende, noch nicht gespeicherte Einstellungsänderung.
    dirty: bool,
    last_save: Instant,
    /// Läuft gerade ein Ordnerdialog (Hintergrund-Thread)?
    dialog_rx: Option<Receiver<Option<PathBuf>>>,
    /// Fehlermeldung, die als eigenes Fenster mit OK gezeigt wird.
    error_modal: Option<String>,
}

/// Aggregierte Kennzahlen der aktuellen Auswahl (für die Zusammenfassung).
#[derive(Default)]
pub struct SelectionSummary {
    pub count: usize,
    pub bytes: u64,
}

impl BargeApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let cfg = crate::config::Config::load();
        cc.egui_ctx.set_zoom_factor(cfg.zoom_factor);
        apply_theme(&cc.egui_ctx, &cfg.theme);
        let load_rx = Some(spawn_load(cc.egui_ctx.clone()));
        BargeApp {
            load_rx,
            load_error: None,
            libraries: Vec::new(),
            source_idx: 0,
            target_idx: 0,
            selected: HashSet::new(),
            comp_choice: HashMap::new(),
            limit_mbps: cfg.limit_mbps,
            dry_run: false,
            verify: true,
            grid_view: cfg.grid_view,
            theme: cfg.theme,
            job: Job::Idle,
            incomplete_jobs: crate::mover::journal::Journal::scan_incomplete().len(),
            initialized: false,
            zoom_factor: cfg.zoom_factor,
            window_size: (cfg.window_w, cfg.window_h),
            panel_w: cfg.panel_w,
            panel_centered: cfg.divider_centered,
            last_limit: cfg.limit_mbps,
            prev_source_idx: 0,
            prev_target_idx: 0,
            dirty: false,
            last_save: Instant::now(),
            dialog_rx: None,
            error_modal: None,
        }
    }

    fn save_config(&self) {
        // Vorhandene Config laden, nur die eigenen Felder aktualisieren, damit
        // extra_libraries u. Ä. erhalten bleiben.
        let mut cfg = crate::config::Config::load();
        cfg.zoom_factor = self.zoom_factor;
        cfg.window_w = self.window_size.0;
        cfg.window_h = self.window_size.1;
        cfg.panel_w = self.panel_w;
        cfg.divider_centered = self.panel_centered;
        cfg.limit_mbps = self.limit_mbps;
        cfg.grid_view = self.grid_view;
        cfg.theme = self.theme.clone();
        // Quelle/Ziel nur überschreiben, wenn die Libraries geladen sind.
        if let Some(l) = self.libraries.get(self.source_idx) {
            cfg.source_lib = l.path.display().to_string();
        }
        if let Some(l) = self.libraries.get(self.target_idx) {
            cfg.target_lib = l.path.display().to_string();
        }
        cfg.save();
    }

    /// Speichert entprellt, wenn eine Einstellung geändert wurde.
    fn flush_if_dirty(&mut self) {
        if self.dirty && self.last_save.elapsed() > Duration::from_millis(500) {
            self.save_config();
            self.dirty = false;
            self.last_save = Instant::now();
        }
    }

    /// Öffnet (in einem Hintergrund-Thread) den Ordnerdialog zum Hinzufügen
    /// einer Library (§8.3).
    fn open_add_library_dialog(&mut self) {
        if self.dialog_rx.is_some() {
            return; // schon offen
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let picked = rfd::FileDialog::new()
                .set_title("Steam-Library-Ordner wählen (Root oder steamapps/)")
                .pick_folder();
            let _ = tx.send(picked);
        });
        self.dialog_rx = Some(rx);
    }

    /// Fügt einen gewählten Pfad als Library hinzu (persistiert) und lädt neu.
    fn add_library(&mut self, ctx: &egui::Context, path: PathBuf) {
        match crate::steam::discovery::normalize_root(&path) {
            Some(root) => {
                let mut cfg = crate::config::Config::load();
                let s = root.to_string_lossy().to_string();
                if !cfg.extra_libraries.iter().any(|p| p == &s) {
                    cfg.extra_libraries.push(s);
                    cfg.zoom_factor = self.zoom_factor;
                    cfg.window_w = self.window_size.0;
                    cfg.window_h = self.window_size.1;
                    cfg.save();
                }
                self.reload(ctx);
            }
            None => {
                self.error_modal = Some(format!(
                    "Kein gültiger Steam-Library-Ordner:\n{}\n\nErwartet wird ein Ordner mit \
                     einem Unterordner „steamapps/“ (oder direkt das steamapps/-Verzeichnis).",
                    path.display()
                ));
            }
        }
    }

    /// Setzt den Zoom, wendet ihn an und merkt die Änderung vor.
    fn set_zoom(&mut self, ctx: &egui::Context, z: f32) {
        self.zoom_factor = z.clamp(0.7, 2.5);
        ctx.set_zoom_factor(self.zoom_factor);
        self.dirty = true;
    }

    /// Verfolgt die Fenstergröße. `screen_rect` liefert egui-Punkte; für das
    /// Wiederherstellen via `with_inner_size` brauchen wir logische Pixel, also
    /// mit dem Zoomfaktor multiplizieren (Punkte = Pixel / Zoom).
    fn track_window_size(&mut self, ctx: &egui::Context) {
        let pts = ctx.screen_rect().size();
        if pts.x < 100.0 || pts.y < 100.0 {
            return;
        }
        let logical = (pts.x * self.zoom_factor, pts.y * self.zoom_factor);
        if (logical.0 - self.window_size.0).abs() > 2.0
            || (logical.1 - self.window_size.1).abs() > 2.0
        {
            self.window_size = logical;
            self.dirty = true;
        }
    }

    /// Index der Library mit diesem (kanonischen) Pfad, falls vorhanden.
    fn find_library(&self, path: &str) -> Option<usize> {
        if path.is_empty() {
            return None;
        }
        let want = PathBuf::from(path);
        self.libraries.iter().position(|l| l.path == want)
    }

    fn selection_summary(&self) -> SelectionSummary {
        let mut s = SelectionSummary::default();
        let Some(src) = self.libraries.get(self.source_idx) else {
            return s;
        };
        for row in &src.games {
            if self.selected.contains(&row.appid) {
                s.count += 1;
                s.bytes += row.size;
            }
        }
        s
    }

    /// Baut die (Spiel, Plan)-Warteschlange aus der aktuellen Auswahl.
    fn build_queue(&self) -> Vec<(Game, MovePlan)> {
        let Some(src) = self.libraries.get(self.source_idx) else {
            return Vec::new();
        };
        let target = self.libraries[self.target_idx].path.clone();
        src.games
            .iter()
            .filter(|r| self.selected.contains(&r.appid) && r.blocked_reason.is_none())
            .map(|r| {
                let choice = self.comp_choice.get(&r.appid).copied().unwrap_or_default();
                let plan = MovePlan::new(&r.game, &target, choice);
                (r.game.clone(), plan)
            })
            .collect()
    }

    fn reload(&mut self, ctx: &egui::Context) {
        self.selected.clear();
        self.comp_choice.clear();
        self.job = Job::Idle;
        self.load_error = None;
        self.incomplete_jobs = crate::mover::journal::Journal::scan_incomplete().len();
        self.load_rx = Some(spawn_load(ctx.clone()));
    }
}

impl eframe::App for BargeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.track_window_size(ctx);

        // --- Ergebnis des Ordnerdialogs abholen
        if let Some(rx) = &self.dialog_rx {
            if let Ok(result) = rx.try_recv() {
                self.dialog_rx = None;
                if let Some(path) = result {
                    self.add_library(ctx, path);
                }
            }
        }

        // --- Hintergrund-Laden abholen
        if let Some(rx) = &self.load_rx {
            if let Ok(res) = rx.try_recv() {
                match res {
                    Ok(views) => {
                        self.libraries = views;
                        let n = self.libraries.len();
                        if !self.initialized {
                            // Erstbelegung: zuletzt gewählte Quelle/Ziel per Pfad
                            // zuordnen, sonst Default (Quelle 0, Ziel 1). Fehlt
                            // eine gespeicherte Library, greift der Fallback.
                            let cfg = crate::config::Config::load();
                            self.source_idx = self.find_library(&cfg.source_lib).unwrap_or(0);
                            self.target_idx = self
                                .find_library(&cfg.target_lib)
                                .filter(|&t| t != self.source_idx)
                                .unwrap_or_else(|| {
                                    if n > 1 && self.source_idx == 0 { 1 } else { 0 }
                                });
                            self.initialized = true;
                        } else {
                            // Reload: die Wahl des Nutzers behalten, nur begrenzen.
                            self.source_idx = self.source_idx.min(n.saturating_sub(1));
                            self.target_idx = self.target_idx.min(n.saturating_sub(1));
                        }
                        self.prev_source_idx = self.source_idx;
                        self.prev_target_idx = self.target_idx;
                    }
                    Err(e) => self.load_error = Some(e),
                }
                self.load_rx = None;
            }
        }

        // --- laufenden Job pollen
        let mut finished_summary = None;
        if let Job::Running(r) = &mut self.job {
            r.poll();
            if r.finished {
                finished_summary = Some(r.summary());
            }
        }
        if let Some(summary) = finished_summary {
            self.job = Job::Finished(summary);
        }

        let loading = self.load_rx.is_some();

        let mut new_zoom: Option<f32> = None;
        let mut new_theme: Option<String> = None;
        let mut open_dialog = false;
        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("barge");
                ui.label("— Steam-Spiele sicher und gedrosselt verschieben");
                ui.separator();
                // Ansicht Liste/Kacheln.
                if ui.selectable_label(!self.grid_view, "Liste").clicked() && self.grid_view {
                    self.grid_view = false;
                    self.dirty = true;
                }
                if ui.selectable_label(self.grid_view, "Kacheln").clicked() && !self.grid_view {
                    self.grid_view = true;
                    self.dirty = true;
                }
                // Zoom-Regler rechts (persistiert).
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("A +").on_hover_text("Schrift größer").clicked() {
                        new_zoom = Some(self.zoom_factor + 0.1);
                    }
                    ui.label(format!("{:.0} %", self.zoom_factor * 100.0));
                    if ui.button("A -").on_hover_text("Schrift kleiner").clicked() {
                        new_zoom = Some(self.zoom_factor - 0.1);
                    }
                    ui.label("Schrift:");
                    ui.separator();
                    // Farbschema.
                    let theme_label = match self.theme.as_str() {
                        "light" => "Hell",
                        "contrast" => "Kontrast",
                        _ => "Dunkel",
                    };
                    egui::ComboBox::from_id_salt("theme")
                        .selected_text(theme_label)
                        .show_ui(ui, |ui| {
                            for (val, label) in
                                [("dark", "Dunkel"), ("light", "Hell"), ("contrast", "Kontrast")]
                            {
                                if ui.selectable_label(self.theme == val, label).clicked() {
                                    new_theme = Some(val.to_string());
                                }
                            }
                        });
                    ui.label("Thema:");
                });
            });
            // Optionszeile zentriert direkt unter dem Titel (§8.1).
            if settings::options_row(ui, &mut self.limit_mbps, &mut self.dry_run, &mut self.verify) {
                open_dialog = true;
            }
            if self.incomplete_jobs > 0 {
                ui.colored_label(
                    egui::Color32::from_rgb(0xd0, 0x90, 0x30),
                    format!(
                        "(!) {} unvollendete(r) Move-Job(s) — im Terminal `barge recover`",
                        self.incomplete_jobs
                    ),
                );
            }
            ui.add_space(4.0);
        });
        if let Some(z) = new_zoom {
            self.set_zoom(ctx, z);
        }
        if let Some(t) = new_theme {
            self.theme = t;
            apply_theme(ctx, &self.theme);
            self.dirty = true;
        }
        if open_dialog {
            self.open_add_library_dialog();
        }
        // Limit-Änderung (aus der Optionszeile) erkennen und vormerken.
        if self.limit_mbps != self.last_limit {
            self.last_limit = self.limit_mbps;
            self.dirty = true;
        }
        // Geänderte Quelle/Ziel-Auswahl vormerken.
        if self.source_idx != self.prev_source_idx || self.target_idx != self.prev_target_idx {
            self.prev_source_idx = self.source_idx;
            self.prev_target_idx = self.target_idx;
            self.dirty = true;
        }

        if loading {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(80.0);
                    ui.spinner();
                    ui.label("Bibliotheken werden geladen (Größen werden berechnet)…");
                });
            });
            return;
        }
        if let Some(err) = self.load_error.clone() {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.colored_label(egui::Color32::LIGHT_RED, format!("Fehler: {}", err));
            });
            return;
        }

        let summary = self.selection_summary();
        let mut start_move = false;
        let mut do_reload = false;
        let mut copy_to_clipboard: Option<String> = None;

        egui::TopBottomPanel::bottom("actions").show(ctx, |ui| {
            ui.add_space(6.0);
            match &mut self.job {
                Job::Running(r) => {
                    progress::view(ui, r);
                }
                Job::Finished(_) => {
                    // Ergebnis wird als eigenes Fenster gezeigt (siehe unten).
                    ui.label("Move abgeschlossen — siehe Ergebnis-Fenster.");
                }
                Job::Idle => {
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "Auswahl: {} Spiel(e) · {}",
                                summary.count,
                                crate::util::human_size(summary.bytes)
                            ))
                            .size(18.0)
                            .strong(),
                        );
                        ui.add_space(8.0);
                        let same = self.source_idx == self.target_idx;
                        let can_go = summary.count > 0 && !same;
                        let label = if self.dry_run { "Trockenlauf" } else { "Verschieben" };
                        let btn = egui::Button::new(egui::RichText::new(label).size(19.0))
                            .min_size(egui::vec2(280.0, 48.0));
                        if ui.add_enabled(can_go, btn).clicked() {
                            start_move = true;
                        }
                        if self.libraries.len() < 2 {
                            ui.colored_label(
                                egui::Color32::LIGHT_RED,
                                "Nur eine Steam-Bibliothek gefunden — kein Ziel zum Verschieben",
                            );
                        } else if same {
                            ui.colored_label(egui::Color32::LIGHT_RED, "Quelle und Ziel sind identisch");
                        } else if summary.count == 0 {
                            ui.label("keine Spiele ausgewählt");
                        }
                    });
                }
            }
            ui.add_space(6.0);
        });

        // Zwei-Panel-Split mit eigenem Trenner: standardmäßig mittig (folgt der
        // Fensterbreite); Ziehen gibt ihn frei, Doppelklick zentriert wieder.
        egui::CentralPanel::default().show(ctx, |ui| {
            const SEP: f32 = 8.0;
            const MIN_SIDE: f32 = 300.0;
            let total = ui.available_width();
            let avail_h = ui.available_height();
            let max_left = (total - SEP - MIN_SIDE).max(MIN_SIDE);
            let left_w = if self.panel_centered {
                ((total - SEP) * 0.5).clamp(MIN_SIDE, max_left)
            } else {
                self.panel_w.clamp(MIN_SIDE, max_left)
            };

            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(left_w, avail_h),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        panels::source_panel(
                            ui,
                            &self.libraries,
                            &mut self.source_idx,
                            self.target_idx,
                            self.grid_view,
                            &mut self.selected,
                            &mut self.comp_choice,
                        );
                    },
                );

                // Trenner: anklickbar/ziehbar.
                let (rect, resp) =
                    ui.allocate_exact_size(egui::vec2(SEP, avail_h), egui::Sense::click_and_drag());
                let hovered = resp.hovered() || resp.dragged();
                let color = if hovered {
                    ui.visuals().widgets.hovered.fg_stroke.color
                } else {
                    ui.visuals().widgets.noninteractive.bg_stroke.color
                };
                ui.painter()
                    .vline(rect.center().x, rect.y_range(), egui::Stroke::new(2.0, color));
                if hovered {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                }
                if resp.double_clicked() {
                    self.panel_centered = true;
                    self.dirty = true;
                } else if resp.dragged() {
                    self.panel_centered = false;
                    self.panel_w = (left_w + resp.drag_delta().x).clamp(MIN_SIDE, max_left);
                    self.dirty = true;
                }

                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), avail_h),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        panels::target_panel(
                            ui,
                            &self.libraries,
                            &mut self.target_idx,
                            self.source_idx,
                            self.grid_view,
                        );
                    },
                );
            });
        });

        // --- Ergebnis-Fenster nach Abschluss eines Jobs (kopierbar + OK).
        if let Job::Finished(msg) = &self.job {
            egui::Window::new("Move abgeschlossen")
                .collapsible(false)
                .resizable(true)
                .default_width(560.0)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("Zusammenfassung der verschobenen Komponenten:");
                    egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut msg.as_str())
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                                .interactive(false),
                        );
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("In Zwischenablage kopieren").clicked() {
                            copy_to_clipboard = Some(msg.clone());
                        }
                        if ui.button("OK").clicked() {
                            do_reload = true;
                        }
                    });
                });
        }

        // --- Aktionen nach dem Rendern ausführen (Borrow-Konflikte vermeiden)
        if let Some(text) = copy_to_clipboard {
            ctx.copy_text(text);
        }
        if start_move {
            let queue = self.build_queue();
            if self.dry_run {
                self.job = Job::Finished(dry_run_report(&queue));
            } else {
                let rate = self.limit_mbps.saturating_mul(1_000_000);
                self.job = Job::Running(job::start(queue, rate, self.verify, ctx.clone()));
            }
        }
        if do_reload {
            self.reload(ctx);
        }

        // --- Fehler-Fenster (z. B. ungültige Bibliothek) mit OK-Rückkehr
        if let Some(msg) = self.error_modal.clone() {
            let mut dismiss = false;
            egui::Window::new("Fehler")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.colored_label(egui::Color32::LIGHT_RED, msg);
                    ui.add_space(8.0);
                    ui.vertical_centered(|ui| {
                        if ui.button("OK").clicked() {
                            dismiss = true;
                        }
                    });
                });
            if dismiss {
                self.error_modal = None;
            }
        }

        // Geänderte Einstellungen entprellt speichern.
        self.flush_if_dirty();
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Endgültige Fenstergröße sichern (Debounce könnte die letzte
        // Änderung sonst verschlucken).
        self.save_config();
    }
}

/// Trockenlauf (§8.4): Vorbedingungen prüfen und als Textbericht ausgeben.
fn dry_run_report(queue: &[(Game, MovePlan)]) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let _ = writeln!(s, "TROCKENLAUF -- keine Datei wird angefasst (§8.4)\n");
    for (game, plan) in queue {
        let _ = writeln!(
            s,
            "- {} (AppID {}) -- {} ueber {} Komponente(n)",
            plan.name,
            plan.appid,
            crate::util::human_size(plan.bytes_total),
            plan.items.len()
        );
        for item in &plan.items {
            let _ = writeln!(s, "    {:?}: {}", item.action, item.kind.label());
        }
        let report = preconditions::check(game, plan);
        for c in &report.checks {
            let _ = writeln!(s, "    [{}] {}: {}", if c.passed { "ok" } else { "!!" }, c.name, c.detail);
        }
        s.push('\n');
    }
    s
}

/// Cache-Verzeichnis für heruntergeladene Cover (`$XDG_CACHE_HOME/barge/covers`).
fn cover_cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    let d = base.join("barge").join("covers");
    std::fs::create_dir_all(&d).ok()?;
    Some(d)
}

/// Lädt das Cover eines Spiels vom Steam-CDN und legt es lokal ab (§3.5).
/// Fällt vom Hochkant-Cover auf das Header-Bild zurück. Gibt den lokalen Pfad
/// zurück oder `None` (auch offline / kein Bild verfügbar). Bereits gecachte
/// Dateien werden wiederverwendet.
fn fetch_cover(cache_dir: &std::path::Path, appid: u32) -> Option<PathBuf> {
    use std::io::Read;
    let out = cache_dir.join(format!("{}.jpg", appid));
    if out.is_file() {
        return Some(out);
    }
    // Nur das Hochkant-Cover (2:3) — einheitliche Kachelgröße. Titel ohne
    // Portrait bekommen einen Platzhalter statt eines Querformat-Headers.
    let url = format!(
        "https://cdn.cloudflare.steamstatic.com/steam/apps/{}/library_600x900.jpg",
        appid
    );
    let Ok(resp) = ureq::get(&url).timeout(std::time::Duration::from_secs(6)).call() else {
        return None;
    };
    let mut bytes = Vec::new();
    if resp.into_reader().take(16_000_000).read_to_end(&mut bytes).is_ok()
        && bytes.len() > 200
        && std::fs::write(&out, &bytes).is_ok()
    {
        return Some(out);
    }
    None
}

/// Sucht das Hochkant-Cover (2:3) eines Spiels im Steam-Cache (§3.5). Deckt
/// lokalisierte Namen (`library_600x900_german.jpg`), `_2x` und das alte flache
/// Layout ab. Header-Bilder (Querformat) werden bewusst NICHT genutzt, damit
/// die Kacheln einheitlich groß bleiben.
fn find_cover(cache_root: &std::path::Path, appid: u32) -> Option<PathBuf> {
    let dir = cache_root.join(appid.to_string());
    if let Ok(rd) = std::fs::read_dir(&dir) {
        let mut fallback: Option<PathBuf> = None;
        for e in rd.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("library_600x900") && name.ends_with(".jpg") {
                if name == "library_600x900.jpg" {
                    return Some(e.path()); // exakter Treffer bevorzugt
                }
                fallback = Some(e.path());
            }
        }
        if fallback.is_some() {
            return fallback;
        }
    }
    // Altes flaches Layout.
    let flat = cache_root.join(format!("{}_library_600x900.jpg", appid));
    flat.is_file().then_some(flat)
}

fn spawn_load(ctx: egui::Context) -> Receiver<Result<Vec<LibraryView>, String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // Erkannte Libraries plus manuell hinzugefügte (§8.3), dedupliziert.
        let mut libs = crate::steam::discovery::discover();
        for p in crate::config::Config::load().extra_libraries {
            if let Some(root) = crate::steam::discovery::normalize_root(std::path::Path::new(&p)) {
                if !libs.iter().any(|l| l.path == root) {
                    libs.push(crate::steam::library::Library::new(root));
                }
            }
        }

        // Steam-Bild-Cache (Cover) — nur in der Haupt-Installation vorhanden (§3.5).
        let cache_root = libs
            .iter()
            .map(|l| l.path.join("appcache").join("librarycache"))
            .find(|p| p.is_dir());
        // Eigener Cover-Cache für vom CDN nachgeladene Bilder.
        let cover_dir = cover_cache_dir();

        let mut views = Vec::new();
        for lib in libs {
            let disk = lib.disk_space();
            let (games, _errors) = lib.games();
            let mut rows: Vec<GameRow> = games
                .into_iter()
                .map(|g| {
                    use crate::steam::game::ComponentKind;
                    let size = g.moved_size();
                    let present = |k: ComponentKind| {
                        g.components.iter().any(|c| c.kind == k && c.present)
                    };
                    let is_tool = g.manifest.is_tool();
                    let appid = g.manifest.appid;
                    // Lokales Steam-Cover bevorzugen; sonst (außer bei Tools) vom
                    // CDN in den eigenen Cache laden. Nur lokale Dateien landen
                    // als URI in der UI — dadurch keine egui-HTTP-Fehler mehr.
                    let path = cache_root
                        .as_ref()
                        .and_then(|cr| find_cover(cr, appid))
                        .or_else(|| {
                            if is_tool {
                                None
                            } else {
                                cover_dir.as_ref().and_then(|d| fetch_cover(d, appid))
                            }
                        });
                    let cover = path.map(|p| format!("file://{}", p.display()));
                    GameRow {
                        appid,
                        name: g.manifest.name.clone(),
                        size,
                        blocked_reason: g.manifest.blocked_reason(),
                        is_tool,
                        cover,
                        has_compatdata: present(ComponentKind::Compatdata),
                        has_workshop: present(ComponentKind::WorkshopContent),
                        has_shadercache: present(ComponentKind::Shadercache),
                        game: g,
                    }
                })
                .collect();
            rows.sort_by(|a, b| b.size.cmp(&a.size));
            views.push(LibraryView {
                label: lib.path.display().to_string(),
                path: lib.path.clone(),
                disk,
                games: rows,
            });
        }
        let _ = tx.send(Ok(views));
        ctx.request_repaint();
    });
    rx
}

/// Wendet ein Farbschema an (§Stufe 6): dunkel, hell oder kontrastreich.
fn apply_theme(ctx: &egui::Context, theme: &str) {
    let visuals = match theme {
        "light" => egui::Visuals::light(),
        "contrast" => contrast_visuals(),
        _ => egui::Visuals::dark(),
    };
    ctx.set_visuals(visuals);
}

/// Kontrastreiches Dunkel-Schema (fast schwarzer Hintergrund, helle Schrift).
fn contrast_visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.override_text_color = Some(egui::Color32::from_gray(245));
    v.panel_fill = egui::Color32::from_gray(8);
    v.window_fill = egui::Color32::from_gray(8);
    v.extreme_bg_color = egui::Color32::BLACK;
    v.faint_bg_color = egui::Color32::from_gray(28);
    v.widgets.noninteractive.bg_stroke.color = egui::Color32::from_gray(110);
    v.widgets.inactive.bg_fill = egui::Color32::from_gray(45);
    v.selection.bg_fill = egui::Color32::from_rgb(0x2f, 0x74, 0xe0);
    v
}

/// Startet die grafische Oberfläche (§8). Blockiert bis das Fenster schließt.
pub fn run() -> eframe::Result<()> {
    let cfg = crate::config::Config::load();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([cfg.window_w, cfg.window_h])
            .with_min_inner_size([720.0, 480.0])
            .with_title("barge"),
        ..Default::default()
    };
    eframe::run_native("barge", options, Box::new(|cc| Ok(Box::new(BargeApp::new(cc)))))
}
