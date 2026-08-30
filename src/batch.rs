//! Apply the current settings to a whole folder of images.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};

use eframe::egui;
use rayon::prelude::*;

use crate::normalmap::{self, MapKind, Settings};

pub const IMAGE_EXTENSIONS: [&str; 9] = [
    "png", "jpg", "jpeg", "bmp", "tga", "tif", "tiff", "webp", "gif",
];

/// A batch run in flight.
struct Run {
    total: usize,
    done: Arc<AtomicUsize>,
    rx: Receiver<String>,
}

pub struct Batch {
    pub open: bool,
    input: Option<PathBuf>,
    output: Option<PathBuf>,
    selected: [bool; 4],
    run: Option<Run>,
    report: Option<String>,
}

impl Default for Batch {
    fn default() -> Self {
        Self {
            open: false,
            input: None,
            output: None,
            // Normal only, matching what most people came for.
            selected: [false, true, false, false],
            run: None,
            report: None,
        }
    }
}

impl Batch {
    pub fn ui(&mut self, ctx: &egui::Context, settings: &Settings) {
        if !self.open {
            return;
        }
        self.collect_finished();

        let mut open = self.open;
        egui::Window::new("Batch convert")
            .open(&mut open)
            .default_width(420.0)
            .collapsible(false)
            .show(ctx, |ui| self.contents(ui, ctx, settings));
        self.open = open;
    }

    fn contents(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, settings: &Settings) {
        let running = self.run.is_some();

        ui.add_enabled_ui(!running, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Input folder…").clicked()
                    && let Some(dir) = rfd::FileDialog::new().pick_folder()
                {
                    if self.output.is_none() {
                        self.output = Some(dir.clone());
                    }
                    self.input = Some(dir);
                }
                ui.label(display_dir(self.input.as_deref()));
            });
            ui.horizontal(|ui| {
                if ui.button("Output folder…").clicked()
                    && let Some(dir) = rfd::FileDialog::new().pick_folder()
                {
                    self.output = Some(dir);
                }
                ui.label(display_dir(self.output.as_deref()));
            });

            ui.add_space(6.0);
            ui.label("Maps to write:");
            ui.horizontal(|ui| {
                for (i, kind) in MapKind::ALL.iter().enumerate() {
                    ui.checkbox(&mut self.selected[i], kind.label());
                }
            });
        });

        ui.add_space(8.0);
        if let Some(run) = &self.run {
            let done = run.done.load(Ordering::Relaxed);
            ui.add(
                egui::ProgressBar::new(done as f32 / run.total.max(1) as f32)
                    .text(format!("{done} / {}", run.total)),
            );
            // Keep the bar moving while the workers churn.
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        } else {
            let ready = self.input.is_some() && self.output.is_some() && !self.kinds().is_empty();
            if ui
                .add_enabled(ready, egui::Button::new("Run"))
                .on_disabled_hover_text("Pick both folders and at least one map")
                .clicked()
            {
                self.start(ctx.clone(), *settings);
            }
        }

        if let Some(report) = &self.report {
            ui.add_space(6.0);
            ui.separator();
            ui.label(report);
        }
    }

    fn kinds(&self) -> Vec<MapKind> {
        MapKind::ALL
            .iter()
            .enumerate()
            .filter(|(i, _)| self.selected[*i])
            .map(|(_, &k)| k)
            .collect()
    }

    fn start(&mut self, ctx: egui::Context, settings: Settings) {
        let (Some(input), Some(output)) = (self.input.clone(), self.output.clone()) else {
            return;
        };
        let files = list_images(&input);
        if files.is_empty() {
            self.report = Some(format!("No images found in {}", input.display()));
            return;
        }

        let kinds = self.kinds();
        let done = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = channel();
        self.report = None;
        self.run = Some(Run {
            total: files.len(),
            done: done.clone(),
            rx,
        });

        std::thread::Builder::new()
            .name("normalmapper-batch".to_owned())
            .spawn(move || {
                let errors: Vec<String> = files
                    .par_iter()
                    .filter_map(|path| {
                        let result = convert(path, &output, &settings, &kinds);
                        done.fetch_add(1, Ordering::Relaxed);
                        result.err().map(|e| format!("{}: {e}", short(path)))
                    })
                    .collect();

                let summary = if errors.is_empty() {
                    format!(
                        "Done — {} image(s), {} map(s) each.",
                        files.len(),
                        kinds.len()
                    )
                } else {
                    format!(
                        "Finished with {} error(s):\n{}",
                        errors.len(),
                        errors.join("\n")
                    )
                };
                let _ = tx.send(summary);
                ctx.request_repaint();
            })
            .expect("spawn batch thread");
    }

    fn collect_finished(&mut self) {
        if let Some(run) = &self.run
            && let Ok(summary) = run.rx.try_recv()
        {
            self.report = Some(summary);
            self.run = None;
        }
    }
}

fn display_dir(path: Option<&Path>) -> String {
    match path {
        Some(p) => p.display().to_string(),
        None => "—".to_owned(),
    }
}

fn short(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

fn list_images(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
                    .unwrap_or(false)
        })
        .collect();
    files.sort();
    files
}

fn convert(
    path: &Path,
    output: &Path,
    settings: &Settings,
    kinds: &[MapKind],
) -> Result<(), String> {
    let src = image::open(path).map_err(|e| e.to_string())?.to_rgba8();
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "image".to_owned());

    for (kind, img) in normalmap::generate(&src, settings, kinds) {
        let dest = output.join(format!("{stem}_{}.png", kind.suffix()));
        img.save(&dest).map_err(|e| e.to_string())?;
    }
    Ok(())
}
