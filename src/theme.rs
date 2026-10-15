//! "Patchbay" design system: switchable palettes, IBM Plex type and the shared widgets.
//!
//! Visual language: dark rack hardware with amber phosphor as the single accent, hairline
//! rules instead of boxes, mono type for anything technical (addresses, ports, keys).

use std::sync::RwLock;

use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Pos2, Rect,
    RichText, Shadow, Stroke, Vec2,
};

/// A complete color scheme for UI chrome and terminal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub id: &'static str,
    pub name: &'static str,
    pub dark: bool,
    /// Main canvas.
    pub bg_deep: Color32,
    /// Port rail and session headers.
    pub bg_panel: Color32,
    /// Raised surfaces: inputs, modals, menus.
    pub bg_card: Color32,
    pub bg_hover: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub accent: Color32,
    /// Text drawn on top of the accent.
    pub on_accent: Color32,
    /// Secondary signal color (RDP, links).
    pub accent2: Color32,
    /// Broadcast mode.
    pub pink: Color32,
    pub amber: Color32,
    pub red: Color32,
    pub green: Color32,
    pub term_bg: Color32,
    pub term_fg: Color32,
    pub ansi: [Color32; 16],
}

const fn c(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

const fn ansi(h: [u32; 16]) -> [Color32; 16] {
    let mut out = [Color32::BLACK; 16];
    let mut i = 0;
    while i < 16 {
        out[i] = c(h[i]);
        i += 1;
    }
    out
}

pub const THEMES: [Palette; 12] = [
    Palette {
        id: "phosphor",
        name: "Phosphor",
        dark: true,
        bg_deep: c(0x141413),
        bg_panel: c(0x0E0E0D),
        bg_card: c(0x1C1B19),
        bg_hover: c(0x252421),
        border: c(0x2E2C28),
        text: c(0xEDE9E1),
        text_dim: c(0x8E897F),
        accent: c(0xF2A33A),
        on_accent: c(0x1A1206),
        accent2: c(0x79B4E0),
        pink: c(0xE8739E),
        amber: c(0xF2A33A),
        red: c(0xE5615B),
        green: c(0x86C77E),
        term_bg: c(0x100F0E),
        term_fg: c(0xE8E2D6),
        ansi: ansi([
            0x2A2825, 0xE5615B, 0x86C77E, 0xF2C14E, 0x79B4E0, 0xC792D8, 0x6CC4B8, 0xD8D2C6,
            0x6B665D, 0xF0857F, 0xA6DB9E, 0xF7D37A, 0x9FCBEC, 0xDBB0E8, 0x8FDACF, 0xF6F2EA,
        ]),
    },
    Palette {
        id: "glacier",
        name: "Glacier",
        dark: true,
        bg_deep: c(0x111418),
        bg_panel: c(0x0B0D10),
        bg_card: c(0x181C21),
        bg_hover: c(0x20262D),
        border: c(0x29303A),
        text: c(0xE6EBF0),
        text_dim: c(0x8792A0),
        accent: c(0x7CC4FA),
        on_accent: c(0x07131D),
        accent2: c(0xB59CF0),
        pink: c(0xF08BB4),
        amber: c(0xE8B65A),
        red: c(0xEF6B6B),
        green: c(0x7DD3A0),
        term_bg: c(0x0D1014),
        term_fg: c(0xDDE4EC),
        ansi: ansi([
            0x232A33, 0xEF6B6B, 0x7DD3A0, 0xE8B65A, 0x7CC4FA, 0xB59CF0, 0x6FD6D6, 0xC9D2DC,
            0x5B6573, 0xF58E8E, 0x9CE4BA, 0xF0CB7E, 0xA2D6FC, 0xCBB8F5, 0x95E6E6, 0xF2F6FA,
        ]),
    },
    Palette {
        id: "nord",
        name: "Nord",
        dark: true,
        bg_deep: c(0x2E3440),
        bg_panel: c(0x272C36),
        bg_card: c(0x3B4252),
        bg_hover: c(0x434C5E),
        border: c(0x4C566A),
        text: c(0xECEFF4),
        text_dim: c(0x9AA5B8),
        accent: c(0x88C0D0),
        on_accent: c(0x1D232D),
        accent2: c(0xB48EAD),
        pink: c(0xB48EAD),
        amber: c(0xEBCB8B),
        red: c(0xBF616A),
        green: c(0xA3BE8C),
        term_bg: c(0x2B303B),
        term_fg: c(0xD8DEE9),
        ansi: ansi([
            0x3B4252, 0xBF616A, 0xA3BE8C, 0xEBCB8B, 0x81A1C1, 0xB48EAD, 0x88C0D0, 0xE5E9F0,
            0x4C566A, 0xBF616A, 0xA3BE8C, 0xEBCB8B, 0x81A1C1, 0xB48EAD, 0x8FBCBB, 0xECEFF4,
        ]),
    },
    Palette {
        id: "dracula",
        name: "Dracula",
        dark: true,
        bg_deep: c(0x282A36),
        bg_panel: c(0x21222C),
        bg_card: c(0x313341),
        bg_hover: c(0x3A3D4E),
        border: c(0x44475A),
        text: c(0xF8F8F2),
        text_dim: c(0x9EA2C0),
        accent: c(0xBD93F9),
        on_accent: c(0x1B1530),
        accent2: c(0x8BE9FD),
        pink: c(0xFF79C6),
        amber: c(0xF1FA8C),
        red: c(0xFF5555),
        green: c(0x50FA7B),
        term_bg: c(0x22232E),
        term_fg: c(0xF8F8F2),
        ansi: ansi([
            0x21222C, 0xFF5555, 0x50FA7B, 0xF1FA8C, 0xBD93F9, 0xFF79C6, 0x8BE9FD, 0xF8F8F2,
            0x6272A4, 0xFF6E6E, 0x69FF94, 0xFFFFA5, 0xD6ACFF, 0xFF92DF, 0xA4FFFF, 0xFFFFFF,
        ]),
    },
    Palette {
        id: "paper",
        name: "Paper",
        dark: false,
        bg_deep: c(0xF5F3EE),
        bg_panel: c(0xEAE7E0),
        bg_card: c(0xFFFFFF),
        bg_hover: c(0xECE9E2),
        border: c(0xDAD6CC),
        text: c(0x1E1C19),
        text_dim: c(0x6F6A61),
        accent: c(0xB4530F),
        on_accent: c(0xFFF8F0),
        accent2: c(0x2F6FA8),
        pink: c(0xB8336A),
        amber: c(0xB7791F),
        red: c(0xC0392B),
        green: c(0x2E8540),
        term_bg: c(0xFBFAF7),
        term_fg: c(0x24211D),
        ansi: ansi([
            0x24211D, 0xC0392B, 0x2E8540, 0x9A6A10, 0x2F6FA8, 0x8E44AD, 0x16808A, 0x6F6A61,
            0x4A4640, 0xD9534A, 0x3E9E52, 0xB7791F, 0x4A88C0, 0xA65FC4, 0x2399A3, 0x0E0D0B,
        ]),
    },
    Palette {
        id: "tokyo-night",
        name: "Tokyo Night",
        dark: true,
        bg_deep: c(0x1A1B26),
        bg_panel: c(0x16161E),
        bg_card: c(0x222436),
        bg_hover: c(0x292E42),
        border: c(0x2F334D),
        text: c(0xC0CAF5),
        text_dim: c(0x7A82AE),
        accent: c(0x7AA2F7),
        on_accent: c(0x0E1222),
        accent2: c(0xBB9AF7),
        pink: c(0xF7768E),
        amber: c(0xE0AF68),
        red: c(0xF7768E),
        green: c(0x9ECE6A),
        term_bg: c(0x16161E),
        term_fg: c(0xC0CAF5),
        ansi: ansi([
            0x15161E, 0xF7768E, 0x9ECE6A, 0xE0AF68, 0x7AA2F7, 0xBB9AF7, 0x7DCFFF, 0xA9B1D6,
            0x414868, 0xF7768E, 0x9ECE6A, 0xE0AF68, 0x7AA2F7, 0xBB9AF7, 0x7DCFFF, 0xC0CAF5,
        ]),
    },
    Palette {
        id: "gruvbox",
        name: "Gruvbox",
        dark: true,
        bg_deep: c(0x282828),
        bg_panel: c(0x1D2021),
        bg_card: c(0x32302F),
        bg_hover: c(0x3C3836),
        border: c(0x45403D),
        text: c(0xEBDBB2),
        text_dim: c(0xA89984),
        accent: c(0xFE8019),
        on_accent: c(0x1D1405),
        accent2: c(0x83A598),
        pink: c(0xD3869B),
        amber: c(0xFABD2F),
        red: c(0xFB4934),
        green: c(0xB8BB26),
        term_bg: c(0x1D2021),
        term_fg: c(0xEBDBB2),
        ansi: ansi([
            0x282828, 0xCC241D, 0x98971A, 0xD79921, 0x458588, 0xB16286, 0x689D6A, 0xA89984,
            0x928374, 0xFB4934, 0xB8BB26, 0xFABD2F, 0x83A598, 0xD3869B, 0x8EC07C, 0xEBDBB2,
        ]),
    },
    Palette {
        id: "catppuccin",
        name: "Catppuccin",
        dark: true,
        bg_deep: c(0x1E1E2E),
        bg_panel: c(0x181825),
        bg_card: c(0x262637),
        bg_hover: c(0x313244),
        border: c(0x3A3B50),
        text: c(0xCDD6F4),
        text_dim: c(0x9399B2),
        accent: c(0xCBA6F7),
        on_accent: c(0x1E1530),
        accent2: c(0x89B4FA),
        pink: c(0xF5C2E7),
        amber: c(0xF9E2AF),
        red: c(0xF38BA8),
        green: c(0xA6E3A1),
        term_bg: c(0x181825),
        term_fg: c(0xCDD6F4),
        ansi: ansi([
            0x45475A, 0xF38BA8, 0xA6E3A1, 0xF9E2AF, 0x89B4FA, 0xF5C2E7, 0x94E2D5, 0xBAC2DE,
            0x585B70, 0xF38BA8, 0xA6E3A1, 0xF9E2AF, 0x89B4FA, 0xF5C2E7, 0x94E2D5, 0xA6ADC8,
        ]),
    },
    Palette {
        id: "rose-pine",
        name: "Rosé Pine",
        dark: true,
        bg_deep: c(0x191724),
        bg_panel: c(0x15131F),
        bg_card: c(0x1F1D2E),
        bg_hover: c(0x26233A),
        border: c(0x2F2B43),
        text: c(0xE0DEF4),
        text_dim: c(0x908CAA),
        accent: c(0xEBBCBA),
        on_accent: c(0x2A1A1E),
        accent2: c(0x9CCFD8),
        pink: c(0xEB6F92),
        amber: c(0xF6C177),
        red: c(0xEB6F92),
        green: c(0x31748F),
        term_bg: c(0x15131F),
        term_fg: c(0xE0DEF4),
        ansi: ansi([
            0x26233A, 0xEB6F92, 0x31748F, 0xF6C177, 0x9CCFD8, 0xC4A7E7, 0xEBBCBA, 0xE0DEF4,
            0x6E6A86, 0xEB6F92, 0x3E8FB0, 0xF6C177, 0x9CCFD8, 0xC4A7E7, 0xEBBCBA, 0xE0DEF4,
        ]),
    },
    Palette {
        id: "solarized-dark",
        name: "Solarized Dark",
        dark: true,
        bg_deep: c(0x002B36),
        bg_panel: c(0x00232C),
        bg_card: c(0x073642),
        bg_hover: c(0x0B4250),
        border: c(0x174E5C),
        text: c(0xEEE8D5),
        text_dim: c(0x839496),
        accent: c(0x2AA198),
        on_accent: c(0x00191F),
        accent2: c(0x268BD2),
        pink: c(0xD33682),
        amber: c(0xB58900),
        red: c(0xDC322F),
        green: c(0x859900),
        term_bg: c(0x00252F),
        term_fg: c(0x93A1A1),
        ansi: ansi([
            0x073642, 0xDC322F, 0x859900, 0xB58900, 0x268BD2, 0xD33682, 0x2AA198, 0xEEE8D5,
            0x586E75, 0xCB4B16, 0x95A800, 0xC79A10, 0x4AA0E0, 0x6C71C4, 0x3BB5AB, 0xFDF6E3,
        ]),
    },
    Palette {
        id: "solarized-light",
        name: "Solarized Light",
        dark: false,
        bg_deep: c(0xFDF6E3),
        bg_panel: c(0xEEE8D5),
        bg_card: c(0xFFFCF2),
        bg_hover: c(0xF1EBD8),
        border: c(0xDDD6C1),
        text: c(0x073642),
        text_dim: c(0x657B83),
        accent: c(0x268BD2),
        on_accent: c(0xF5FBFF),
        accent2: c(0x2AA198),
        pink: c(0xD33682),
        amber: c(0xB58900),
        red: c(0xDC322F),
        green: c(0x859900),
        term_bg: c(0xFDF6E3),
        term_fg: c(0x586E75),
        ansi: ansi([
            0x073642, 0xDC322F, 0x859900, 0xB58900, 0x268BD2, 0xD33682, 0x2AA198, 0x93A1A1,
            0x002B36, 0xCB4B16, 0x6F8000, 0x9A7400, 0x1E6FA8, 0x6C71C4, 0x1F8A82, 0x073642,
        ]),
    },
    Palette {
        id: "github-light",
        name: "GitHub Light",
        dark: false,
        bg_deep: c(0xFFFFFF),
        bg_panel: c(0xF6F8FA),
        bg_card: c(0xFFFFFF),
        bg_hover: c(0xF3F4F6),
        border: c(0xD8DEE4),
        text: c(0x1F2328),
        text_dim: c(0x656D76),
        accent: c(0x0969DA),
        on_accent: c(0xFFFFFF),
        accent2: c(0x8250DF),
        pink: c(0xBF3989),
        amber: c(0x9A6700),
        red: c(0xCF222E),
        green: c(0x1A7F37),
        term_bg: c(0xFFFFFF),
        term_fg: c(0x1F2328),
        ansi: ansi([
            0x24292F, 0xCF222E, 0x116329, 0x4D2D00, 0x0969DA, 0x8250DF, 0x1B7C83, 0x6E7781,
            0x57606A, 0xA40E26, 0x1A7F37, 0x633C01, 0x218BFF, 0xA475F9, 0x3192AA, 0x8C959F,
        ]),
    },
];

static CURRENT: RwLock<Palette> = RwLock::new(THEMES[0]);

pub fn palette() -> Palette {
    *CURRENT.read().unwrap()
}

pub fn find(id: &str) -> Palette {
    THEMES
        .iter()
        .copied()
        .find(|t| t.id == id)
        .unwrap_or(THEMES[0])
}

pub fn bg_deep() -> Color32 {
    palette().bg_deep
}
pub fn bg_panel() -> Color32 {
    palette().bg_panel
}
pub fn bg_card() -> Color32 {
    palette().bg_card
}
pub fn bg_hover() -> Color32 {
    palette().bg_hover
}
pub fn border() -> Color32 {
    palette().border
}
pub fn text() -> Color32 {
    palette().text
}
pub fn text_dim() -> Color32 {
    palette().text_dim
}
pub fn accent() -> Color32 {
    palette().accent
}
pub fn accent2() -> Color32 {
    palette().accent2
}
pub fn pink() -> Color32 {
    palette().pink
}
pub fn amber() -> Color32 {
    palette().amber
}
pub fn red() -> Color32 {
    palette().red
}
pub fn green() -> Color32 {
    palette().green
}

/// Colour of an environment label.
pub fn env_color(env: crate::model::Env) -> Color32 {
    use crate::model::Env;
    match env {
        Env::Prod => red(),
        Env::Staging => amber(),
        Env::Test => Color32::from_rgb(110, 160, 245),
        Env::Dev => green(),
        Env::None => text_dim(),
    }
}
pub fn on_accent() -> Color32 {
    palette().on_accent
}

/// Group colors, picked to read on every palette.
pub const TAG_COLORS: [[u8; 3]; 8] = [
    [242, 163, 58],
    [121, 180, 224],
    [134, 199, 126],
    [229, 97, 91],
    [199, 146, 216],
    [108, 196, 184],
    [232, 115, 158],
    [176, 160, 120],
];

pub fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

// ------------------------------------------------------------------ type

pub const SANS: &str = "plex-sans";
pub const SANS_SEMI: &str = "plex-sans-semibold";
pub const MONO: &str = "plex-mono";
pub const MONO_MEDIUM: &str = "plex-mono-medium";

/// Registers IBM Plex (with egui's bundled fonts as fallback for symbols).
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    for (name, bytes) in [
        (
            SANS,
            &include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf")[..],
        ),
        (
            SANS_SEMI,
            &include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf")[..],
        ),
        (
            MONO,
            &include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf")[..],
        ),
        (
            MONO_MEDIUM,
            &include_bytes!("../assets/fonts/IBMPlexMono-Medium.ttf")[..],
        ),
    ] {
        fonts.font_data.insert(
            name.to_owned(),
            std::sync::Arc::new(FontData::from_static(bytes)),
        );
    }
    let family = |chain: &[&str]| {
        let mut v: Vec<String> = chain.iter().map(|s| s.to_string()).collect();
        v.push("NotoEmoji-Regular".to_owned());
        v.push("emoji-icon-font".to_owned());
        v
    };
    fonts
        .families
        .insert(FontFamily::Proportional, family(&[SANS, "Ubuntu-Light"]));
    // Hack covers box drawing and other terminal symbols Plex Mono lacks.
    fonts.families.insert(
        FontFamily::Monospace,
        family(&[MONO, "Hack", "Ubuntu-Light"]),
    );
    fonts.families.insert(
        FontFamily::Name(SANS_SEMI.into()),
        family(&[SANS_SEMI, SANS, "Ubuntu-Light"]),
    );
    fonts.families.insert(
        FontFamily::Name(MONO_MEDIUM.into()),
        family(&[MONO_MEDIUM, MONO, "Hack"]),
    );
    ctx.set_fonts(fonts);
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SANS_SEMI.into()))
}

