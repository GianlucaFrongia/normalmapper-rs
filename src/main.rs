#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod batch;
mod light;
mod normalmap;
mod relief;
mod theme;
mod worker;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use image::RgbaImage;

use batch::Batch;
use light::LightPreview;
use normalmap::{HeightSource, Kernel, MapKind, Settings, Shading};
use relief::{Relief, Side, Tool};
use worker::{Request, Worker};

const PREVIEW_MAX: u32 = 1024;
/// How long the settings have to sit still before we kick off a regeneration.
const DEBOUNCE: Duration = Duration::from_millis(80);
const SETTINGS_KEY: &str = "settings";
const PIXEL_MODE_KEY: &str = "pixel_mode";
/// Longest side of the images the lit preview shades, in pixels.
const LIGHT_MAX: u32 = 512;
const ZOOM_MIN: f32 = 1.0 / 64.0;
const ZOOM_MAX: f32 = 64.0;
/// Multiplier for one press of the zoom buttons or keys.
const ZOOM_STEP: f32 = 1.25;
/// Power-of-two zoom rungs, the ladder pixel-art editors use.
const ZOOM_LADDER: [f32; 13] = [
    1.0 / 64.0,
    1.0 / 32.0,
    1.0 / 16.0,
    1.0 / 8.0,
    1.0 / 4.0,
    1.0 / 2.0,
    1.0,
    2.0,
    4.0,
    8.0,
    16.0,
    32.0,
    64.0,
];
/// Scroll units per zoom rung, so one wheel notch is one step.
const SCROLL_NOTCH: f32 = 40.0;
/// Show the texel grid from this zoom up.
const GRID_FROM: f32 = 8.0;

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
    theme::install(ctx);
    ctx.all_styles_mut(|style| {
        style.spacing.slider_width = 160.0;
    });
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Source,
    Map(MapKind),
    /// Painting one side's height canvas.
    Paint(Side),
    /// The normal map baked from the four side canvases.
    Relief,
}

/// The currently loaded image plus everything derived from it.
struct Loaded {
    path: Option<PathBuf>,
    source: Arc<RgbaImage>,
    /// Downscaled copy the live preview is generated from.
    preview_source: Arc<RgbaImage>,
    source_tex: egui::TextureHandle,
    /// The same pixels, uploaded unfiltered, for the relief canvas to sit on.
    /// egui takes a texture's filter from the handle rather than from the
    /// `Image` widget, so a nearest copy has to exist as its own texture.
    reference_tex: egui::TextureHandle,
    maps: Vec<(MapKind, egui::TextureHandle)>,
    /// Small copies of the maps, kept for the lit preview.
    shading: Option<Shading>,
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
    /// Image centre offset from the viewport centre, in screen pixels.
    pan: egui::Vec2,
    fit: bool,
    /// Preview from a downscaled source while dragging sliders.
    fast_preview: bool,
    /// Integer zoom rungs, nearest sampling and a texel grid.
    pixel_mode: bool,
    scroll_accum: f32,
    /// Set when the settings change; the request goes out once it stops moving.
    dirty_since: Option<Instant>,
    worker: Worker,
    generation: u64,
    pending: bool,
    last_gen_ms: f32,
    batch: Batch,
    light: LightPreview,
    /// The four hand-painted edge-height canvases and what they bake to.
    relief: Relief,
    /// Bumped on every finished generation, so the lit preview can tell.
    map_revision: u64,
    /// What the lit preview was last handed, so it knows when that changed.
    lit_key: (bool, u64, u64),
    lit_revision: u64,
    /// The texel under the cursor on the paint canvas, for the status bar.
    hover_texel: Option<(i32, i32)>,
    status: String,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let settings = cc
            .storage
            .and_then(|s| s.get_string(SETTINGS_KEY))
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();
        let pixel_mode = cc
            .storage
            .and_then(|s| s.get_string(PIXEL_MODE_KEY))
            .map(|v| v == "true")
            .unwrap_or(false);

