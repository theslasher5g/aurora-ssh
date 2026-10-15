//! Theming: switchable palettes (UI chrome + terminal) and shared widgets.

use egui::{Color32, CornerRadius, Margin, Pos2, Rect, Shadow, Stroke, Vec2};

use std::sync::RwLock;

/// A complete color scheme for UI chrome and terminal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub id: &'static str,
    pub name: &'static str,
    pub dark: bool,
    pub bg_deep: Color32,
    pub bg_panel: Color32,
    pub bg_card: Color32,
    pub bg_hover: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    /// Gradient start / end.
    pub accent: Color32,
    pub accent2: Color32,
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

pub const THEMES: [Palette; 6] = [
    Palette {
        id: "aurora",
        name: "Aurora",
        dark: true,
        bg_deep: c(0x0A0C12),
        bg_panel: c(0x0F121B),
        bg_card: c(0x161B27),
        bg_hover: c(0x1E2434),
        border: c(0x252C3D),
        text: c(0xE6E9F2),
        text_dim: c(0x8A93A8),
        accent: c(0x2DE2C3),
        accent2: c(0x8B5CF6),
        pink: c(0xF472B6),
        amber: c(0xFBBF24),
        red: c(0xF87171),
        green: c(0x4ADE80),
        term_bg: c(0x0B0D13),
        term_fg: c(0xDCE1EC),
        ansi: ansi([
            0x1C202C, 0xF87171, 0x4ADE80, 0xFBBF24, 0x60A5FA, 0xC084FC, 0x2DE2C3, 0xCBD5E1,
            0x58627A, 0xFCA5A5, 0x86EFAC, 0xFDE047, 0x93C5FD, 0xD8B4FE, 0x5EEAD4, 0xF8FAFC,
        ]),
    },
    Palette {
        id: "midnight",
        name: "Midnight",
        dark: true,
        bg_deep: c(0x09090B),
        bg_panel: c(0x111113),
        bg_card: c(0x18181B),
        bg_hover: c(0x232327),
        border: c(0x2A2A30),
        text: c(0xF4F4F5),
        text_dim: c(0x8E8E99),
        accent: c(0x3B82F6),
        accent2: c(0x06B6D4),
        pink: c(0xEC4899),
        amber: c(0xF59E0B),
        red: c(0xEF4444),
        green: c(0x22C55E),
        term_bg: c(0x0C0C0E),
        term_fg: c(0xE4E4E7),
        ansi: ansi([
            0x27272A, 0xEF4444, 0x22C55E, 0xEAB308, 0x3B82F6, 0xA855F7, 0x06B6D4, 0xD4D4D8,
            0x52525B, 0xF87171, 0x4ADE80, 0xFACC15, 0x60A5FA, 0xC084FC, 0x22D3EE, 0xFAFAFA,
        ]),
    },
    Palette {
        id: "nord",
        name: "Nord",
        dark: true,
        bg_deep: c(0x242933),
        bg_panel: c(0x2E3440),
        bg_card: c(0x3B4252),
        bg_hover: c(0x434C5E),
        border: c(0x4C566A),
        text: c(0xECEFF4),
        text_dim: c(0x9AA5B8),
        accent: c(0x88C0D0),
        accent2: c(0x81A1C1),
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
        bg_deep: c(0x1E1F29),
        bg_panel: c(0x21222C),
        bg_card: c(0x282A36),
        bg_hover: c(0x343746),
        border: c(0x44475A),
        text: c(0xF8F8F2),
        text_dim: c(0x9EA2C0),
        accent: c(0xBD93F9),
        accent2: c(0xFF79C6),
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
        id: "ember",
        name: "Ember",
        dark: true,
        bg_deep: c(0x120D0B),
        bg_panel: c(0x18120F),
        bg_card: c(0x221915),
        bg_hover: c(0x2D211C),
        border: c(0x3A2B24),
        text: c(0xF5EDE8),
        text_dim: c(0xA8968C),
        accent: c(0xFB923C),
        accent2: c(0xF43F5E),
        pink: c(0xF472B6),
        amber: c(0xFBBF24),
        red: c(0xEF4444),
        green: c(0x84CC16),
        term_bg: c(0x140F0D),
        term_fg: c(0xEDE3DC),
        ansi: ansi([
            0x2A201B, 0xEF4444, 0x84CC16, 0xF59E0B, 0x60A5FA, 0xE879F9, 0x2DD4BF, 0xE7DCD5,
            0x6B5A51, 0xF87171, 0xA3E635, 0xFBBF24, 0x93C5FD, 0xF0ABFC, 0x5EEAD4, 0xFFF7F2,
        ]),
    },
    Palette {
        id: "daylight",
        name: "Daylight",
        dark: false,
        bg_deep: c(0xF4F5F8),
        bg_panel: c(0xFFFFFF),
        bg_card: c(0xFFFFFF),
        bg_hover: c(0xEEF0F5),
        border: c(0xE1E4EB),
        text: c(0x1A1D26),
        text_dim: c(0x6B7280),
        accent: c(0x6366F1),
        accent2: c(0x14B8A6),
        pink: c(0xDB2777),
        amber: c(0xD97706),
        red: c(0xDC2626),
        green: c(0x16A34A),
        term_bg: c(0xFBFBFD),
        term_fg: c(0x1F2937),
        ansi: ansi([
            0x1F2937, 0xDC2626, 0x16A34A, 0xB45309, 0x2563EB, 0x9333EA, 0x0D9488, 0x6B7280,
            0x4B5563, 0xEF4444, 0x22C55E, 0xD97706, 0x3B82F6, 0xA855F7, 0x14B8A6, 0x111827,
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
pub fn teal() -> Color32 {
    palette().accent
}
pub fn violet() -> Color32 {
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

/// Text color that stays readable on top of the accent gradient.
pub fn on_accent() -> Color32 {
    Color32::from_rgb(10, 12, 18)
}

pub const TAG_COLORS: [[u8; 3]; 8] = [
    [45, 226, 195],
    [139, 92, 246],
    [244, 114, 182],
    [251, 191, 36],
    [248, 113, 113],
    [74, 222, 128],
    [56, 189, 248],
    [251, 146, 60],
];

pub fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

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
    v.panel_fill = p.bg_panel;
    v.window_fill = p.bg_card;
    v.extreme_bg_color = if p.dark { p.bg_deep } else { p.bg_hover };
    v.faint_bg_color = p.bg_card;
    v.code_bg_color = p.bg_deep;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = CornerRadius::same(14);
    v.menu_corner_radius = CornerRadius::same(10);
    let shadow: u8 = if p.dark { 140 } else { 40 };
    v.window_shadow = Shadow {
        offset: [0, 12],
        blur: 40,
        spread: 0,
        color: Color32::from_black_alpha(shadow),
    };
    v.popup_shadow = Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha((u16::from(shadow) * 5 / 6) as u8),
    };
    v.selection.bg_fill = accent.linear_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0, accent);
    v.hyperlink_color = accent;
    v.override_text_color = Some(p.text);

    let r = CornerRadius::same(8);
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
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, accent.linear_multiply(0.6));
    v.widgets.hovered.fg_stroke = Stroke::new(1.5, p.text);
    v.widgets.hovered.corner_radius = r;
    v.widgets.active.bg_fill = accent.linear_multiply(0.3);
    v.widgets.active.weak_bg_fill = accent.linear_multiply(0.3);
    v.widgets.active.bg_stroke = Stroke::new(1.0, accent);
    v.widgets.active.fg_stroke = Stroke::new(1.5, p.text);
    v.widgets.active.corner_radius = r;
    v.widgets.open = v.widgets.active;

    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(12.0, 6.0);
    style.spacing.window_margin = Margin::same(18);
    style.spacing.interact_size.y = 30.0;
    style.spacing.text_edit_width = 260.0;
    ctx.set_global_style(style);
}

/// Horizontal gradient rectangle with rounded corners (used for brand, avatars, highlights).
pub fn gradient_rect(
    painter: &egui::Painter,
    rect: Rect,
    left: Color32,
    right: Color32,
    radius: f32,
) {
    let r = radius
        .min(rect.width() / 2.0)
        .min(rect.height() / 2.0)
        .max(0.0);
    // Outline of the rounded rectangle; it is convex, so a triangle fan from the center fills it.
    let mut outline = Vec::new();
    let corners = [
        (rect.right_bottom() + Vec2::new(-r, -r), 0.0),
        (rect.left_bottom() + Vec2::new(r, -r), 90.0),
        (rect.left_top() + Vec2::new(r, r), 180.0),
        (rect.right_top() + Vec2::new(-r, r), 270.0),
    ];
    let segments = if r > 0.5 { 8 } else { 0 };
    for (center, start) in corners {
        for i in 0..=segments {
            let a = (start + 90.0 * i as f32 / segments.max(1) as f32).to_radians();
            outline.push(center + Vec2::new(a.cos(), a.sin()) * r);
        }
    }
    let color_at = |p: Pos2| {
        lerp(
            left,
            right,
            ((p.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0),
        )
    };
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(rect.center(), color_at(rect.center()));
    for p in &outline {
        mesh.colored_vertex(*p, color_at(*p));
    }
    let n = outline.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    painter.add(mesh);
}

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

/// Small rounded "chip" label.
pub fn chip(ui: &mut egui::Ui, text: &str, color: Color32) -> egui::Response {
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_string(), egui::FontId::proportional(11.5), color);
    let size = galley.size() + Vec2::new(14.0, 6.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(255), color.linear_multiply(0.16));
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, color);
    resp
}