pub fn mono_medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(MONO_MEDIUM.into()))
}

/// Page / dialog heading.
pub fn heading(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).font(semibold(size)).color(self::text())
}

/// Technical value: address, port, fingerprint.
pub fn mono(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text).font(FontId::monospace(size))
}

// ------------------------------------------------------------------ style

/// Activates a theme for the whole UI.
pub fn apply(ctx: &egui::Context, theme_id: &str) {
    let p = find(theme_id);
    *CURRENT.write().unwrap() = p;
    let accent = p.accent;
    let mut style = (*ctx.global_style()).clone();
    let v = &mut style.visuals;
    *v = if p.dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    v.panel_fill = p.bg_deep;
    v.window_fill = p.bg_card;
    v.extreme_bg_color = p.bg_card;
    v.faint_bg_color = p.bg_hover;
    v.code_bg_color = p.term_bg;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    let shadow: u8 = if p.dark { 150 } else { 36 };
    v.window_shadow = Shadow {
        offset: [0, 16],
        blur: 48,
        spread: 0,
        color: Color32::from_black_alpha(shadow),
    };
    v.popup_shadow = Shadow {
        offset: [0, 8],
        blur: 20,
        spread: 0,
        color: Color32::from_black_alpha((u16::from(shadow) * 2 / 3) as u8),
    };
    v.selection.bg_fill = accent.linear_multiply(0.30);
    v.selection.stroke = Stroke::new(1.0, accent);
    v.hyperlink_color = p.accent2;
    v.override_text_color = Some(p.text);
    v.text_cursor.stroke = Stroke::new(2.0, accent);

    let r = CornerRadius::same(6);
    v.widgets.noninteractive.bg_fill = p.bg_card;
    v.widgets.noninteractive.weak_bg_fill = p.bg_card;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.noninteractive.corner_radius = r;
    v.widgets.inactive.bg_fill = p.bg_hover;
    v.widgets.inactive.weak_bg_fill = p.bg_card;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.border);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.inactive.corner_radius = r;
    v.widgets.hovered.bg_fill = p.bg_hover;
    v.widgets.hovered.weak_bg_fill = p.bg_hover;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.text_dim.linear_multiply(0.6));
    v.widgets.hovered.fg_stroke = Stroke::new(1.5, p.text);
    v.widgets.hovered.corner_radius = r;
    v.widgets.active.bg_fill = accent.linear_multiply(0.25);
    v.widgets.active.weak_bg_fill = accent.linear_multiply(0.25);
    v.widgets.active.bg_stroke = Stroke::new(1.0, accent);
    v.widgets.active.fg_stroke = Stroke::new(1.5, p.text);
    v.widgets.active.corner_radius = r;
    v.widgets.open = v.widgets.active;

    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(12.0, 5.0);
    style.spacing.window_margin = Margin::same(22);
    style.spacing.interact_size.y = 30.0;
    style.spacing.text_edit_width = 260.0;
    style
        .text_styles
        .insert(egui::TextStyle::Body, FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, FontId::proportional(13.5));
    style
        .text_styles
        .insert(egui::TextStyle::Small, FontId::proportional(11.5));
    style
        .text_styles
        .insert(egui::TextStyle::Monospace, FontId::monospace(13.0));
    style
        .text_styles
        .insert(egui::TextStyle::Heading, semibold(22.0));
    ctx.set_global_style(style);
}

