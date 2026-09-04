//! The app's own look, and the handful of widgets it is built from.
//!
//! egui's stock dark theme is a fine default and a poor identity: every panel
//! is the same grey, every heading the same 18px, and the one accent is a blue
//! that fights the sprite. So the whole `Visuals` struct is replaced here —
//! cooler greys with three levels of surface, an indigo accent, filled slider
//! rails — and the few places the stock widgets could not say what was meant
//! (a section rule, a tool with an icon, a tab that knows whether its canvas
//! has been drawn on) get their own painting.
//!
//! Everything is a theme, not a fork: no widget here reaches outside egui, and
//! the light and dark palettes differ only in their numbers.

use eframe::egui;
use egui::{Color32, CornerRadius, Stroke, Vec2};

/// Every colour the app paints with, for one theme.
#[derive(Clone, Copy)]
pub struct Palette {
    /// The three surface levels: the panels, the bars that top and tail them,
    /// and the wells the viewports sink into.
    pub panel: Color32,
    pub bar: Color32,
    pub status: Color32,
    pub well: Color32,
    pub card: Color32,
    pub card_stroke: Color32,
    pub line: Color32,

    pub widget: Color32,
    pub widget_hover: Color32,
    pub widget_active: Color32,

    pub text: Color32,
    pub text_strong: Color32,
    pub text_weak: Color32,
    pub text_button: Color32,

    pub accent: Color32,
    pub accent_soft: Color32,
    /// Reserved for one meaning: there is something on this canvas.
    pub amber: Color32,

    pub checker_light: Color32,
    pub checker_dark: Color32,
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

pub const DARK: Palette = Palette {
    panel: rgb(0x1d1e23),
    bar: rgb(0x21232a),
    status: rgb(0x1a1b20),
    well: rgb(0x15161a),
    card: rgb(0x22242a),
    card_stroke: rgb(0x2b2d34),
    line: rgb(0x2e3037),

    widget: rgb(0x32343c),
    widget_hover: rgb(0x3d404a),
    widget_active: rgb(0x3a3f6b),

    text: rgb(0xa3a7b2),
    text_strong: rgb(0xeceef3),
    text_weak: rgb(0x6b6f7a),
    text_button: rgb(0xc6cad4),

    accent: rgb(0x4b57d6),
    accent_soft: rgb(0x6d7cf7),
    amber: rgb(0xe3a44c),

    checker_light: rgb(0x212228),
    checker_dark: rgb(0x191a1f),
};

pub const LIGHT: Palette = Palette {
    panel: rgb(0xedeef2),
    bar: rgb(0xf5f6f9),
    status: rgb(0xe6e7ed),
    well: rgb(0xd7d9e1),
    card: rgb(0xf8f9fb),
    card_stroke: rgb(0xdcdee5),
    line: rgb(0xd2d4dc),

    widget: rgb(0xdfe1e8),
    widget_hover: rgb(0xd2d5df),
    widget_active: rgb(0xc6cbf0),

    text: rgb(0x4a4e58),
    text_strong: rgb(0x1c1e24),
    text_weak: rgb(0x878b95),
    text_button: rgb(0x3a3d45),

    accent: rgb(0x4b57d6),
    accent_soft: rgb(0x6d7cf7),
    amber: rgb(0xb5761f),

    checker_light: rgb(0xd0d2da),
    checker_dark: rgb(0xc2c5ce),
};

pub fn palette(ui: &egui::Ui) -> Palette {
    if ui.visuals().dark_mode { DARK } else { LIGHT }
}

/// Numbers, in the monospace face — so a slider readout stops jittering as it
/// drags and figures line up down a column.
pub fn num(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text).monospace()
}

// --------------------------------------------------------------- the style

