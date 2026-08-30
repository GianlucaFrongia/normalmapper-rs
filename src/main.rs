#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod normalmap;

use std::path::{Path, PathBuf};
use std::time::Instant;

use eframe::egui;
use image::RgbaImage;
use normalmap::{HeightSource, Kernel, Settings};

const PREVIEW_MAX: u32 = 1024;

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
            Ok(Box::<App>::default())
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
    Height,
    Normal,
}

/// The currently loaded image plus everything derived from it.
struct Loaded {
    path: Option<PathBuf>,
    source: RgbaImage,
    /// Downscaled copy the live preview is generated from.
    preview_source: RgbaImage,
    source_tex: egui::TextureHandle,
    height_tex: Option<egui::TextureHandle>,
    normal_tex: Option<egui::TextureHandle>,
}

struct App {
    settings: Settings,
    loaded: Option<Loaded>,
    view: View,
    zoom: f32,
    fit: bool,
    /// Preview from a downscaled source while dragging sliders.
    fast_preview: bool,
    dirty: bool,
    last_gen_ms: f32,
    status: String,
}

impl Default for App {
    fn default() -> Self {
        Self {
            settings: Settings::default(),
            loaded: None,
            view: View::Normal,
            zoom: 1.0,
            fit: true,
            fast_preview: true,
            dirty: false,
            last_gen_ms: 0.0,
            status: "Open an image, or drop one onto the window.".to_owned(),
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        self.handle_dropped_files(&ctx);
        self.handle_shortcuts(&ctx);
        if self.dirty {
            self.regenerate(&ctx);
        }

        self.top_bar(ui, &ctx);
        self.side_panel(ui);
        self.status_bar(ui);
        self.central_panel(ui);
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
                    if ui.button("Save normal map…").on_hover_text("Ctrl+S").clicked() {
                        self.save_dialog(View::Normal);
                    }
                    if ui.button("Save height map…").clicked() {
                        self.save_dialog(View::Height);
                    }
                    if ui.button("Reload").clicked()
                        && let Some(path) = self.loaded.as_ref().and_then(|l| l.path.clone())
                    {
                        self.load(ctx, &path);
                    }
                });

                ui.separator();
                if ui.button("Reset settings").clicked() {
                    self.settings = Settings::default();
                    self.dirty = true;
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    egui::global_theme_preference_switch(ui);
                });
            });
        });
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) {
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
                        .on_hover_text("Gaussian pre-blur; smooths out noise before differentiating");
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

                    if self.settings != before {
                        self.dirty = true;
                    }

                    ui.add_space(10.0);
                    ui.heading("Preview");
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.view, View::Source, "Source");
                        ui.selectable_value(&mut self.view, View::Height, "Height");
                        ui.selectable_value(&mut self.view, View::Normal, "Normal");
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
                        self.dirty = true;
                    }
                });
            });
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
                View::Height => loaded.height_tex.as_ref(),
                View::Normal => loaded.normal_tex.as_ref(),
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
            });
        });
    }
}

// ---------------------------------------------------------------- logic

impl App {
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
            self.save_dialog(View::Normal);
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
            .add_filter(
                "Images",
                &["png", "jpg", "jpeg", "bmp", "tga", "tif", "tiff", "webp", "gif"],
            )
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
                    source,
                    preview_source,
                    source_tex,
                    height_tex: None,
                    normal_tex: None,
                });
                self.status = format!("Loaded {}", path.display());
                self.dirty = true;
            }
            Err(err) => self.status = format!("Failed to load {}: {err}", path.display()),
        }
    }

    /// Rebuild the height and normal previews from the current settings.
    fn regenerate(&mut self, ctx: &egui::Context) {
        self.dirty = false;
        let Some(loaded) = &mut self.loaded else {
            return;
        };

        let src = if self.fast_preview {
            &loaded.preview_source
        } else {
            &loaded.source
        };

        let started = Instant::now();
        let hm = normalmap::height_map(src, &self.settings);
        let normal = normalmap::normal_map(&hm, &self.settings);
        self.last_gen_ms = started.elapsed().as_secs_f32() * 1000.0;

        loaded.height_tex = Some(ctx.load_texture(
            "height",
            to_color_image(&hm.to_rgba()),
            egui::TextureOptions::LINEAR,
        ));
        loaded.normal_tex = Some(ctx.load_texture(
            "normal",
            to_color_image(&normal),
            egui::TextureOptions::LINEAR,
        ));
    }

    fn save_dialog(&mut self, what: View) {
        let Some(loaded) = &self.loaded else { return };

        let stem = loaded
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "image".to_owned());
        let suffix = if what == View::Height { "height" } else { "normal" };

        let Some(path) = rfd::FileDialog::new()
            .add_filter("PNG", &["png"])
            .add_filter("TGA", &["tga"])
            .add_filter("JPEG", &["jpg"])
            .set_file_name(format!("{stem}_{suffix}.png"))
            .save_file()
        else {
            return;
        };

        // Always export at the source resolution, whatever the preview used.
        let hm = normalmap::height_map(&loaded.source, &self.settings);
        let out = if what == View::Height {
            hm.to_rgba()
        } else {
            normalmap::normal_map(&hm, &self.settings)
        };

        self.status = match out.save(&path) {
            Ok(()) => format!("Saved {}", path.display()),
            Err(err) => format!("Failed to save {}: {err}", path.display()),
        };
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
