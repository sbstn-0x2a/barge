//! Move-Ausführung für die GUI: ein Worker-Thread (§2, §6.1) verschiebt die
//! ausgewählten Spiele sequenziell und meldet Fortschritt per Channel an den
//! UI-Thread. Abbrechen setzt ein Flag, das die Engine zwischen Chunks prüft
//! (§8.2); der laufende Job wird dann aufgeräumt.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::thread::JoinHandle;

use eframe::egui;

use crate::mover::copy::Stats;
use crate::mover::journal::{JobState, Journal};
use crate::mover::plan::{Action, MovePlan};
use crate::mover::{execute, preconditions};
use crate::steam::game::Game;

/// Nachrichten vom Worker an die UI.
pub enum Msg {
    Started { name: String, bytes_total: u64 },
    Progress { bytes_done: u64, rate_mbps: f64 },
    Skipped { name: String, reasons: Vec<String> },
    Done { name: String, moved: Vec<String>, deleted: Vec<String> },
    Failed { name: String, error: String },
    Cancelled,
    AllDone { moved: usize, total: usize },
}

/// Laufender Job samt gespiegeltem Zustand fürs Rendering.
pub struct RunningJob {
    rx: Receiver<Msg>,
    pub cancel: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    pub queue_total: usize,
    pub queue_done: usize,
    pub current_name: String,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub rate_mbps: f64,
    pub log: Vec<String>,
    pub finished: bool,
    pub cancelling: bool,
}

impl RunningJob {
    /// Verarbeitet alle anstehenden Worker-Nachrichten.
    pub fn poll(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Started { name, bytes_total } => {
                    self.current_name = name;
                    self.bytes_total = bytes_total;
                    self.bytes_done = 0;
                    self.rate_mbps = 0.0;
                }
                Msg::Progress { bytes_done, rate_mbps } => {
                    self.bytes_done = bytes_done;
                    self.rate_mbps = rate_mbps;
                }
                Msg::Skipped { name, reasons } => {
                    // ASCII-Marker: die egui-Monospace-Schrift hat keine ✓/✗-Glyphen.
                    self.log.push(format!("SKIP  {} -- {}", name, reasons.join("; ")));
                }
                Msg::Done { name, moved, deleted } => {
                    self.queue_done += 1;
                    let mut line = format!("OK    {} -- verschoben: {}", name, moved.join(", "));
                    if !deleted.is_empty() {
                        line.push_str(&format!("; geloescht: {}", deleted.join(", ")));
                    }
                    self.log.push(line);
                }
                Msg::Failed { name, error } => {
                    self.log.push(format!("FAIL  {} -- {}", name, error));
                }
                Msg::Cancelled => {
                    self.log.push("Abgebrochen -- .partial aufgeraeumt, Quelle intakt".into());
                    self.finished = true;
                }
                Msg::AllDone { moved, total } => {
                    self.log.push(format!("Fertig: {} von {} verschoben.", moved, total));
                    self.finished = true;
                }
            }
        }
    }

    /// Fordert den Abbruch an (§8.2).
    pub fn request_cancel(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancelling = true;
    }

    /// Zusammenfassung nach Abschluss (für die Ergebnisanzeige).
    pub fn summary(&self) -> String {
        self.log.join("\n")
    }
}

/// Startet den Worker-Thread für eine Warteschlange von (Spiel, Plan)-Paaren.
pub fn start(
    jobs: Vec<(Game, MovePlan)>,
    rate_bytes: u64,
    verify: bool,
    ctx: egui::Context,
) -> RunningJob {
    let (tx, rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let total = jobs.len();
    let worker_cancel = cancel.clone();

    let handle = std::thread::spawn(move || {
        let mut moved = 0usize;
        for (game, plan) in &jobs {
            if worker_cancel.load(Ordering::Relaxed) {
                let _ = tx.send(Msg::Cancelled);
                ctx.request_repaint();
                return;
            }

            // Vorbedingungen unmittelbar vor der Ausführung (§5).
            let report = preconditions::check(game, plan);
            if !report.all_passed() {
                let reasons = report
                    .failures()
                    .map(|c| format!("{}: {}", c.name, c.detail))
                    .collect();
                let _ = tx.send(Msg::Skipped { name: plan.name.clone(), reasons });
                ctx.request_repaint();
                continue;
            }

            let _ = tx.send(Msg::Started {
                name: plan.name.clone(),
                bytes_total: plan.bytes_total,
            });
            ctx.request_repaint();

            let labels = plan.moved_component_labels();
            let mut journal = match Journal::create(
                plan.appid,
                &plan.name,
                &plan.installdir,
                &plan.source_library,
                &plan.target_library,
                &labels,
                plan.bytes_total,
            ) {
                Ok(j) => j,
                Err(e) => {
                    let _ = tx.send(Msg::Failed { name: plan.name.clone(), error: e.to_string() });
                    ctx.request_repaint();
                    continue;
                }
            };

            let tx_p = tx.clone();
            let ctx_p = ctx.clone();
            let progress = move |st: &Stats, rate_mbps: f64| {
                let _ = tx_p.send(Msg::Progress { bytes_done: st.bytes, rate_mbps });
                ctx_p.request_repaint();
            };

            match execute::execute(plan, rate_bytes, false, verify, worker_cancel.clone(), &mut journal, progress) {
                Ok(_) => {
                    moved += 1;
                    let moved_comps: Vec<String> = plan
                        .items
                        .iter()
                        .filter(|i| i.action != Action::DeleteSource)
                        .map(|i| i.kind.label().to_string())
                        .collect();
                    let deleted_comps: Vec<String> = plan
                        .items
                        .iter()
                        .filter(|i| i.action == Action::DeleteSource)
                        .map(|i| i.kind.label().to_string())
                        .collect();
                    let _ = tx.send(Msg::Done {
                        name: plan.name.clone(),
                        moved: moved_comps,
                        deleted: deleted_comps,
                    });
                    ctx.request_repaint();
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                    // §8.2: Abbruch räumt das Ziel-.partial auf, Quelle bleibt.
                    let _ = execute::cleanup_target_partials(&journal);
                    let _ = tx.send(Msg::Cancelled);
                    ctx.request_repaint();
                    return;
                }
                Err(e) => {
                    let _ = journal.set_state(JobState::Failed);
                    let _ = tx.send(Msg::Failed { name: plan.name.clone(), error: e.to_string() });
                    ctx.request_repaint();
                }
            }
        }
        let _ = tx.send(Msg::AllDone { moved, total });
        ctx.request_repaint();
    });

    RunningJob {
        rx,
        cancel,
        handle: Some(handle),
        queue_total: total,
        queue_done: 0,
        current_name: String::new(),
        bytes_done: 0,
        bytes_total: 0,
        rate_mbps: 0.0,
        log: Vec::new(),
        finished: false,
        cancelling: false,
    }
}

impl Drop for RunningJob {
    fn drop(&mut self) {
        // Beim Verwerfen sauber abbrechen und auf den Worker warten.
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