/// Install the theme into both the light and the dark style.
pub fn install(ctx: &egui::Context) {
    for (theme, p) in [(egui::Theme::Dark, DARK), (egui::Theme::Light, LIGHT)] {
        ctx.style_mut_of(theme, |style| {
            style.text_styles = text_styles();
            // Every readout in the monospace face, so a figure stops jittering
            // under the cursor as its slider drags.
            style.drag_value_text_style = egui::TextStyle::Monospace;

            let s = &mut style.spacing;
            // Narrow enough that a slider, its value box and its label all fit
            // inside the tool column without the label being clipped.
            s.slider_width = 100.0;
            s.slider_rail_height = 6.0;
            s.item_spacing = egui::vec2(8.0, 5.0);
            s.button_padding = egui::vec2(8.0, 3.0);
            s.interact_size = egui::vec2(40.0, 22.0);
            s.combo_width = 104.0;
            s.icon_width = 15.0;
            s.icon_width_inner = 9.0;
            s.menu_margin = egui::Margin::same(6);
            s.window_margin = egui::Margin::same(8);

            let v = &mut style.visuals;
            let base = if theme == egui::Theme::Dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            };
            *v = base;

            v.panel_fill = p.panel;
            v.window_fill = p.card;
            v.window_stroke = Stroke::new(1.0, p.card_stroke);
            v.extreme_bg_color = p.well;
            v.faint_bg_color = p.card;
            v.code_bg_color = p.card;
            v.window_corner_radius = CornerRadius::same(8);
            v.menu_corner_radius = CornerRadius::same(8);
            v.hyperlink_color = p.accent_soft;

            v.selection.bg_fill = p.accent;
            v.selection.stroke = Stroke::new(1.0, Color32::WHITE);

            // The rail fills up to the handle: a slider then reads as a
            // quantity at a glance rather than as a dot on a line.
            v.slider_trailing_fill = true;
            v.handle_shape = egui::style::HandleShape::Rect { aspect_ratio: 0.5 };

            let w = &mut v.widgets;
            w.noninteractive.bg_fill = p.panel;
            w.noninteractive.weak_bg_fill = p.panel;
            w.noninteractive.bg_stroke = Stroke::new(1.0, p.line);
            w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
            w.noninteractive.corner_radius = CornerRadius::same(4);

            w.inactive.bg_fill = p.widget;
            w.inactive.weak_bg_fill = p.widget;
            w.inactive.bg_stroke = Stroke::NONE;
            w.inactive.fg_stroke = Stroke::new(1.0, p.text_button);
            w.inactive.corner_radius = CornerRadius::same(4);

            w.hovered.bg_fill = p.widget_hover;
            w.hovered.weak_bg_fill = p.widget_hover;
            w.hovered.bg_stroke = Stroke::new(1.0, p.line);
            w.hovered.fg_stroke = Stroke::new(1.5, p.text_strong);
            w.hovered.corner_radius = CornerRadius::same(4);
            w.hovered.expansion = 0.0;

            w.active.bg_fill = p.widget_active;
            w.active.weak_bg_fill = p.widget_active;
            w.active.bg_stroke = Stroke::new(1.0, p.accent_soft);
            w.active.fg_stroke = Stroke::new(1.5, p.text_strong);
            w.active.corner_radius = CornerRadius::same(4);
            w.active.expansion = 0.0;

            w.open.bg_fill = p.card;
            w.open.weak_bg_fill = p.widget;
            w.open.bg_stroke = Stroke::new(1.0, p.card_stroke);
            w.open.fg_stroke = Stroke::new(1.0, p.text_strong);
            w.open.corner_radius = CornerRadius::same(4);
        });
    }
}

fn text_styles() -> std::collections::BTreeMap<egui::TextStyle, egui::FontId> {
    use egui::FontFamily::{Monospace, Proportional};
    use egui::{FontId, TextStyle};
    [
        (TextStyle::Small, FontId::new(11.0, Proportional)),
        (TextStyle::Body, FontId::new(13.0, Proportional)),
        (TextStyle::Button, FontId::new(13.0, Proportional)),
        (TextStyle::Heading, FontId::new(15.0, Proportional)),
        (TextStyle::Monospace, FontId::new(12.0, Monospace)),
    ]
    .into()
}