// ------------------------------------------------------------------ helpers

pub fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(
        l(a.r(), b.r()),
        l(a.g(), b.g()),
        l(a.b(), b.b()),
        l(a.a(), b.a()),
    )
}

/// Derives a stable color for things without an explicit one.
pub fn hash_color(s: &str) -> [u8; 3] {
    let mut h: u32 = 2166136261;
    for b in s.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    TAG_COLORS[(h as usize) % TAG_COLORS.len()]
}

// ------------------------------------------------------------------ widgets

/// Status LED: a small dot with a soft halo when lit.
pub fn led(painter: &egui::Painter, center: Pos2, color: Color32, lit: bool) {
    if lit {
        painter.circle_filled(center, 6.0, color.linear_multiply(0.18));
    }
    painter.circle_filled(
        center,
        3.2,
        if lit {
            color
        } else {
            color.linear_multiply(0.5)
        },
    );
}

/// Compact outlined mono label (protocol, tag, variable).
pub fn chip(ui: &mut egui::Ui, text: &str, color: Color32) -> egui::Response {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), FontId::monospace(11.0), color);
    let size = galley.size() + Vec2::new(10.0, 4.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(3),
        Stroke::new(1.0, color.linear_multiply(0.45)),
        egui::StrokeKind::Inside,
    );
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, color);
    resp
}

