//! Colors, spacing and type scale for both light and dark mode (follows Windows).

use eframe::egui::{
    self, Color32, CornerRadius, FontId, Stroke, Style, TextStyle, Theme, Visuals, vec2,
};

pub struct Palette {
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_pressed: Color32,
    pub on_accent: Color32,
    pub accent_soft: Color32,
    pub background: Color32,
    pub surface: Color32,
    pub hover: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_weak: Color32,
    /// Error text and icons.
    pub danger: Color32,
    pub danger_soft: Color32,
    /// Fill of a destructive button, under white text.
    pub danger_fill: Color32,
    pub success: Color32,
    pub code_background: Color32,
    pub code_text: Color32,
    pub toast_background: Color32,
    pub toast_text: Color32,
    pub toast_border: Color32,
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

pub const LIGHT: Palette = Palette {
    accent: rgb(0x2563eb),
    accent_hover: rgb(0x1d4ed8),
    accent_pressed: rgb(0x1e40af),
    on_accent: Color32::WHITE,
    accent_soft: rgb(0xe6eefe),
    background: rgb(0xf3f5f9),
    surface: Color32::WHITE,
    hover: rgb(0xeef1f6),
    border: rgb(0xd9dfe8),
    text: rgb(0x1f2937),
    text_weak: rgb(0x6b7280),
    danger: rgb(0xdc2626),
    danger_soft: rgb(0xfdecec),
    danger_fill: rgb(0xdc2626),
    success: rgb(0x16a34a),
    code_background: rgb(0x1e293b),
    code_text: rgb(0xe2e8f0),
    toast_background: rgb(0x1f2937),
    toast_text: rgb(0xf3f4f6),
    toast_border: rgb(0x1f2937),
};

pub const DARK: Palette = Palette {
    accent: rgb(0x4f8cff),
    accent_hover: rgb(0x6a9eff),
    accent_pressed: rgb(0x3b74e0),
    on_accent: Color32::WHITE,
    accent_soft: rgb(0x22314f),
    background: rgb(0x16181d),
    surface: rgb(0x20232a),
    hover: rgb(0x2a2e37),
    border: rgb(0x343945),
    text: rgb(0xe6e8ee),
    text_weak: rgb(0x9aa1b0),
    danger: rgb(0xf87171),
    danger_soft: rgb(0x3a1f24),
    danger_fill: rgb(0xdc2626),
    success: rgb(0x4ade80),
    code_background: rgb(0x0e1015),
    code_text: rgb(0xe2e8f0),
    toast_background: rgb(0x2c313c),
    toast_text: rgb(0xf3f4f6),
    toast_border: rgb(0x414857),
};

pub const RADIUS: u8 = 8;
pub const CARD_RADIUS: u8 = 14;

pub fn palette(ctx: &egui::Context) -> &'static Palette {
    match ctx.theme() {
        Theme::Dark => &DARK,
        Theme::Light => &LIGHT,
    }
}

pub fn install(ctx: &egui::Context) {
    ctx.style_mut_of(Theme::Light, |style| apply(style, Visuals::light(), &LIGHT));
    ctx.style_mut_of(Theme::Dark, |style| apply(style, Visuals::dark(), &DARK));
}

fn apply(style: &mut Style, base: Visuals, p: &Palette) {
    let mut v = base;
    v.panel_fill = p.background;
    v.window_fill = p.surface;
    v.extreme_bg_color = p.surface;
    v.text_edit_bg_color = Some(p.surface);
    v.faint_bg_color = p.hover;
    v.code_bg_color = p.code_background;
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.accent_soft;
    v.selection.stroke = Stroke::new(1.5, p.accent);
    v.window_corner_radius = CornerRadius::same(CARD_RADIUS);

    let radius = CornerRadius::same(RADIUS);
    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.bg_fill = p.surface;
    w.inactive.weak_bg_fill = p.surface;
    w.inactive.bg_stroke = Stroke::new(1.0, p.border);
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.hovered.bg_fill = p.hover;
    w.hovered.weak_bg_fill = p.hover;
    w.hovered.bg_stroke = Stroke::new(1.0, p.accent);
    w.hovered.fg_stroke = Stroke::new(1.0, p.text);
    w.active.bg_fill = p.accent_soft;
    w.active.weak_bg_fill = p.accent_soft;
    w.active.bg_stroke = Stroke::new(1.5, p.accent);
    w.active.fg_stroke = Stroke::new(1.0, p.text);
    for state in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        state.corner_radius = radius;
        state.expansion = 0.0;
    }
    style.visuals = v;

    style.spacing.item_spacing = vec2(8.0, 8.0);
    style.spacing.button_padding = vec2(14.0, 7.0);
    style.spacing.interact_size.y = 34.0;
    style.text_styles = [
        (TextStyle::Heading, FontId::proportional(22.0)),
        (TextStyle::Body, FontId::proportional(15.0)),
        (TextStyle::Button, FontId::proportional(15.0)),
        (TextStyle::Small, FontId::proportional(12.5)),
        (TextStyle::Monospace, FontId::monospace(13.5)),
    ]
    .into();
}