// ------------------------------------------------------------- the widgets

/// A section header: a small upper-case label with a hairline running out of
/// it to the panel's edge. Quieter than an 18px heading and easier to skim,
/// because the rule does the separating a bigger font would have to do.
pub fn section(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let p = palette(ui);
    ui.add_space(4.0);
    let response = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let response = ui.label(
                egui::RichText::new(label.to_uppercase())
                    .size(10.0)
                    .color(p.text_weak),
            );
            let rest = ui.available_width();
            if rest > 8.0 {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(rest, 1.0), egui::Sense::hover());
                let y = rect.center().y;
                // Fading out rather than stopping dead: a full-width hairline
                // reads as a divider, and this is a label, not a division.
                let mut mesh = egui::Mesh::default();
                let (a, b) = (p.line, p.line.gamma_multiply(0.0));
                mesh.colored_vertex(egui::pos2(rect.left(), y - 0.5), a);
                mesh.colored_vertex(egui::pos2(rect.left(), y + 0.5), a);
                mesh.colored_vertex(egui::pos2(rect.right(), y - 0.5), b);
                mesh.colored_vertex(egui::pos2(rect.right(), y + 0.5), b);
                mesh.add_triangle(0, 1, 2);
                mesh.add_triangle(1, 2, 3);
                ui.painter().add(egui::Shape::mesh(mesh));
            }
            response
        })
        .inner;
    ui.add_space(2.0);
    response
}

/// A tab: a pill, with an optional amber dot meaning there is something on the
/// canvas behind it.
pub fn tab(ui: &mut egui::Ui, selected: bool, text: &str, drawn: bool) -> egui::Response {
    let p = palette(ui);
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font, Color32::PLACEHOLDER);

    let dot = if drawn { 10.0 } else { 0.0 };
    let size = egui::vec2(galley.size().x + 18.0 + dot, galley.size().y + 7.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());

    if ui.is_rect_visible(rect) {
        let radius = CornerRadius::same((rect.height() / 2.0) as u8);
        let fill = if selected {
            p.accent
        } else if response.hovered() {
            p.widget
        } else {
            Color32::TRANSPARENT
        };
        if fill != Color32::TRANSPARENT {
            ui.painter().rect_filled(rect, radius, fill);
        }
        let text_color = if selected {
            Color32::WHITE
        } else if drawn {
            p.text
        } else {
            p.text_weak
        };
        let pos = egui::pos2(rect.left() + 9.0, rect.center().y - galley.size().y / 2.0);
        ui.painter().galley(pos, galley, text_color);
        if drawn {
            let c = egui::pos2(rect.right() - 9.0, rect.center().y);
            ui.painter().circle_filled(c, 2.5, p.amber);
        }
    }
    response
}

/// The paint tools: an icon, then a label, in a row two of which fit the
/// column. Selected is the accent; the icon carries the recognition, so the
/// grid can be skimmed by shape once the labels have been read once.
pub fn tool(
    ui: &mut egui::Ui,
    selected: bool,
    enabled: bool,
    icon: Icon,
    label: &str,
) -> egui::Response {
    let p = palette(ui);
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, Color32::PLACEHOLDER);

    let width = ui.available_width().min(122.0).max(galley.size().x + 38.0);
    let size = egui::vec2(width, galley.size().y.max(15.0) + 9.0);
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);

    if ui.is_rect_visible(rect) {
        let fill = if selected {
            p.accent
        } else if enabled && response.hovered() {
            p.widget
        } else {
            Color32::TRANSPARENT
        };
        if fill != Color32::TRANSPARENT {
            ui.painter().rect_filled(rect, CornerRadius::same(4), fill);
        }
        let colour = if !enabled {
            p.text_weak.gamma_multiply(0.7)
        } else if selected {
            Color32::WHITE
        } else if response.hovered() {
            p.text_strong
        } else {
            p.text_button
        };
        let icon_rect = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 7.0, rect.center().y - 7.5),
            Vec2::splat(15.0),
        );
        icon.paint(ui.painter(), icon_rect, colour);
        let pos = egui::pos2(
            icon_rect.right() + 7.0,
            rect.center().y - galley.size().y / 2.0,
        );
        ui.painter().galley(pos, galley, colour);
    }
    response
}

