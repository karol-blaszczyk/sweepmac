//! Visual tokens and shared chrome for the sweepmac window.
//!
//! Every colour, radius and gutter used by the GUI is named here — no literal
//! `Color32::from_rgb` or magic padding scattered through the layout code. The
//! dark palette is the designed default; a light variant mirrors it so the app
//! follows the system appearance where macOS reports one.

use eframe::egui;
use egui::{Color32, Margin, RichText, Rounding, Stroke};

/// Named colours for one appearance. Field names describe the *role*, so the
/// layout code never has to know which appearance is active.
#[derive(Clone, Copy, PartialEq)]
pub struct Tokens {
    pub dark: bool,
    /// Window background.
    pub canvas: Color32,
    /// Ordinary group/card background.
    pub surface: Color32,
    /// Raised or hovered surface.
    pub raised: Color32,
    pub divider: Color32,
    pub text: Color32,
    pub text_secondary: Color32,
    pub text_muted: Color32,
    /// Accent, focus ring, and the normal call to action.
    pub accent: Color32,
    /// "Safe" status and the reclaimable highlight.
    pub safe: Color32,
    /// Conditional developer cleanup.
    pub caution: Color32,
    /// Irreversible actions only.
    pub danger: Color32,
    /// Text drawn on top of a filled accent/danger button.
    pub on_accent: Color32,
}

impl Tokens {
    pub fn dark() -> Self {
        Tokens {
            dark: true,
            canvas: Color32::from_rgb(0x11, 0x13, 0x18),
            surface: Color32::from_rgb(0x19, 0x1D, 0x25),
            raised: Color32::from_rgb(0x21, 0x27, 0x35),
            divider: Color32::from_white_alpha(20), // ~8%
            text: Color32::from_rgb(0xF3, 0xF5, 0xF8),
            text_secondary: Color32::from_rgb(0xA9, 0xB1, 0xC0),
            text_muted: Color32::from_rgb(0x78, 0x81, 0x94),
            accent: Color32::from_rgb(0x78, 0xA7, 0xFF),
            safe: Color32::from_rgb(0x55, 0xB9, 0x8B),
            caution: Color32::from_rgb(0xE8, 0xB4, 0x5B),
            danger: Color32::from_rgb(0xE1, 0x6A, 0x70),
            on_accent: Color32::from_rgb(0x0C, 0x12, 0x1E),
        }
    }

    /// The same roles for macOS light appearance. Accent/safe/caution/danger are
    /// darkened so they keep contrast on a light surface.
    pub fn light() -> Self {
        Tokens {
            dark: false,
            canvas: Color32::from_rgb(0xF2, 0xF4, 0xF7),
            surface: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            raised: Color32::from_rgb(0xE7, 0xEB, 0xF1),
            divider: Color32::from_black_alpha(22),
            text: Color32::from_rgb(0x14, 0x17, 0x1D),
            text_secondary: Color32::from_rgb(0x4A, 0x53, 0x63),
            text_muted: Color32::from_rgb(0x6B, 0x74, 0x85),
            accent: Color32::from_rgb(0x2C, 0x6B, 0xD9),
            safe: Color32::from_rgb(0x1E, 0x8A, 0x5D),
            caution: Color32::from_rgb(0xA9, 0x72, 0x0E),
            danger: Color32::from_rgb(0xC0, 0x3B, 0x43),
            on_accent: Color32::from_rgb(0xFF, 0xFF, 0xFF),
        }
    }

    pub fn for_theme(theme: egui::Theme) -> Self {
        match theme {
            egui::Theme::Light => Self::light(),
            egui::Theme::Dark => Self::dark(),
        }
    }
}

// --- metrics -------------------------------------------------------------

/// Window gutter (left/right page margin).
pub const GUTTER: f32 = 22.0;
/// Vertical padding at the top/bottom of a panel.
pub const PANEL_PAD_Y: f32 = 14.0;
/// Internal padding inside a group card.
pub const GROUP_PAD_X: f32 = 14.0;
pub const GROUP_PAD_Y: f32 = 12.0;
/// Corner radius for groups and dialogs.
pub const RADIUS: f32 = 9.0;
pub const RADIUS_SMALL: f32 = 6.0;
/// Vertical rhythm between sections and between rows.
pub const SECTION_GAP: f32 = 16.0;
pub const ROW_GAP: f32 = 6.0;

// --- type scale ----------------------------------------------------------

/// The single visual anchor: "4.7 GB safely reclaimable".
pub const T_HERO: f32 = 29.0;
pub const T_TITLE: f32 = 15.0;
pub const T_BODY: f32 = 13.0;
/// Section headers and secondary labels.
pub const T_LABEL: f32 = 12.0;
/// Technical metadata; kept at readable contrast, never below this size.
pub const T_META: f32 = 11.0;

// --- chrome --------------------------------------------------------------