        Self {
            settings,
            loaded: None,
            view: View::Map(MapKind::Normal),
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            fit: true,
            fast_preview: true,
            pixel_mode,
            scroll_accum: 0.0,
            dirty_since: None,
            worker: Worker::spawn(cc.egui_ctx.clone()),
            generation: 0,
            pending: false,
            last_gen_ms: 0.0,
            batch: Batch::default(),
            light: LightPreview::default(),
            relief: Relief::default(),
            map_revision: 0,
            lit_key: (false, u64::MAX, u64::MAX),
            lit_revision: 0,
            hover_texel: None,
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

        // Rebake the relief before anything draws it, so a stroke and the
        // light that answers it land in the same frame.
        let albedo = self.loaded.as_ref().map(|l| l.preview_source.clone());
        self.relief.sync(&ctx, &self.settings, albedo.as_deref());

        // The relief is a deliberate statement about depth and the image maps
        // are a guess, so once anything has been painted the light shows the
        // relief. An empty relief keeps out of the way entirely.
        let from_relief = self.relief.has_content();
        let key = (from_relief, self.map_revision, self.relief.revision);
        if key != self.lit_key {
            self.lit_key = key;
            self.lit_revision += 1;
        }

        self.top_bar(ui, &ctx);
        self.side_panel(ui);
        self.status_bar(ui);

        // Split the borrow so the column can hold the maps while it draws, and
        // draw it before the central panel: the maps take the width that is
        // left, rather than the preview taking what the maps did not want.
        let Self {
            light,
            loaded,
            relief,
            lit_revision,
            settings,
            ..
        } = self;
        let shading = if from_relief {
            relief.baked().map(|b| &b.shading)
        } else {
            loaded.as_ref().and_then(|l| l.shading.as_ref())
        };
        light.panel(
            ui,
            &ctx,
            shading,
            if from_relief {
                "painted relief"
            } else {
                "image"
            },
            *lit_revision,
            settings.flip_y,
        );

        self.central_panel(ui);
        self.batch.ui(&ctx, &self.settings);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if let Ok(json) = serde_json::to_string(&self.settings) {
            storage.set_string(SETTINGS_KEY, json);
        }
        storage.set_string(PIXEL_MODE_KEY, self.pixel_mode.to_string());
    }
}

// ---------------------------------------------------------------- UI