/// One rung of the height ramp: a square of the grey that height bakes to,
/// ringed in the accent when it is the one the brush is holding. A palette,
/// except that the colours mean depth — which is the whole point, because a
/// relief drawn in ten greys can be read back off the canvas by eye.
pub fn swatch(ui: &mut egui::Ui, selected: bool, value: f32) -> egui::Response {
    let p = palette(ui);
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(21.0), egui::Sense::click());

    if ui.is_rect_visible(rect) {
        // The selected swatch pulls in to leave room for its ring, rather than
        // growing and shoving the row along.
        let inner = rect.shrink(if selected { 3.0 } else { 0.0 });
        let radius = CornerRadius::same(3);
        let grey = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
        ui.painter()
            .rect_filled(inner, radius, Color32::from_gray(grey));
        // Black and white both need an edge, or they dissolve into whichever
        // theme is running.
        ui.painter().rect_stroke(
            inner,
            radius,
            Stroke::new(1.0, if response.hovered() { p.text } else { p.line }),
            egui::StrokeKind::Inside,
        );
        if selected {
            ui.painter().rect_stroke(
                rect,
                CornerRadius::same(5),
                Stroke::new(2.0, p.accent_soft),
                egui::StrokeKind::Inside,
            );
        }
    }
    response
}

/// The frame the two viewports sink into, and the shadow the image sits on.
pub fn well(ui: &egui::Ui, rect: egui::Rect) {
    let p = palette(ui);
    ui.painter()
        .rect_filled(rect, CornerRadius::same(6), p.well);
}

/// Lift the image off the well: a soft drop shadow and a hairline in the
/// accent, so a sprite with dark edges still has a boundary.
pub fn sprite_frame(ui: &egui::Ui, rect: egui::Rect) {
    let p = palette(ui);
    let shadow = egui::epaint::Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(if ui.visuals().dark_mode { 150 } else { 60 }),
    };
    ui.painter().add(shadow.as_shape(rect, 0.0));
    ui.painter().rect_stroke(
        rect,
        0.0,
        Stroke::new(1.0, p.accent_soft.gamma_multiply(0.35)),
        egui::StrokeKind::Outside,
    );
}

/// The checkerboard a transparent sprite sits on, so it does not read as a
/// dark silhouette.
pub fn paint_checkerboard(ui: &egui::Ui, rect: egui::Rect) {
    const CHECKER: f32 = 10.0;
    let p = palette(ui);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, p.checker_light);

    let cols = (rect.width() / CHECKER).ceil() as i32;
    let rows = (rect.height() / CHECKER).ceil() as i32;
    for row in 0..rows {
        for col in 0..cols {
            if (row + col) % 2 == 0 {
                continue;
            }
            let min = rect.min + egui::vec2(col as f32, row as f32) * CHECKER;
            let square = egui::Rect::from_min_size(min, Vec2::splat(CHECKER)).intersect(rect);
            painter.rect_filled(square, 0.0, p.checker_dark);
        }
    }
}

/// The frames the bars at the top and bottom of the window sit in — a step
/// lighter and a step darker than the panels between them, so the window has
/// a top and a bottom rather than being one flat sheet.
pub fn bar(ui: &egui::Ui, top: bool) -> egui::Frame {
    let p = palette(ui);
    egui::Frame::new()
        .fill(if top { p.bar } else { p.status })
        .inner_margin(egui::Margin::symmetric(10, 6))
}

/// A group of controls that act as one: the zoom stepper, say.
pub fn group(ui: &egui::Ui) -> egui::Frame {
    let p = palette(ui);
    egui::Frame::new()
        .fill(p.well)
        .stroke(Stroke::new(1.0, p.line))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(4, 2))
}