/// Brand mark: an amber patch jack (socket ring with a pin).
pub fn logo(ui: &mut egui::Ui, size: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    paint_logo(ui.painter(), rect);
    resp
}

pub fn paint_logo(p: &egui::Painter, rect: Rect) {
    let size = rect.width();
    p.rect_filled(rect, CornerRadius::same((size * 0.24) as u8), accent());
    let c = rect.center();
    p.circle_stroke(c, size * 0.27, Stroke::new(size * 0.09, on_accent()));
    p.circle_filled(c, size * 0.09, on_accent());
}

/// Solid accent button for the one primary action on a screen.
pub fn primary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), semibold(13.5), on_accent());
    let size = Vec2::new(galley.size().x + 26.0, 30.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let fill = if !ui.is_enabled() {
        accent().linear_multiply(0.35)
    } else if resp.hovered() {
        lerp(accent(), Color32::WHITE, 0.12)
    } else {
        accent()
    };
    ui.painter().rect_filled(rect, CornerRadius::same(6), fill);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, on_accent());
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Small uppercase mono label above a section.
pub fn section_title(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(spaced(&text.to_uppercase()))
            .font(FontId::monospace(10.5))
            .color(text_dim()),
    );
}

/// Uppercase label text. Plex Mono is already wide enough, so no extra tracking.
pub fn spaced(s: &str) -> String {
    s.to_string()
}