impl App {
    fn top_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::Panel::top("top")
            .frame(theme::bar(ui, true))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    theme::mark(ui);
                    ui.label(
                        egui::RichText::new("Normal Mapper").color(theme::palette(ui).text_strong),
                    );
                    ui.separator();
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
                    if theme::tab(ui, self.light.open, "Lit", true)
                        .on_hover_text(
                            "The lit preview column — the only honest way to judge a map",
                        )
                        .clicked()
                    {
                        self.light.open = !self.light.open;
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
                        ui.separator();
                        self.zoom_controls(ui);
                    });
                });
            });
    }

    fn side_panel(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        let mut zoom_request = None;
        let mut snap_zoom = false;
        let mut refilter = false;
        egui::Panel::left("settings")
            .resizable(false)
            .exact_size(280.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let before = self.settings;
                    let s = &mut self.settings;

                    ui.add_space(6.0);
                    theme::section(ui, "Height");
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
                    theme::section(ui, "Normals");
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
                    theme::section(ui, "Ambient occlusion");
                    ui.add(egui::Slider::new(&mut s.ao_radius, 0.0..=64.0).text("Radius"))
                        .on_hover_text("How far to search for occluders. 0 disables AO");
                    ui.add(egui::Slider::new(&mut s.ao_strength, 0.0..=2.0).text("Amount"));

                    ui.add_space(10.0);
                    theme::section(ui, "Roughness");
                    ui.add(egui::Slider::new(&mut s.roughness_base, 0.0..=1.0).text("Base"))
                        .on_hover_text("Roughness of perfectly flat areas");
                    ui.add(egui::Slider::new(&mut s.roughness_detail, 0.0..=32.0).text("Detail"))
                        .on_hover_text("How strongly fine height detail raises roughness");
                    ui.checkbox(&mut s.roughness_invert, "Invert");

                    changed = self.settings != before;

                    ui.add_space(10.0);
                    theme::section(ui, "Preview");
                    ui.horizontal_wrapped(|ui| {
                        if theme::tab(ui, self.view == View::Source, "Source", true).clicked() {
                            self.view = View::Source;
                        }
                        for kind in MapKind::ALL {
                            let on = self.view == View::Map(kind);
                            if theme::tab(ui, on, kind.label(), true).clicked() {
                                self.view = View::Map(kind);
                            }
                        }
                    });
                    if ui.checkbox(&mut self.fit, "Fit to window").changed() {
                        self.pan = egui::Vec2::ZERO;
                    }
                    let mut zoom = self.zoom;
                    if ui
                        .add(
                            egui::Slider::new(&mut zoom, ZOOM_MIN..=ZOOM_MAX)
                                .logarithmic(true)
                                .text("Zoom"),
                        )
                        .changed()
                    {
                        zoom_request = Some(zoom);
                    }
                    if ui
                        .checkbox(&mut self.pixel_mode, "Pixel art mode")
                        .on_hover_text(
                            "Power-of-two zoom rungs, nearest sampling at every zoom, \
                             a texel grid, and fit is allowed to magnify.",
                        )
                        .changed()
                    {
                        snap_zoom = true;
                        refilter = true;
                    }
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

                    self.relief_controls(ui);
                });
            });

        if refilter {
            // The filter is baked into the texture, so the mode switch means
            // uploading the source again and regenerating the maps.
            if let Some(loaded) = &mut self.loaded {
                loaded.source_tex.set(
                    to_color_image(&loaded.source),
                    image_filter(self.pixel_mode),
                );
            }
            self.touch();
        }
        if snap_zoom {
            let zoom = self.zoom;
            self.zoom_to(zoom);
        }
        if let Some(zoom) = zoom_request {
            self.zoom_to(zoom);
        }
        if changed {
            self.touch();
        }
    }

    /// The relief: four canvases, seven tools, and the knobs that decide what
    /// a stroke means. It bakes on every change, so there is no Apply button —
    /// the lit column on the right is the Apply button.
    fn relief_controls(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        theme::section(ui, "Relief").on_hover_text(
            "Draw how high each texel's four edges are. Right minus left is the \
             horizontal slope, bottom minus top the vertical one — that is the \
             normal, with nothing inferred from the picture.",
        );

        let painting = matches!(self.view, View::Paint(_));
        ui.horizontal_wrapped(|ui| {
            for side in Side::ALL {
                let drawn = self.relief.drawn(side);
                if theme::tab(ui, self.view == View::Paint(side), side.label(), drawn)
                    .on_hover_text(format!(
                        "Paint how high every texel's {} edge is",
                        side.label().to_lowercase()
                    ))
                    .clicked()
                {
                    self.show_side(side);
                }
            }
            if theme::tab(
                ui,
                self.view == View::Relief,
                "Baked",
                self.relief.has_content(),
            )
            .on_hover_text("The normal map the four canvases bake to")
            .clicked()
            {
                self.view = View::Relief;
                self.fit = true;
                self.pan = egui::Vec2::ZERO;
            }
        });

        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            for tool in Tool::ALL {
                if theme::tool(
                    ui,
                    self.relief.tool == tool,
                    true,
                    tool.icon(),
                    tool.label(),
                )
                .on_hover_text(tool.hint())
                .clicked()
                {
                    self.relief.tool = tool;
                    // Picking up a tool means you want to draw, so open the
                    // canvas it would draw on rather than just arming it.
                    if !painting {
                        self.show_side(self.relief.side);
                    }
                }
            }
        });

        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for i in 0..relief::STEPS {
                let value = i as f32 * relief::STEP;
                let selected = (self.relief.value - value).abs() < relief::STEP / 2.0;
                if theme::swatch(ui, selected, value)
                    .on_hover_text(format!(
                        "{:.0}% — the grey this height bakes to",
                        value * 100.0
                    ))
                    .clicked()
                {
                    self.relief.value = value;
                }
            }
        });
        ui.add(
            egui::Slider::new(&mut self.relief.value, 0.0..=1.0)
                .step_by(relief::STEP as f64)
                .custom_formatter(|v, _| format!("{:.0}%", v * 100.0))
                .custom_parser(|s| {
                    s.trim()
                        .trim_end_matches('%')
                        .parse::<f64>()
                        .ok()
                        .map(|v| v / 100.0)
                })
                .text("Height"),
        )
        .on_hover_text("What the brush declares an edge's height to be. 50% is flat.");
        ui.add(egui::Slider::new(&mut self.relief.brush, 1..=8).text("Brush"));
        if ui
            .add(
                egui::Slider::new(&mut self.relief.depth, 0.05..=20.0)
                    .logarithmic(true)
                    .text("Depth"),
            )
            .on_hover_text("How steep a given difference in height reads. The relief keeps its own, so the image Strength slider cannot move it.")
            .changed()
        {
            self.relief.touch();
        }

        ui.add_enabled_ui(self.loaded.is_some(), |ui| {
            ui.add(
                egui::Slider::new(&mut self.relief.reference, 0.0..=1.0)
                    .fixed_decimals(2)
                    .text("Reference"),
            )
            .on_hover_text("The loaded image, behind the canvas, to draw over. 0 hides it.");
            ui.add(
                egui::Slider::new(&mut self.relief.opacity, 0.0..=1.0)
                    .fixed_decimals(2)
                    .text("Canvas"),
            )
            .on_hover_text("Fade the heights back to check them against the art underneath.");
        });

        ui.horizontal(|ui| {
            if ui
                .add_enabled(self.relief.can_undo(), egui::Button::new("Undo"))
                .on_hover_text("Ctrl+Z")
                .clicked()
            {
                self.relief.undo();
                self.show_side(self.relief.side);
            }
            if ui
                .add_enabled(self.relief.can_redo(), egui::Button::new("Redo"))
                .on_hover_text("Ctrl+Shift+Z")
                .clicked()
            {
                self.relief.redo();
                self.show_side(self.relief.side);
            }
            let side = self.relief.side;
            if ui
                .add_enabled(self.relief.drawn(side), egui::Button::new("Clear"))
                .on_hover_text(format!("Erase the {} canvas", side.label().to_lowercase()))
                .clicked()
            {
                self.relief.clear_side(side);
            }
        });

        let (cw, ch) = self.relief.dims();
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut self.relief.new_size[0]).range(1..=relief::MAX_SIDE));
            ui.label("×");
            ui.add(egui::DragValue::new(&mut self.relief.new_size[1]).range(1..=relief::MAX_SIDE));
            let [w, h] = self.relief.new_size;
            if ui
                .add_enabled((w, h) != (cw, ch), egui::Button::new("Resize"))
                .on_hover_text("Keeps whatever still fits inside the new canvas")
                .clicked()
            {
                self.relief.resize(w, h);
            }
        });
        if let Some(loaded) = &self.loaded {
            let (iw, ih) = loaded.source.dimensions();
            let fitted = relief::fitted_dims(iw, ih);
            if fitted != (cw, ch) && ui.button("Match the image").clicked() {
                self.relief.fit_to(iw, ih);
            }
        }
        if painting {
            ui.label(
                egui::RichText::new("Drag to paint · right-drag to pan")
                    .small()
                    .color(theme::palette(ui).text_weak),
            );
        }
    }

    /// Show one side's canvas, framed: the canvas is rarely the size of the
    /// image, so arriving at last frame's zoom would land you off the edge.
    fn show_side(&mut self, side: Side) {
        self.relief.side = side;
        self.view = View::Paint(side);
        self.fit = true;
        self.pan = egui::Vec2::ZERO;
    }

    fn zoom_controls(&mut self, ui: &mut egui::Ui) {
        // Laid out right-to-left, so this reads backwards on screen.
        ui.add_enabled_ui(self.loaded.is_some(), |ui| {
            // The stepper is one control, so it gets one frame around it.
            theme::group(ui).show(ui, |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    if ui.button("Fit").on_hover_text("F").clicked() {
                        self.fit = true;
                        self.pan = egui::Vec2::ZERO;
                    }
                    if ui.button("1:1").on_hover_text("0").clicked() {
                        self.zoom_to(1.0);
                    }
                    ui.separator();
                    if ui.button("+").on_hover_text("+ / scroll up").clicked() {
                        self.zoom_step(1);
                    }
                    ui.label(theme::num(format!("{:>4.0}%", self.zoom * 100.0)));
                    if ui.button("−").on_hover_text("- / scroll down").clicked() {
                        self.zoom_step(-1);
                    }
                });
            });
        });
    }

    fn central_panel(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            // How big the thing on screen is, in its own pixels. The relief
            // canvases exist whether or not an image is loaded and have their
            // own size; the image tabs are measured in source pixels, so zoom
            // means the same on every tab even when fast preview works from a
            // small copy.
            let native = match self.view {
                View::Paint(_) | View::Relief => Some(self.relief.size_vec2()),
                View::Source | View::Map(_) => self.loaded.as_ref().map(|l| {
                    let (w, h) = l.source.dimensions();
                    egui::vec2(w as f32, h as f32)
                }),
            };
            let Some(native) = native else {
                ui.centered_and_justified(|ui| {
                    ui.label(
                        egui::RichText::new(
                            "Drop an image here, use Open…\nor pick a Relief side and start drawing",
                        )
                        .size(20.0)
                        .weak(),
                    );
                });
                return;
            };

            let (viewport, response) =
                ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());

            if self.fit {
                self.zoom = fit_zoom(viewport, native, self.pixel_view());
                self.pan = egui::Vec2::ZERO;
            }
            self.handle_view_input(ui, &response, viewport, native);
            self.clamp_pan(viewport, native);

            let size = native * self.zoom;
            // Land the image on whole device pixels, or every texel edge blurs.
            let ppp = ui.ctx().pixels_per_point();
            let snap = |v: f32| (v * ppp).round() / ppp;
            let centre = viewport.center() + self.pan;
            let rect = egui::Rect::from_min_size(
                egui::pos2(snap(centre.x - size.x / 2.0), snap(centre.y - size.y / 2.0)),
                size,
            );
            // The stroke is taken before the canvas is drawn, and the canvas
            // texture rebuilt in between. Otherwise every texel you lay down
            // appears one frame late — and if the pointer is not moving there
            // is no next frame, so painting over a texel looks like it did
            // nothing at all.
            if matches!(self.view, View::Paint(_)) {
                self.handle_paint(ui, &response, rect, viewport);
                self.relief.refresh_canvas(ui.ctx());
            } else {
                self.hover_texel = None;
            }

            // Copying the id out ends the borrow of `self`.
            let tex_id = match self.view {
                View::Paint(side) => self.relief.canvas_texture(side),
                View::Relief => self.relief.normal_texture(),
                View::Source => self.loaded.as_ref().map(|l| l.source_tex.id()),
                View::Map(kind) => self
                    .loaded
                    .as_ref()
                    .and_then(|l| l.texture(kind))
                    .map(|t| t.id()),
            };

            theme::well(ui, viewport);
            let Some(tex_id) = tex_id else {
                // Generating: the viewport is already allocated, so the
                // spinner goes into it rather than laying out beside it.
                ui.put(viewport, egui::Spinner::new());
                return;
            };
            // Without this a transparent sprite reads as a dark silhouette.
            theme::paint_checkerboard(ui, rect.intersect(viewport));
            theme::sprite_frame(ui, rect.intersect(viewport));

            // On a relief view the image goes in behind: unpainted texels are
            // transparent, so the sprite shows through everywhere you have not
            // spoken yet, and that is exactly where you need to see it.
            let relief_view = matches!(self.view, View::Paint(_) | View::Relief);
            if relief_view
                && self.relief.reference > 0.0
                && let Some(loaded) = &self.loaded
            {
                // The image itself, untouched, stretched onto the canvas.
                egui::Image::new((loaded.reference_tex.id(), size))
                    .tint(alpha_tint(self.relief.reference))
                    .paint_at(ui, rect);
            }

            // No `texture_options` here: egui reads the filter off the
            // texture handle and ignores what the widget asks for.
            let mut image = egui::Image::new((tex_id, size));
            if relief_view {
                image = image.tint(alpha_tint(self.relief.opacity));
            }
            image.paint_at(ui, rect);

            // A paint canvas gets its grid whatever the mode: you cannot aim a
            // pencil at a texel you cannot see the edges of.
            let grid = if matches!(self.view, View::Paint(_)) {
                self.zoom >= 4.0
            } else {
                self.pixel_mode && self.zoom >= GRID_FROM
            };
            if grid {
                paint_texel_grid(ui, rect, viewport, self.zoom);
            }
        });
    }

    /// Painting on the canvas: the primary button lays texels down, and the
    /// brush is followed from wherever it was last so a fast drag draws a line
    /// rather than a dotted trail.
    fn handle_paint(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        rect: egui::Rect,
        viewport: egui::Rect,
    ) {
        let texel_at = |pos: egui::Pos2| {
            let t = (pos - rect.min) / self.zoom;
            (t.x.floor() as i32, t.y.floor() as i32)
        };

        let (pointer, primary_down) = ui.ctx().input(|i| {
            (
                i.pointer.interact_pos(),
                i.pointer.button_down(egui::PointerButton::Primary),
            )
        });

        // Painting from the canvas keeps going while the button is held, even
        // once the pointer has wandered off the edge of the image.
        let painting = primary_down && response.is_pointer_button_down_on();
        if painting {
            if !self.relief.stroking() {
                self.relief.begin_stroke();
            }
            if let Some(pos) = pointer {
                let (x, y) = texel_at(pos);
                self.relief.stroke_to(x, y);
            }
        } else if self.relief.stroking() {
            self.relief.end_stroke();
        }
        if painting {
            // The lit column and the bake are drawn before this, so they can
            // only answer the stroke on the next frame. Ask for it.
            ui.ctx().request_repaint();
        }

        let hover = pointer
            .filter(|p| viewport.contains(*p) && rect.contains(*p))
            .map(texel_at);
        if hover != self.hover_texel {
            // The status bar is drawn before this, so the readout it shows is
            // a frame behind; ask for that frame.
            self.hover_texel = hover;
            ui.ctx().request_repaint();
        }
        if hover.is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
        }
    }

    /// Scroll/pinch to zoom at the cursor, drag to pan, double-click to toggle.
    fn handle_view_input(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        viewport: egui::Rect,
        native: egui::Vec2,
    ) {
        // On a paint canvas the primary button belongs to the brush, so
        // panning moves to the right and middle buttons.
        let panning = if matches!(self.view, View::Paint(_)) {
            response.dragged_by(egui::PointerButton::Secondary)
                || response.dragged_by(egui::PointerButton::Middle)
        } else {
            response.dragged()
        };
        if panning {
            self.pan += response.drag_delta();
            self.fit = false;
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if response.hovered() && !matches!(self.view, View::Paint(_)) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }

        if response.double_clicked() && !matches!(self.view, View::Paint(_)) {
            if self.fit {
                let anchor = response.interact_pointer_pos().unwrap_or(viewport.center());
                self.zoom_at(1.0, anchor, viewport, native);
            } else {
                self.fit = true;
                self.pan = egui::Vec2::ZERO;
            }
        }

        if !response.hovered() {
            self.scroll_accum = 0.0;
            return;
        }
        let (scroll, pinch, cursor) = ui.ctx().input(|i| {
            (
                i.smooth_scroll_delta.y,
                i.zoom_delta(),
                i.pointer.hover_pos(),
            )
        });
        let anchor = cursor.unwrap_or(viewport.center());

        if self.pixel_view() {
            // Smoothed scroll arrives over several frames; accumulate so one
            // notch is one rung instead of four.
            self.scroll_accum += scroll;
            while self.scroll_accum.abs() >= SCROLL_NOTCH {
                let dir = self.scroll_accum.signum();
                self.scroll_accum -= dir * SCROLL_NOTCH;
                let target = ladder_step(self.zoom, dir as i32);
                self.zoom_at(target, anchor, viewport, native);
            }
            if (pinch - 1.0).abs() > 1e-4 {
                self.zoom_at(self.zoom * pinch, anchor, viewport, native);
            }
            return;
        }

        let factor = pinch * (scroll * 0.0025).exp();
        if (factor - 1.0).abs() > 1e-4 {
            self.zoom_at(self.zoom * factor, anchor, viewport, native);
        }
    }

    /// Whether the view on screen is texels rather than pixels. The relief
    /// canvases always are — a paint canvas you cannot aim at is no use — so
    /// they get the ladder and the magnifying fit whatever the mode says.
    fn pixel_view(&self) -> bool {
        self.pixel_mode || matches!(self.view, View::Paint(_) | View::Relief)
    }

    /// Clamp, and in a texel view snap down to the nearest rung.
    fn resolve_zoom(&self, zoom: f32) -> f32 {
        let zoom = zoom.clamp(ZOOM_MIN, ZOOM_MAX);
        if self.pixel_view() {
            ladder_snap(zoom)
        } else {
            zoom
        }
    }

    /// One notch in or out, following the ladder in pixel mode.
    fn zoom_step(&mut self, dir: i32) {
        let target = if self.pixel_view() {
            ladder_step(self.zoom, dir)
        } else if dir > 0 {
            self.zoom * ZOOM_STEP
        } else {
            self.zoom / ZOOM_STEP
        };
        self.zoom_to(target);
    }

    /// Zoom about the viewport centre; used by the buttons, keys and slider.
    fn zoom_to(&mut self, zoom: f32) {
        let zoom = self.resolve_zoom(zoom);
        // Keeping the centred point fixed is just a rescale of the pan.
        self.pan *= zoom / self.zoom;
        self.zoom = zoom;
        self.fit = false;
    }

    /// Zoom while keeping the image point under `anchor` under `anchor`.
    fn zoom_at(&mut self, zoom: f32, anchor: egui::Pos2, viewport: egui::Rect, native: egui::Vec2) {
        let zoom = self.resolve_zoom(zoom);
        let old_min = viewport.center() + self.pan - native * self.zoom / 2.0;
        let point = (anchor - old_min) / self.zoom;
        let new_min = anchor - point * zoom;
        self.pan = new_min + native * zoom / 2.0 - viewport.center();
        self.zoom = zoom;
        self.fit = false;
    }

    /// Stop the image being dragged completely off screen.
    fn clamp_pan(&mut self, viewport: egui::Rect, native: egui::Vec2) {
        const KEEP_VISIBLE: f32 = 48.0;
        let size = native * self.zoom;
        let limit = (size + viewport.size()) / 2.0 - egui::Vec2::splat(KEEP_VISIBLE);
        self.pan.x = self.pan.x.clamp(-limit.x.max(0.0), limit.x.max(0.0));
        self.pan.y = self.pan.y.clamp(-limit.y.max(0.0), limit.y.max(0.0));
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status")
            .frame(theme::bar(ui, false))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let p = theme::palette(ui);
                    if let Some(loaded) = &self.loaded {
                        let (w, h) = loaded.source.dimensions();
                        let name = loaded
                            .path
                            .as_ref()
                            .and_then(|p| p.file_name())
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| "untitled".to_owned());
                        ui.label(egui::RichText::new(name).color(p.text_strong));
                        ui.label(theme::num(format!("{w}×{h}")).color(p.text_weak));
                        ui.separator();
                        ui.label(egui::RichText::new("generated in").weak());
                        ui.label(
                            theme::num(format!("{:.0} ms", self.last_gen_ms)).color(p.accent_soft),
                        );
                        ui.separator();
                    }
                    if let View::Paint(side) = self.view {
                        let (cw, ch) = self.relief.dims();
                        ui.label(
                            egui::RichText::new(format!("{} canvas", side.label()))
                                .color(p.text_strong),
                        );
                        ui.label(theme::num(format!("{cw}×{ch}")).color(p.text_weak));
                        ui.separator();
                        match self.hover_texel {
                            Some((x, y)) => {
                                let height = match self.relief.get(side, x, y) {
                                    Some(v) => format!("{:.0}%", v as f32 / 2.55),
                                    // Unpainted, which bakes as flat but is not
                                    // the same as having been called flat.
                                    None => "—".to_owned(),
                                };
                                ui.label(theme::num(format!("{x},{y}")).color(p.text_weak));
                                ui.label(egui::RichText::new("height").weak());
                                ui.label(theme::num(height).color(p.accent_soft));
                            }
                            None => {
                                ui.label(
                                    egui::RichText::new("hover a texel to read its height").weak(),
                                );
                            }
                        }
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
        self.map_revision += 1;

        let fast = self.fast_preview;
        let filter = image_filter(self.pixel_mode);
        if let Some(loaded) = &mut self.loaded {
            loaded.maps = response
                .maps
                .iter()
                .map(|(kind, img)| {
                    let tex = ctx.load_texture(kind.suffix(), to_color_image(img), filter);
                    (*kind, tex)
                })
                .collect();

            let albedo = if fast {
                &loaded.preview_source
            } else {
                &loaded.source
            };
            loaded.shading = build_shading(&response.maps, albedo);
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let (open, save, redo, undo) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::O),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::S),
                // Tested before plain undo, or the shift would be ignored.
                i.consume_key(
                    egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                    egui::Key::Z,
                ),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z),
            )
        });
        if open {
            self.open_dialog(ctx);
        }
        if save {
            self.save_dialog();
        }
        if undo {
            self.relief.undo();
            self.show_side(self.relief.side);
        }
        if redo {
            self.relief.redo();
            self.show_side(self.relief.side);
        }
        self.handle_zoom_keys(ctx);
    }

    fn handle_zoom_keys(&mut self, ctx: &egui::Context) {
        let (zoom_in, zoom_out, actual, fit) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals),
                i.key_pressed(egui::Key::Minus),
                i.key_pressed(egui::Key::Num0),
                i.key_pressed(egui::Key::F),
            )
        });
        if zoom_in {
            self.zoom_step(1);
        }
        if zoom_out {
            self.zoom_step(-1);
        }
        if actual {
            self.zoom_to(1.0);
        }
        if fit {
            self.fit = true;
            self.pan = egui::Vec2::ZERO;
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
                let pixel_mode = self.pixel_mode;
                let source = img.to_rgba8();
                let (img_w, img_h) = source.dimensions();
                let preview_source = downscale(&source, PREVIEW_MAX);
                let source_tex =
                    ctx.load_texture("source", to_color_image(&source), image_filter(pixel_mode));
                // Sized like the preview copy: it only ever sits behind a
                // relief canvas, which is 512 texels at the most.
                let reference_tex = ctx.load_texture(
                    "reference",
                    to_color_image(&preview_source),
                    egui::TextureOptions::NEAREST,
                );
                self.loaded = Some(Loaded {
                    path: Some(path.to_path_buf()),
                    source: Arc::new(source),
                    preview_source: Arc::new(preview_source),
                    source_tex,
                    reference_tex,
                    maps: Vec::new(),
                    shading: None,
                });
                // An untouched relief takes the shape of the image; one that
                // has been drawn on is the user's work and keeps its size.
                if !self.relief.has_content() {
                    self.relief.fit_to(img_w, img_h);
                }
                // The image behind the canvas, and behind the lit preview,
                // just changed.
                self.relief.touch();
                self.status = format!("Loaded {}", path.display());
                self.fit = true;
                self.pan = egui::Vec2::ZERO;
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
        // Whatever is on screen is what gets saved — including one side's
        // heights, which is the only way to hand a relief to another tool.
        let suffix = match self.view {
            View::Map(kind) if self.loaded.is_some() => kind.suffix().to_owned(),
            View::Paint(side) => format!("relief_{}", side.suffix()),
            View::Relief => "relief_normal".to_owned(),
            View::Map(_) => return,
            View::Source => {
                self.status = "Select a map or a relief canvas to save.".to_owned();
                return;
            }
        };

        let Some(path) = rfd::FileDialog::new()
            .add_filter("PNG", &["png"])
            .add_filter("TGA", &["tga"])
            .add_filter("JPEG", &["jpg"])
            .set_file_name(format!("{}_{suffix}.png", self.stem()))
            .save_file()
        else {
            return;
        };

        let out = match self.view {
            // Maps are re-rendered at the source resolution, never taken from
            // the downscaled preview.
            View::Map(kind) => {
                let loaded = self.loaded.as_ref().expect("checked above");
                let hm = normalmap::height_map(&loaded.source, &self.settings);
                normalmap::render(&hm, &self.settings, kind)
            }
            View::Paint(side) => self.relief.side_image(side),
            View::Relief => match self.relief.baked() {
                Some(baked) => baked.normal.clone(),
                None => return,
            },
            View::Source => return,
        };
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
        let mut written = MapKind::ALL.len();
        // A painted relief is its own pair of maps, not a variant of the
        // generated ones, so it exports under its own names.
        if self.relief.has_content()
            && let Some(baked) = self.relief.baked()
        {
            for (name, img) in [
                ("relief_normal", &baked.normal),
                ("relief_height", &baked.height),
            ] {
                let dest = dir.join(format!("{stem}_{name}.png"));
                if let Err(err) = img.save(&dest) {
                    self.status = format!("Failed to save {}: {err}", dest.display());
                    return;
                }
                written += 1;
            }
        }
        self.status = format!("Exported {written} maps to {}", dir.display());
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

/// How an image texture is sampled. It has to be chosen when the texture is
/// uploaded — `Image::texture_options` is a no-op for a raw texture id — so it
/// follows the persisted mode rather than the current zoom.
fn image_filter(pixel_mode: bool) -> egui::TextureOptions {
    if pixel_mode {
        egui::TextureOptions::NEAREST
    } else {
        egui::TextureOptions::LINEAR
    }
}

/// A tint that only fades: white at full strength, so the image comes through
/// with its own colours.
fn alpha_tint(strength: f32) -> egui::Color32 {
    egui::Color32::from_white_alpha((strength.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// Scale that fits `native` inside `viewport`. Photos are never upscaled;
/// pixel art is, but only to a whole rung of the ladder.
fn fit_zoom(viewport: egui::Rect, native: egui::Vec2, pixel_mode: bool) -> f32 {
    let scale = (viewport.width() / native.x).min(viewport.height() / native.y);
    if pixel_mode {
        ladder_snap(scale.clamp(ZOOM_MIN, ZOOM_MAX))
    } else {
        scale.min(1.0).clamp(ZOOM_MIN, ZOOM_MAX)
    }
}

/// The largest rung at or below `zoom`.
fn ladder_snap(zoom: f32) -> f32 {
    ZOOM_LADDER
        .iter()
        .rev()
        .copied()
        .find(|&rung| rung <= zoom * 1.0001)
        .unwrap_or(ZOOM_MIN)
}

/// The next rung above or below `zoom`, saturating at the ends.
fn ladder_step(zoom: f32, dir: i32) -> f32 {
    if dir > 0 {
        ZOOM_LADDER
            .iter()
            .copied()
            .find(|&rung| rung > zoom * 1.0001)
            .unwrap_or(ZOOM_MAX)
    } else {
        ZOOM_LADDER
            .iter()
            .rev()
            .copied()
            .find(|&rung| rung < zoom * 0.9999)
            .unwrap_or(ZOOM_MIN)
    }
}

/// One hairline per texel edge, drawn only over the visible part of the image.
fn paint_texel_grid(ui: &egui::Ui, image: egui::Rect, viewport: egui::Rect, zoom: f32) {
    let visible = image.intersect(viewport);
    if !visible.is_positive() {
        return;
    }
    let painter = ui.painter_at(viewport);
    let stroke = egui::Stroke::new(1.0, ui.visuals().weak_text_color().gamma_multiply(0.35));

    let first = |lo: f32, origin: f32| ((lo - origin) / zoom).floor().max(0.0);
    let last = |hi: f32, origin: f32| ((hi - origin) / zoom).ceil();

    let mut i = first(visible.min.x, image.min.x);
    while i <= last(visible.max.x, image.min.x) {
        let x = image.min.x + i * zoom;
        painter.line_segment(
            [egui::pos2(x, visible.min.y), egui::pos2(x, visible.max.y)],
            stroke,
        );
        i += 1.0;
    }
    let mut j = first(visible.min.y, image.min.y);
    while j <= last(visible.max.y, image.min.y) {
        let y = image.min.y + j * zoom;
        painter.line_segment(
            [egui::pos2(visible.min.x, y), egui::pos2(visible.max.x, y)],
            stroke,
        );
        j += 1.0;
    }
}

/// Small copies of the maps for the lit preview, all at matching dimensions.
fn build_shading(maps: &[(MapKind, RgbaImage)], albedo: &RgbaImage) -> Option<Shading> {
    let find = |kind| maps.iter().find(|(k, _)| *k == kind).map(|(_, img)| img);
    let normal = find(MapKind::Normal)?;
    let ao = find(MapKind::Ao)?;
    let roughness = find(MapKind::Roughness)?;

    let (width, height) = fit_dims(normal.width(), normal.height(), LIGHT_MAX);
    Some(Shading {
        width,
        height,
        albedo: resize_to(albedo, width, height),
        normal: resize_to(normal, width, height),
        ao: resize_to(ao, width, height),
        roughness: resize_to(roughness, width, height),
    })
}

fn resize_to(img: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    if img.dimensions() == (w, h) {
        img.clone()
    } else {
        image::imageops::resize(img, w, h, image::imageops::FilterType::Triangle)
    }
}

/// Dimensions of `w`x`h` shrunk so its longest side is at most `max`.
fn fit_dims(w: u32, h: u32, max: u32) -> (u32, u32) {
    if w <= max && h <= max {
        return (w, h);
    }
    let scale = max as f32 / w.max(h) as f32;
    (
        ((w as f32 * scale).round() as u32).max(1),
        ((h as f32 * scale).round() as u32).max(1),
    )
}

fn to_color_image(img: &RgbaImage) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied(
        [img.width() as usize, img.height() as usize],
        img.as_raw(),
    )
}

/// Shrink `img` so its longest side is at most `max` pixels.
fn downscale(img: &RgbaImage, max: u32) -> RgbaImage {
    let (w, h) = fit_dims(img.width(), img.height(), max);
    resize_to(img, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_snaps_down_to_a_rung() {
        assert_eq!(ladder_snap(1.0), 1.0);
        assert_eq!(ladder_snap(3.9), 2.0);
        assert_eq!(ladder_snap(0.7), 0.5);
        assert_eq!(ladder_snap(0.001), ZOOM_MIN);
        assert_eq!(ladder_snap(1000.0), 64.0);
    }

    #[test]
    fn ladder_steps_move_exactly_one_rung() {
        assert_eq!(ladder_step(1.0, 1), 2.0);
        assert_eq!(ladder_step(1.0, -1), 0.5);
        // A value between rungs steps to the next one either way.
        assert_eq!(ladder_step(3.0, 1), 4.0);
        assert_eq!(ladder_step(3.0, -1), 2.0);
        // And it saturates rather than running off the end.
        assert_eq!(ladder_step(64.0, 1), ZOOM_MAX);
        assert_eq!(ladder_step(ZOOM_MIN, -1), ZOOM_MIN);
    }

    #[test]
    fn fit_dims_only_shrinks() {
        assert_eq!(fit_dims(32, 32, 512), (32, 32));
        assert_eq!(fit_dims(2048, 1024, 512), (512, 256));
    }
}
