#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod batch;
mod normalmap;
mod worker;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use image::RgbaImage;

use batch::Batch;
use normalmap::{HeightSource, Kernel, MapKind, Settings};
use worker::{Request, Worker};

const PREVIEW_MAX: u32 = 1024;
/// How long the settings have to sit still before we kick off a regeneration.
const DEBOUNCE: Duration = Duration::from_millis(80);
const SETTINGS_KEY: &str = "settings";

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([760.0, 480.0])
            .with_title("Normal Mapper"),
        ..Default::default()
    };
    eframe::run_native(
        "Normal Mapper",
        options,
        Box::new(|cc| {
            setup_style(&cc.egui_ctx);
            Ok(Box::new(App::new(cc)))
        }),
    )
}

fn setup_style(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.spacing.slider_width = 160.0;
    });
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Source,
    Map(MapKind),
}

/// The currently loaded image plus everything derived from it.
struct Loaded {
    path: Option<PathBuf>,
    source: Arc<RgbaImage>,
    /// Downscaled copy the live preview is generated from.
    preview_source: Arc<RgbaImage>,
    source_tex: egui::TextureHandle,
    maps: Vec<(MapKind, egui::TextureHandle)>,
}

impl Loaded {
    fn texture(&self, kind: MapKind) -> Option<&egui::TextureHandle> {
        self.maps.iter().find(|(k, _)| *k == kind).map(|(_, t)| t)
    }
}

struct App {
    settings: Settings,
    loaded: Option<Loaded>,
    view: View,
    zoom: f32,
    fit: bool,
    /// Preview from a downscaled source while dragging sliders.
    fast_preview: bool,
    /// Set when the settings change; the request goes out once it stops moving.
    dirty_since: Option<Instant>,
    worker: Worker,
    generation: u64,
    pending: bool,
    last_gen_ms: f32,
    batch: Batch,
    status: String,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let settings = cc
            .storage
            .and_then(|s| s.get_string(SETTINGS_KEY))
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();

        Self {
            settings,
            loaded: None,
            view: View::Map(MapKind::Normal),
            zoom: 1.0,
            fit: true,
            fast_preview: true,
            dirty_since: None,
            worker: Worker::spawn(cc.egui_ctx.clone()),
            generation: 0,
            pending: false,
            last_gen_ms: 0.0,
            batch: Batch::default(),
            status: "Open an image, or drop one onto the window.".to_owned(),
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        self.poll_worker(&ctx);
        self.handle_dropped_files(&ctx);
        self.handle_shortcuts(&ctx);
        self.maybe_dispatch(&ctx);

        self.top_bar(ui, &ctx);
        self.side_panel(ui);
        self.status_bar(ui);
        self.central_panel(ui);
        self.batch.ui(&ctx, &self.settings);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if let Ok(json) = serde_json::to_string(&self.settings) {
            storage.set_string(SETTINGS_KEY, json);
        }
    }
}

// ---------------------------------------------------------------- UI

