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
    job: Job,
    incomplete_jobs: usize,
    /// Quelle/Ziel nur beim ersten Laden vorbelegen, danach die Wahl des
    /// Nutzers über Reloads hinweg erhalten.
    initialized: bool,
    /// Schrift-/Zoom-Faktor (persistiert, §4K-Displays).
    zoom_factor: f32,
    /// Zuletzt gesehene Fenster-Innengröße (Punkte) und Debounce fürs Speichern.
    window_size: (f32, f32),
    last_size_save: Instant,
}

/// Aggregierte Kennzahlen der aktuellen Auswahl (für die Zusammenfassung).
#[derive(Default)]
pub struct SelectionSummary {
    pub count: usize,
    pub bytes: u64,
}

impl BargeApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let cfg = crate::config::Config::load();
        cc.egui_ctx.set_zoom_factor(cfg.zoom_factor);
        let load_rx = Some(spawn_load(cc.egui_ctx.clone()));
        BargeApp {
            load_rx,
            load_error: None,
            libraries: Vec::new(),
            source_idx: 0,
            target_idx: 0,
            selected: HashSet::new(),
            comp_choice: HashMap::new(),
            limit_mbps: 250,
            dry_run: false,
            job: Job::Idle,
            incomplete_jobs: crate::mover::journal::Journal::scan_incomplete().len(),
            initialized: false,
            zoom_factor: cfg.zoom_factor,
            window_size: (cfg.window_w, cfg.window_h),
            last_size_save: Instant::now(),
        }
    }

    fn save_config(&self) {
        crate::config::Config {
            zoom_factor: self.zoom_factor,
            window_w: self.window_size.0,
            window_h: self.window_size.1,
        }
        .save();
    }

    /// Setzt den Zoom, wendet ihn an und speichert die Config.
    fn set_zoom(&mut self, ctx: &egui::Context, z: f32) {
        self.zoom_factor = z.clamp(0.7, 2.5);
        ctx.set_zoom_factor(self.zoom_factor);
        self.save_config();
    }

    /// Verfolgt die Fenstergröße und speichert sie entprellt (§Stufe 6).
    fn track_window_size(&mut self, ctx: &egui::Context) {
        let sz = ctx.screen_rect().size();
        if sz.x < 100.0 || sz.y < 100.0 {
            return;
        }
        if (sz.x - self.window_size.0).abs() > 2.0 || (sz.y - self.window_size.1).abs() > 2.0 {
            self.window_size = (sz.x, sz.y);
            if self.last_size_save.elapsed() > Duration::from_millis(700) {
                self.save_config();
                self.last_size_save = Instant::now();
            }
        }
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

        // --- Hintergrund-Laden abholen
        if let Some(rx) = &self.load_rx {
            if let Ok(res) = rx.try_recv() {
                match res {
                    Ok(views) => {
                        self.libraries = views;
                        let n = self.libraries.len();
                        if !self.initialized {
                            // Erstbelegung: Quelle 0, Ziel 1 (falls vorhanden).
                            self.source_idx = 0;
                            self.target_idx = if n > 1 { 1 } else { 0 };
                            self.initialized = true;
                        } else {
                            // Reload: die Wahl des Nutzers behalten, nur begrenzen.
                            self.source_idx = self.source_idx.min(n.saturating_sub(1));
                            self.target_idx = self.target_idx.min(n.saturating_sub(1));
                        }
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
        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("barge");
                ui.label("— Steam-Spiele sicher und gedrosselt verschieben");
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
                });
            });
            // Optionszeile zentriert direkt unter dem Titel (§8.1).
            settings::options_row(ui, &mut self.limit_mbps, &mut self.dry_run);
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

        egui::SidePanel::left("source")
            .resizable(true)
            .default_width(480.0)
            .show(ctx, |ui| {
                panels::source_panel(
                    ui,
                    &self.libraries,
                    &mut self.source_idx,
                    self.target_idx,
                    &mut self.selected,
                    &mut self.comp_choice,
                );
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            panels::target_panel(ui, &self.libraries, &mut self.target_idx, self.source_idx);
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
                self.job = Job::Running(job::start(queue, rate, ctx.clone()));
            }
        }
        if do_reload {
            self.reload(ctx);
        }
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

fn spawn_load(ctx: egui::Context) -> Receiver<Result<Vec<LibraryView>, String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut views = Vec::new();
        for lib in crate::steam::discovery::discover() {
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
                    GameRow {
                        appid: g.manifest.appid,
                        name: g.manifest.name.clone(),
                        size,
                        blocked_reason: g.manifest.blocked_reason(),
                        is_tool: g.manifest.is_tool(),
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
