//! Visual style: dark, macOS-flavoured, modelled on Screen Studio's editor.

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle};

pub const BG: Color32 = Color32::from_rgb(0x08, 0x09, 0x0c);
pub const PANEL: Color32 = Color32::from_rgb(0x16, 0x18, 0x1d);
pub const SURFACE: Color32 = Color32::from_rgb(0x1b, 0x1d, 0x22);
pub const SURFACE_HOVER: Color32 = Color32::from_rgb(0x28, 0x2a, 0x2f);
pub const BORDER: Color32 = Color32::from_rgb(0x22, 0x24, 0x29);
pub const TEXT: Color32 = Color32::from_rgb(0xe6, 0xe6, 0xea);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x7c, 0x7e, 0x86);
pub const ACCENT: Color32 = Color32::from_rgb(0x49, 0x30, 0xec);
pub const KNOB: Color32 = Color32::from_rgb(0x9e, 0x91, 0xf3);
pub const TRACK: Color32 = Color32::from_rgb(0x2b, 0x2d, 0x32);
pub const ZOOM_B: Color32 = Color32::from_rgb(0x71, 0x5e, 0xee);
pub const ZOOM: Color32 = ZOOM_B;
pub const PLAYHEAD: Color32 = Color32::from_rgb(0x5b, 0x45, 0xf0);
pub const RECORD: Color32 = Color32::from_rgb(0xff, 0x3b, 0x30);

const SF_DIR: &str = "/usr/share/fonts/AppleSF";

/// Uses SF Pro when it is installed locally (we don't ship it), otherwise
/// egui's bundled fonts.
fn load_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let mut add = |name: &str, file: &str, family: FontFamily| {
        if let Ok(bytes) = std::fs::read(format!("{SF_DIR}/{file}")) {
            fonts.font_data.insert(name.into(), FontData::from_owned(bytes).into());
            fonts.families.entry(family).or_default().insert(0, name.into());
        }
    };
    add("sf-text", "SF-Pro-Text-Regular.otf", FontFamily::Proportional);
    add("sf-semibold", "SF-Pro-Text-Semibold.otf", FontFamily::Name("semibold".into()));
    if !fonts.families.contains_key(&FontFamily::Name("semibold".into())) {
        fonts.families.insert(FontFamily::Name("semibold".into()), fonts.families[&FontFamily::Proportional].clone());
    } else {
        let fallback = fonts.families[&FontFamily::Proportional].clone();
        fonts.families.get_mut(&FontFamily::Name("semibold".into())).unwrap().extend(fallback);
    }
    ctx.set_fonts(fonts);
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

pub fn apply(ctx: &egui::Context) {
    load_fonts(ctx);
    ctx.set_theme(egui::Theme::Dark);
    ctx.global_style_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(11.0)),
            (TextStyle::Body, FontId::proportional(13.0)),
            (TextStyle::Button, FontId::proportional(13.0)),
            (TextStyle::Heading, semibold(15.0)),
            (TextStyle::Monospace, FontId::monospace(12.0)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.spacing.slider_width = 170.0;
        style.spacing.interact_size.y = 26.0;
        let v = &mut style.visuals;
        v.panel_fill = PANEL;
        v.window_fill = PANEL;
        v.extreme_bg_color = BG;
        v.faint_bg_color = SURFACE;
        v.override_text_color = Some(TEXT);
        v.window_corner_radius = CornerRadius::same(12);
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.selection.bg_fill = ACCENT;
        v.widgets.inactive.expansion = 0.0;
        v.selection.stroke = Stroke::new(1.0, TEXT);
        v.slider_trailing_fill = true;
        let r = CornerRadius::same(7);
        for (w, fill) in [
            (&mut v.widgets.noninteractive, PANEL),
            (&mut v.widgets.inactive, SURFACE),
            (&mut v.widgets.hovered, SURFACE_HOVER),
            (&mut v.widgets.active, SURFACE_HOVER),
            (&mut v.widgets.open, SURFACE),
        ] {
            w.bg_fill = fill;
            w.weak_bg_fill = fill;
            w.corner_radius = r;
            w.bg_stroke = Stroke::NONE;
            w.fg_stroke = Stroke::new(1.0, TEXT);
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_DIM);
    });
}

/// Filled accent button (e.g. Export).
pub fn primary_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(label).font(semibold(13.0)).color(Color32::WHITE))
            .fill(ACCENT)
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(8))
            .min_size(egui::vec2(0.0, 30.0)),
    )
}

/// Small grey section caption used in the settings sidebar.
pub fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(10.0);
    ui.label(egui::RichText::new(title).size(13.5).color(TEXT));
    ui.add_space(2.0);
}