/// Rounded avatar tile with gradient and a glyph in the middle.
pub fn avatar(ui: &mut egui::Ui, size: f32, color: Color32, glyph: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    let p = ui.painter();
    gradient_rect(p, rect, color, lerp(color, violet(), 0.55), size * 0.28);
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        egui::FontId::proportional(size * 0.48),
        on_accent(),
    );
    resp
}

/// Brand mark: gradient tile with a four-pointed "aurora" star.
pub fn logo(ui: &mut egui::Ui, size: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    let p = ui.painter();
    gradient_rect(p, rect, teal(), violet(), size * 0.28);
    let c = rect.center();
    let (outer, inner) = (size * 0.32, size * 0.08);
    let points: Vec<Pos2> = (0..8)
        .map(|i| {
            let angle = i as f32 * std::f32::consts::FRAC_PI_4 - std::f32::consts::FRAC_PI_2;
            let r = if i % 2 == 0 { outer } else { inner };
            c + Vec2::angled(angle) * r
        })
        .collect();
    // Draw as triangles from the center so the concave star fills correctly.
    let mut mesh = egui::Mesh::default();
    let dark = on_accent();
    mesh.colored_vertex(c, dark);
    for pt in &points {
        mesh.colored_vertex(*pt, dark);
    }
    for i in 0..8u32 {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % 8);
    }
    p.add(mesh);
    resp
}

/// Primary call-to-action button with gradient fill.
pub fn primary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        text.to_string(),
        egui::FontId::proportional(14.0),
        on_accent(),
    );
    let size = Vec2::new(galley.size().x + 28.0, 32.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let (l, r) = if resp.hovered() {
        (
            lerp(teal(), Color32::WHITE, 0.15),
            lerp(violet(), Color32::WHITE, 0.15),
        )
    } else {
        (teal(), violet())
    };
    gradient_rect(ui.painter(), rect, l, r, 9.0);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, on_accent());
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn section_title(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text.to_uppercase())
            .size(11.0)
            .color(text_dim())
            .strong(),
    );
}

pub fn card_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(bg_card())
        .stroke(Stroke::new(1.0, border()))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(14))
}

/// Compact square icon button with tooltip (toolbars).
pub fn icon_button(ui: &mut egui::Ui, glyph: &str, tooltip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(30.0), egui::Sense::click());
    let enabled = ui.is_enabled();
    if resp.hovered() && enabled {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(8), bg_hover());
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
        egui::FontId::proportional(15.0),
        color,
    );
    resp.on_hover_text(tooltip)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}