impl App {
    fn top_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::Panel::top("top").show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Open…").on_hover_text("Ctrl+O").clicked() {
                    self.open_dialog(ctx);
                }
                ui.add_enabled_ui(self.loaded.is_some(), |ui| {
                    if ui.button("Save map…").on_hover_text("Ctrl+S").clicked() {
                        self.save_dialog();
                    }
                    if ui
                        .button("Export all…")
                        .on_hover_text("Write every map at full resolution into a folder")
                        .clicked()
                    {
                        self.export_all();
                    }
                    if ui.button("Reload").clicked()
                        && let Some(path) = self.loaded.as_ref().and_then(|l| l.path.clone())
                    {
                        self.load(ctx, &path);
                    }
                });

                ui.separator();
                if ui.button("Batch…").clicked() {
                    self.batch.open = true;
                }

                ui.separator();
                if ui.button("Save preset…").clicked() {
                    self.save_preset();
                }
                if ui.button("Load preset…").clicked() {
                    self.load_preset();
                }
                if ui.button("Reset").clicked() {
                    self.settings = Settings::default();
                    self.touch();
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    egui::global_theme_preference_switch(ui);
                });
            });
        });
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        egui::Panel::left("settings")
            .resizable(false)
            .exact_size(280.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let before = self.settings;
                    let s = &mut self.settings;

                    ui.add_space(6.0);
                    ui.heading("Height");
                    egui::ComboBox::from_label("Source")
                        .selected_text(s.source.label())
                        .show_ui(ui, |ui| {
                            for opt in HeightSource::ALL {
                                ui.selectable_value(&mut s.source, opt, opt.label());
                            }
                        });
                    ui.add(
                        egui::Slider::new(&mut s.contrast, 0.0..=4.0)
                            .text("Contrast")
                            .clamping(egui::SliderClamping::Always),
                    );
                    ui.add(egui::Slider::new(&mut s.blur, 0.0..=16.0).text("Blur"))
                        .on_hover_text(
                            "Gaussian pre-blur; smooths out noise before differentiating",
                        );
                    ui.checkbox(&mut s.invert_height, "Invert height")
                        .on_hover_text("Treat dark pixels as peaks instead of valleys");

                    ui.add_space(10.0);
                    ui.heading("Normals");
                    egui::ComboBox::from_label("Kernel")
                        .selected_text(s.kernel.label())
                        .show_ui(ui, |ui| {
                            for opt in Kernel::ALL {
                                ui.selectable_value(&mut s.kernel, opt, opt.label());
                            }
                        });
                    ui.add(
                        egui::Slider::new(&mut s.strength, 0.05..=20.0)
                            .logarithmic(true)
                            .text("Strength"),
                    );
                    ui.checkbox(&mut s.flip_x, "Flip X");
                    ui.checkbox(&mut s.flip_y, "Flip Y (DirectX)")
                        .on_hover_text("Off: OpenGL / Bevy (+Y up). On: DirectX / Unity (+Y down)");
                    ui.checkbox(&mut s.tileable, "Seamless / tileable")
                        .on_hover_text("Wrap sampling across the borders");

                    ui.add_space(10.0);
                    ui.heading("Ambient occlusion");
                    ui.add(egui::Slider::new(&mut s.ao_radius, 0.0..=64.0).text("Radius"))
                        .on_hover_text("How far to search for occluders. 0 disables AO");
                    ui.add(egui::Slider::new(&mut s.ao_strength, 0.0..=2.0).text("Amount"));

                    ui.add_space(10.0);
                    ui.heading("Roughness");
                    ui.add(egui::Slider::new(&mut s.roughness_base, 0.0..=1.0).text("Base"))
                        .on_hover_text("Roughness of perfectly flat areas");
                    ui.add(egui::Slider::new(&mut s.roughness_detail, 0.0..=32.0).text("Detail"))
                        .on_hover_text("How strongly fine height detail raises roughness");
                    ui.checkbox(&mut s.roughness_invert, "Invert");

                    changed = self.settings != before;

                    ui.add_space(10.0);
                    ui.heading("Preview");
                    ui.horizontal_wrapped(|ui| {
                        ui.selectable_value(&mut self.view, View::Source, "Source");
                        for kind in MapKind::ALL {
                            ui.selectable_value(&mut self.view, View::Map(kind), kind.label());
                        }
                    });
                    ui.checkbox(&mut self.fit, "Fit to window");
                    ui.add_enabled(
                        !self.fit,
                        egui::Slider::new(&mut self.zoom, 0.05..=8.0)
                            .logarithmic(true)
                            .text("Zoom"),
                    );
                    if ui
                        .checkbox(&mut self.fast_preview, "Fast preview")
                        .on_hover_text(format!(
                            "Preview from a copy downscaled to {PREVIEW_MAX}px. \
                             Saving always uses the full resolution."
                        ))
                        .changed()
                    {
                        changed = true;
                    }
                });
            });

        if changed {
            self.touch();
        }
    }

    fn central_panel(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            let Some(loaded) = &self.loaded else {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        egui::RichText::new("Drop an image here\nor use Open…")
                            .size(20.0)
                            .weak(),
                    );
                });
                return;
            };

            let tex = match self.view {
                View::Source => Some(&loaded.source_tex),
                View::Map(kind) => loaded.texture(kind),
            };
            let Some(tex) = tex else {
                ui.centered_and_justified(|ui| ui.spinner());
                return;
            };

            let native = tex.size_vec2();
            let size = if self.fit {
                let avail = ui.available_size();
                let scale = (avail.x / native.x).min(avail.y / native.y).min(1.0);
                native * scale
            } else {
                native * self.zoom
            };

            egui::ScrollArea::both()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    ui.centered_and_justified(|ui| {
                        ui.add(
                            egui::Image::new((tex.id(), size))
                                .texture_options(egui::TextureOptions::NEAREST),
                        );
                    });
                });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                if let Some(loaded) = &self.loaded {
                    let (w, h) = loaded.source.dimensions();
                    let name = loaded
                        .path
                        .as_ref()
                        .and_then(|p| p.file_name())
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "untitled".to_owned());
                    ui.label(format!("{name} — {w}×{h}"));
                    ui.separator();
                    ui.label(format!("generated in {:.0} ms", self.last_gen_ms));
                    ui.separator();
                }
                ui.label(&self.status);
                if self.pending {
                    ui.spinner();
                }
            });
        });
    }
}