/// The app's mark: the accent square with a relief profile cut through it.
pub fn mark(ui: &mut egui::Ui) {
    let p = palette(ui);
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(17.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(4), p.accent);
    let s = rect.width() / 16.0;
    let pt = |x: f32, y: f32| rect.min + egui::vec2(x * s, y * s);
    ui.painter().add(egui::Shape::line(
        vec![pt(4.0, 11.0), pt(6.6, 6.2), pt(9.0, 9.2), pt(12.0, 5.0)],
        Stroke::new(1.5, Color32::from_rgb(0xdf, 0xe3, 0xff)),
    ));
}

// ---------------------------------------------------------------- the icons

/// The tool icons, drawn rather than shipped: seven strokes on a 16-unit grid,
/// so they scale with the ui and take the colour of the row they sit in.
#[derive(Clone, Copy)]
pub enum Icon {
    Pencil,
    Eraser,
    Bucket,
    Picker,
    Raise,
    Lower,
    Smooth,
}

impl Icon {
    /// Polylines in a 0..16 box.
    fn strokes(self) -> Vec<Vec<[f32; 2]>> {
        match self {
            Icon::Pencil => vec![
                vec![
                    [2.8, 13.2],
                    [3.5, 10.6],
                    [10.5, 3.6],
                    [12.6, 5.7],
                    [5.6, 12.7],
                    [2.8, 13.2],
                ],
                vec![[9.6, 3.2], [12.8, 6.4]],
            ],
            Icon::Eraser => vec![
                vec![
                    [6.6, 12.4],
                    [2.8, 8.6],
                    [8.8, 2.6],
                    [12.6, 6.4],
                    [6.6, 12.4],
                ],
                vec![[2.6, 13.8], [13.4, 13.8]],
            ],
            Icon::Bucket => vec![
                vec![[7.4, 2.4], [12.2, 7.2], [7.4, 12.0], [2.6, 7.2], [7.4, 2.4]],
                vec![[13.2, 8.4], [14.6, 11.0], [11.8, 11.0], [13.2, 8.4]],
            ],
            Icon::Picker => vec![
                vec![[10.2, 2.6], [13.4, 5.8]],
                vec![
                    [9.4, 4.2],
                    [11.8, 6.6],
                    [5.8, 12.6],
                    [3.4, 12.6],
                    [3.4, 10.2],
                    [9.4, 4.2],
                ],
            ],
            Icon::Raise => vec![
                vec![[8.0, 11.0], [8.0, 3.4]],
                vec![[4.8, 6.6], [8.0, 3.4], [11.2, 6.6]],
                vec![[2.6, 13.8], [13.4, 13.8]],
            ],
            Icon::Lower => vec![
                vec![[8.0, 2.6], [8.0, 10.2]],
                vec![[4.8, 7.0], [8.0, 10.2], [11.2, 7.0]],
                vec![[2.6, 13.8], [13.4, 13.8]],
            ],
            Icon::Smooth => {
                // Two sine waves, the lower one twice the amplitude: the tool
                // takes the edge off a step, and the icon says so.
                let wave = |y: f32, amp: f32| {
                    (0..=16)
                        .map(|i| {
                            let t = i as f32 / 16.0;
                            [1.8 + t * 12.4, y - (t * std::f32::consts::TAU).sin() * amp]
                        })
                        .collect::<Vec<_>>()
                };
                vec![wave(5.4, 1.6), wave(10.6, 2.6)]
            }
        }
    }

    fn paint(self, painter: &egui::Painter, rect: egui::Rect, colour: Color32) {
        let stroke = Stroke::new(1.3 * rect.width() / 15.0, colour);
        let scale = rect.width() / 16.0;
        for line in self.strokes() {
            let points = line
                .into_iter()
                .map(|[x, y]| rect.min + egui::vec2(x * scale, y * scale))
                .collect();
            painter.add(egui::Shape::line(points, stroke));
        }
    }
}
