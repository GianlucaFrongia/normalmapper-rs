//! The lit-preview window: a draggable light over the generated maps.

use eframe::egui;

use crate::normalmap::{self, Light, Shading};

/// Radius of the light-direction ball widget, in points.
const BALL_RADIUS: f32 = 44.0;

pub struct LightPreview {
    pub open: bool,
    light: Light,
    tex: Option<egui::TextureHandle>,
    /// Bumped by the app whenever the maps behind the preview change.
    seen_revision: u64,
    dirty: bool,
}

impl Default for LightPreview {
    fn default() -> Self {
        Self {
            open: false,
            light: Light::default(),
            tex: None,
            seen_revision: u64::MAX,
            dirty: true,
        }
    }
}

impl LightPreview {
    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        shading: Option<&Shading>,
        revision: u64,
        flip_y: bool,
    ) {
        if !self.open {
            return;
        }
        if self.seen_revision != revision {
            self.seen_revision = revision;
            self.dirty = true;
        }
        if self.light.flip_y != flip_y {
            self.light.flip_y = flip_y;
            self.dirty = true;
        }

        let mut open = self.open;
        egui::Window::new("Lit preview")
            .open(&mut open)
            .default_size([340.0, 420.0])
            .show(ctx, |ui| self.contents(ui, ctx, shading));
        self.open = open;
    }

    fn contents(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, shading: Option<&Shading>) {
        let Some(shading) = shading else {
            ui.label("Open an image to see it lit.");
            return;
        };

        if self.dirty || self.tex.is_none() {
            self.dirty = false;
            let lit = normalmap::shade(shading, &self.light);
            self.tex = Some(ctx.load_texture(
                "lit",
                egui::ColorImage::from_rgba_unmultiplied(
                    [lit.width() as usize, lit.height() as usize],
                    lit.as_raw(),
                ),
                egui::TextureOptions::NEAREST,
            ));
        }

        if let Some(tex) = &self.tex {
            let native = tex.size_vec2();
            let avail = ui.available_size() - egui::vec2(0.0, 150.0);
            let scale = (avail.x / native.x).min(avail.y / native.y).max(0.05);
            ui.vertical_centered(|ui| {
                ui.add(
                    egui::Image::new((tex.id(), native * scale))
                        .texture_options(egui::TextureOptions::NEAREST),
                );
            });
        }

        ui.separator();
        ui.horizontal(|ui| {
            self.light_ball(ui);
            ui.vertical(|ui| {
                let mut changed = false;
                changed |= ui
                    .add(egui::Slider::new(&mut self.light.ambient, 0.0..=1.0).text("Ambient"))
                    .changed();
                changed |= ui
                    .add(egui::Slider::new(&mut self.light.specular, 0.0..=1.0).text("Specular"))
                    .changed();
                changed |= ui
                    .checkbox(&mut self.light.use_albedo, "Use source colours")
                    .changed();
                if changed {
                    self.dirty = true;
                }
            });
        });
    }

    /// The classic light ball: drag inside the circle to aim the light.
    fn light_ball(&mut self, ui: &mut egui::Ui) {
        let size = egui::Vec2::splat(BALL_RADIUS * 2.0 + 4.0);
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
        let centre = rect.center();

        if (response.dragged() || response.clicked())
            && let Some(pos) = response.interact_pointer_pos()
        {
            let mut v = (pos - centre) / BALL_RADIUS;
            let len = v.length();
            if len > 1.0 {
                v /= len;
            }
            // Screen Y grows downwards; the light's does not.
            let z = (1.0 - v.length_sq()).max(0.0).sqrt();
            self.light.dir = [v.x, -v.y, z];
            self.dirty = true;
        }

        let painter = ui.painter();
        let visuals = ui.visuals();
        painter.circle_filled(centre, BALL_RADIUS, visuals.extreme_bg_color);
        painter.circle_stroke(centre, BALL_RADIUS, visuals.widgets.inactive.fg_stroke);

        let [x, y, _] = self.light.dir;
        let knob = centre + egui::vec2(x, -y) * BALL_RADIUS;
        painter.line_segment([centre, knob], visuals.widgets.inactive.fg_stroke);
        painter.circle_filled(knob, 5.0, visuals.strong_text_color());
        response.on_hover_text("Drag to move the light");
    }
}