/// Apply the palette to egui's own widget visuals.
pub fn install(ctx: &egui::Context, t: &Tokens) {
    let mut style = (*ctx.style()).clone();
    let v = &mut style.visuals;
    v.dark_mode = t.dark;
    v.panel_fill = t.canvas;
    v.window_fill = t.surface;
    v.window_stroke = Stroke::new(1.0, t.divider);
    v.window_rounding = Rounding::same(12.0);
    v.override_text_color = Some(t.text);
    v.selection.bg_fill = t.accent.gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0, t.accent);
    v.hyperlink_color = t.accent;

    let rounding = Rounding::same(RADIUS_SMALL);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.rounding = rounding;
        w.fg_stroke = Stroke::new(1.0, t.text);
    }
    v.widgets.noninteractive.bg_fill = t.surface;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, t.divider);
    v.widgets.inactive.bg_fill = t.raised;
    v.widgets.inactive.weak_bg_fill = t.raised;
    // Hovered/active double as the keyboard-focus appearance in egui, so give
    // them a visible accent edge rather than a subtle fill change.
    v.widgets.hovered.bg_fill = t.raised;
    v.widgets.hovered.weak_bg_fill = t.raised;
    v.widgets.hovered.bg_stroke = Stroke::new(1.5, t.accent);
    v.widgets.active.bg_fill = t.raised;
    v.widgets.active.weak_bg_fill = t.raised;
    v.widgets.active.bg_stroke = Stroke::new(2.0, t.accent);
    // Disabled controls stay legible: they explain themselves through nearby
    // copy, so they must not fade to unreadable grey.
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, t.text_secondary);

    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    style.spacing.icon_width = 16.0;
    style.spacing.icon_spacing = 8.0;
    style.spacing.interact_size.y = 22.0;
    ctx.set_style(style);
}

/// Install the macOS system font (SF) as the proportional family when it can be
/// read and looks like a usable single-face TrueType/OpenType file.
///
/// egui ships its own fonts; this only upgrades the UI to the platform face so
/// the window matches native chrome. Anything unexpected about the file and we
/// keep egui's bundled default rather than risk a font-parse failure.
pub fn install_system_font(ctx: &egui::Context) {
    const CANDIDATES: &[&str] = &[
        "/System/Library/Fonts/SFNS.ttf",
        "/System/Library/Fonts/SFNSText.ttf",
    ];
    for path in CANDIDATES {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        // Accept only single-face sfnt files: 0x00010000 (TrueType), "true", or
        // "OTTO" (CFF). Skip "ttcf" collections — they need a face index.
        let tag = bytes.get(0..4).unwrap_or(&[]);
        let usable = matches!(tag, [0x00, 0x01, 0x00, 0x00] | b"true" | b"OTTO");
        if !usable {
            continue;
        }
        let mut fonts = egui::FontDefinitions::default();
        fonts
            .font_data
            .insert("system-ui".to_owned(), egui::FontData::from_owned(bytes));
        // Insert ahead of the bundled face, which stays as the fallback for any
        // glyph the system font lacks.
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "system-ui".to_owned());
        ctx.set_fonts(fonts);
        return;
    }
}

// --- primitives ----------------------------------------------------------

/// A standard group card.
pub fn group<R>(ui: &mut egui::Ui, t: &Tokens, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::default()
        .fill(t.surface)
        .rounding(Rounding::same(RADIUS))
        .inner_margin(Margin::symmetric(GROUP_PAD_X, GROUP_PAD_Y))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// A group card tinted by an accent edge — used for the caution and danger
/// zones so their boundary is structural, not just a coloured word.
pub fn tinted_group<R>(
    ui: &mut egui::Ui,
    t: &Tokens,
    tint: Color32,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    egui::Frame::default()
        .fill(t.surface)
        .rounding(Rounding::same(RADIUS))
        .inner_margin(Margin::symmetric(GROUP_PAD_X, GROUP_PAD_Y))
        .stroke(Stroke::new(1.0, tint.gamma_multiply(0.45)))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// A hairline separator.
pub fn divider(ui: &mut egui::Ui, t: &Tokens) {
    let h = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(h, 1.0), egui::Sense::hover());
    ui.painter()
        .hline(rect.x_range(), rect.center().y, Stroke::new(1.0, t.divider));
}

/// An uppercase section label.
pub fn section_label(ui: &mut egui::Ui, t: &Tokens, text: &str) {
    ui.label(
        RichText::new(text)
            .size(T_LABEL)
            .strong()
            .color(t.text_secondary),
    );
}

pub fn meta(t: &Tokens, text: impl Into<String>) -> RichText {
    RichText::new(text.into()).size(T_META).color(t.text_muted)
}

pub fn body(t: &Tokens, text: impl Into<String>) -> RichText {
    RichText::new(text.into()).size(T_BODY).color(t.text)
}

/// The primary call to action.
pub fn primary_button(t: &Tokens, text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).size(T_BODY).strong().color(t.on_accent))
        .fill(t.accent)
        .rounding(Rounding::same(RADIUS_SMALL))
}

/// A destructive call to action. Reserved for irreversible paths.
pub fn danger_button(t: &Tokens, text: &str) -> egui::Button<'static> {
    egui::Button::new(
        RichText::new(text)
            .size(T_BODY)
            .strong()
            .color(Color32::WHITE),
    )
    .fill(t.danger)
    .rounding(Rounding::same(RADIUS_SMALL))
}

/// A quiet, secondary control (Rescan, Cancel, Select all…).
pub fn quiet_button(t: &Tokens, text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).size(T_LABEL).color(t.text_secondary))
        .fill(t.raised)
        .stroke(Stroke::NONE)
        .rounding(Rounding::same(RADIUS_SMALL))
}