/// Flat surface for grouped settings / forms.
pub fn card_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(bg_card())
        .stroke(Stroke::new(1.0, border()))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(16))
}

/// Compact square icon button with tooltip (toolbars).
pub fn icon_button(ui: &mut egui::Ui, glyph: &str, tooltip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(30.0), egui::Sense::click());
    let enabled = ui.is_enabled();
    if resp.hovered() && enabled {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), bg_hover());
    }
    let color = if !enabled {
        text_dim().linear_multiply(0.5)
    } else if resp.hovered() {
        text()
    } else {
        text_dim()
    };
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        FontId::proportional(15.0),
        color,
    );
    resp.on_hover_text(tooltip)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Underline-style segmented control; returns the clicked index.
pub fn segmented(ui: &mut egui::Ui, items: &[&str], selected: usize) -> Option<usize> {
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 20.0;
        for (i, label) in items.iter().enumerate() {
            let active = i == selected;
            let font = if active {
                semibold(13.0)
            } else {
                FontId::proportional(13.0)
            };
            let galley = ui.painter().layout_no_wrap(label.to_string(), font, text());
            let (rect, resp) =
                ui.allocate_exact_size(galley.size() + Vec2::new(0.0, 10.0), egui::Sense::click());
            let color = if active || resp.hovered() {
                text()
            } else {
                text_dim()
            };
            ui.painter().galley(rect.left_top(), galley, color);
            if active {
                let y = rect.bottom() - 1.0;
                ui.painter().line_segment(
                    [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
                    Stroke::new(2.0, accent()),
                );
            }
            if resp
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                clicked = Some(i);
            }
        }
    });
    clicked
}

/// Thin full-width rule.
pub fn hairline(ui: &mut egui::Ui) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::ZERO, border());
}