// ---------------------------------------------------------------- logic

impl App {
    /// Mark the settings dirty; the debounce decides when work actually starts.
    fn touch(&mut self) {
        self.dirty_since = Some(Instant::now());
    }

    fn maybe_dispatch(&mut self, ctx: &egui::Context) {
        let Some(since) = self.dirty_since else {
            return;
        };
        let waited = since.elapsed();
        if waited < DEBOUNCE {
            ctx.request_repaint_after(DEBOUNCE - waited);
            return;
        }
        self.dirty_since = None;

        let Some(loaded) = &self.loaded else { return };
        let src = if self.fast_preview {
            loaded.preview_source.clone()
        } else {
            loaded.source.clone()
        };

        self.generation += 1;
        self.pending = true;
        self.worker.request(Request {
            generation: self.generation,
            src,
            settings: self.settings,
        });
    }

    fn poll_worker(&mut self, ctx: &egui::Context) {
        let Some(response) = self.worker.poll() else {
            return;
        };
        // A newer request is already in flight; this result is stale.
        if response.generation != self.generation {
            return;
        }
        self.pending = false;
        self.last_gen_ms = response.millis;

        if let Some(loaded) = &mut self.loaded {
            loaded.maps = response
                .maps
                .iter()
                .map(|(kind, img)| {
                    let tex = ctx.load_texture(
                        kind.suffix(),
                        to_color_image(img),
                        egui::TextureOptions::LINEAR,
                    );
                    (*kind, tex)
                })
                .collect();
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let (open, save) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::O),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::S),
            )
        });
        if open {
            self.open_dialog(ctx);
        }
        if save && self.loaded.is_some() {
            self.save_dialog();
        }
    }

    fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(path) = dropped.first().map(|f| f.path().to_path_buf()) {
            self.load(ctx, &path);
        }
    }

    fn open_dialog(&mut self, ctx: &egui::Context) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("Images", &batch::IMAGE_EXTENSIONS)
            .pick_file()
        {
            self.load(ctx, &path);
        }
    }

    fn load(&mut self, ctx: &egui::Context, path: &Path) {
        match image::open(path) {
            Ok(img) => {
                let source = img.to_rgba8();
                let preview_source = downscale(&source, PREVIEW_MAX);
                let source_tex = ctx.load_texture(
                    "source",
                    to_color_image(&source),
                    egui::TextureOptions::LINEAR,
                );
                self.loaded = Some(Loaded {
                    path: Some(path.to_path_buf()),
                    source: Arc::new(source),
                    preview_source: Arc::new(preview_source),
                    source_tex,
                    maps: Vec::new(),
                });
                self.status = format!("Loaded {}", path.display());
                // Nothing to wait for on a fresh load.
                self.dirty_since = Some(Instant::now() - DEBOUNCE);
            }
            Err(err) => self.status = format!("Failed to load {}: {err}", path.display()),
        }
    }

    fn stem(&self) -> String {
        self.loaded
            .as_ref()
            .and_then(|l| l.path.as_ref())
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "image".to_owned())
    }

    /// Save the map currently on screen, always at the source resolution.
    fn save_dialog(&mut self) {
        let Some(loaded) = &self.loaded else { return };
        let View::Map(kind) = self.view else {
            self.status = "Select a generated map to save.".to_owned();
            return;
        };

        let Some(path) = rfd::FileDialog::new()
            .add_filter("PNG", &["png"])
            .add_filter("TGA", &["tga"])
            .add_filter("JPEG", &["jpg"])
            .set_file_name(format!("{}_{}.png", self.stem(), kind.suffix()))
            .save_file()
        else {
            return;
        };

        let hm = normalmap::height_map(&loaded.source, &self.settings);
        let out = normalmap::render(&hm, &self.settings, kind);
        self.status = match out.save(&path) {
            Ok(()) => format!("Saved {}", path.display()),
            Err(err) => format!("Failed to save {}: {err}", path.display()),
        };
    }

    /// Write every map for the loaded image into a folder in one go.
    fn export_all(&mut self) {
        let Some(loaded) = &self.loaded else { return };
        let Some(dir) = rfd::FileDialog::new().pick_folder() else {
            return;
        };

        let stem = self.stem();
        let maps = normalmap::generate(&loaded.source, &self.settings, &MapKind::ALL);
        for (kind, img) in maps {
            let dest = dir.join(format!("{stem}_{}.png", kind.suffix()));
            if let Err(err) = img.save(&dest) {
                self.status = format!("Failed to save {}: {err}", dest.display());
                return;
            }
        }
        self.status = format!("Exported {} maps to {}", MapKind::ALL.len(), dir.display());
    }

    fn save_preset(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Preset", &["json"])
            .set_file_name("normalmapper-preset.json")
            .save_file()
        else {
            return;
        };
        let json = serde_json::to_string_pretty(&self.settings).unwrap_or_default();
        self.status = match std::fs::write(&path, json) {
            Ok(()) => format!("Saved preset {}", path.display()),
            Err(err) => format!("Failed to save preset: {err}"),
        };
    }

    fn load_preset(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Preset", &["json"])
            .pick_file()
        else {
            return;
        };
        match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|json| serde_json::from_str::<Settings>(&json).map_err(|e| e.to_string()))
        {
            Ok(settings) => {
                self.settings = settings;
                self.touch();
                self.status = format!("Loaded preset {}", path.display());
            }
            Err(err) => self.status = format!("Failed to load preset: {err}"),
        }
    }
}

fn to_color_image(img: &RgbaImage) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied(
        [img.width() as usize, img.height() as usize],
        img.as_raw(),
    )
}

/// Shrink `img` so its longest side is at most `max` pixels.
fn downscale(img: &RgbaImage, max: u32) -> RgbaImage {
    let (w, h) = img.dimensions();
    if w <= max && h <= max {
        return img.clone();
    }
    let scale = max as f32 / w.max(h) as f32;
    let (nw, nh) = (
        ((w as f32 * scale).round() as u32).max(1),
        ((h as f32 * scale).round() as u32).max(1),
    );
    image::imageops::resize(img, nw, nh, image::imageops::FilterType::Triangle)
}
