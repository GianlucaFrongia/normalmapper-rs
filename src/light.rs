//! The lit preview: a draggable light over the generated maps.
//!
//! It sits in a column beside the maps rather than in a window of its own.
//! Judging a normal map means watching it move under a light while you turn
//! the knobs that made it — a floating window puts the two in different
//! places, and covers the thing being judged as soon as the mouse lands on it.

use eframe::egui;

use crate::normalmap::{self, Light, Shading};

/// Radius of the light-direction ball widget, in points.
const BALL_RADIUS: f32 = 44.0;

pub struct LightPreview {
    /// Whether the column is showing. Not a window any more — a panel the
    /// window can be narrowed by folding away.
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
            open: true,
            light: Light::default(),
            tex: None,
            seen_revision: u64::MAX,
            dirty: true,
        }
    }
}

impl LightPreview {
    /// The right-hand column. Drawn before the central panel, so the maps get
    /// whatever width is left rather than the other way round.
    pub fn panel(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        shading: Option<&Shading>,
        source: &str,
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

        egui::Panel::right("lit")
            .resizable(true)
            .default_size(380.0)
            .size_range(280.0..=720.0)
            .show(ui, |ui| self.contents(ui, ctx, shading, source));
    }

    fn contents(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        shading: Option<&Shading>,
        source: &str,
    ) {
        crate::theme::section(ui, "Lit preview");
        let Some(shading) = shading else {
            ui.centered_and_justified(|ui| {
                ui.label(
                    egui::RichText::new("Open an image, or paint a relief, to see it lit.").weak(),
                );
            });
            return;
        };
        // Two things can feed this column; say which one is talking.
        ui.label(
            egui::RichText::new(format!("from the {source}"))
                .small()
                .color(crate::theme::palette(ui).text_weak),
        );

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

        // The controls go at the bottom so the image gets whatever is left,
        // rather than the other way round.
        egui::Panel::bottom("light-controls")
            .frame(egui::Frame::NONE)
            .show(ui, |ui| self.controls(ui));

        let Some(tex) = &self.tex else { return };
        let native = tex.size_vec2();
        let (viewport, _) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
        let avail = viewport.size() - egui::vec2(8.0, 8.0);
        let mut scale = (avail.x / native.x).min(avail.y / native.y).max(0.05);
        // Magnify by whole texels only: this is a pixel-art preview, and a
        // sprite blown up 7.3× has uneven texels wherever the fraction lands.
        if scale > 1.0 {
            scale = scale.floor();
        }
        let rect = egui::Rect::from_center_size(viewport.center(), (native * scale).round());

        crate::theme::well(ui, viewport);
        // Without this a transparent sprite reads as a dark silhouette.
        crate::theme::paint_checkerboard(ui, rect);
        crate::theme::sprite_frame(ui, rect);
        egui::Image::new((tex.id(), rect.size()))
            .texture_options(egui::TextureOptions::NEAREST)
            .paint_at(ui, rect);
    }

    /// The light itself: where it comes from, and what the surface does with it.
    fn controls(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
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

        let p = crate::theme::palette(ui);
        let painter = ui.painter();
        painter.circle_filled(centre, BALL_RADIUS, p.well);
        painter.circle_stroke(centre, BALL_RADIUS, egui::Stroke::new(1.0, p.line));
        // A horizon ring, so the ball reads as a hemisphere rather than a
        // disc: inside it the light is high, out at the rim it is grazing.
        painter.circle_stroke(
            centre,
            BALL_RADIUS * 0.55,
            egui::Stroke::new(1.0, p.line.gamma_multiply(0.7)),
        );

        let [x, y, _] = self.light.dir;
        let knob = centre + egui::vec2(x, -y) * BALL_RADIUS;
        painter.line_segment([centre, knob], egui::Stroke::new(1.0, p.accent_soft));
        painter.circle_filled(knob, 5.0, p.text_strong);
        response.on_hover_text("Drag to move the light");
    }
}
