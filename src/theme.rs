//! "Aurora" look: deep night background, teal→violet gradient accents.

use egui::{Color32, CornerRadius, Margin, Pos2, Rect, Shadow, Stroke, Vec2};

pub const BG_DEEP: Color32 = Color32::from_rgb(10, 12, 18);
pub const BG_PANEL: Color32 = Color32::from_rgb(15, 18, 27);
pub const BG_CARD: Color32 = Color32::from_rgb(22, 27, 39);
pub const BG_HOVER: Color32 = Color32::from_rgb(30, 36, 52);
pub const BORDER: Color32 = Color32::from_rgb(37, 44, 61);
pub const TEXT: Color32 = Color32::from_rgb(230, 233, 242);
pub const TEXT_DIM: Color32 = Color32::from_rgb(138, 147, 168);
pub const TEAL: Color32 = Color32::from_rgb(45, 226, 195);
pub const VIOLET: Color32 = Color32::from_rgb(139, 92, 246);
pub const PINK: Color32 = Color32::from_rgb(244, 114, 182);
pub const AMBER: Color32 = Color32::from_rgb(251, 191, 36);
pub const RED: Color32 = Color32::from_rgb(248, 113, 113);
pub const GREEN: Color32 = Color32::from_rgb(74, 222, 128);

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

pub fn apply(ctx: &egui::Context, accent: Color32) {
    let mut style = (*ctx.global_style()).clone();
    let v = &mut style.visuals;
    *v = egui::Visuals::dark();
    v.panel_fill = BG_PANEL;
    v.window_fill = BG_CARD;
    v.extreme_bg_color = BG_DEEP;
    v.faint_bg_color = BG_CARD;
    v.code_bg_color = BG_DEEP;
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_corner_radius = CornerRadius::same(14);
    v.menu_corner_radius = CornerRadius::same(10);
    v.window_shadow = Shadow {
        offset: [0, 12],
        blur: 40,
        spread: 0,
        color: Color32::from_black_alpha(140),
    };
    v.popup_shadow = Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(120),
    };
    v.selection.bg_fill = accent.linear_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0, accent);
    v.hyperlink_color = accent;
    v.override_text_color = Some(TEXT);

    let r = CornerRadius::same(8);
    v.widgets.noninteractive.bg_fill = BG_CARD;
    v.widgets.noninteractive.weak_bg_fill = BG_CARD;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.noninteractive.corner_radius = r;
    v.widgets.inactive.bg_fill = BG_HOVER;
    v.widgets.inactive.weak_bg_fill = BG_CARD;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.inactive.corner_radius = r;
    v.widgets.hovered.bg_fill = BG_HOVER;
    v.widgets.hovered.weak_bg_fill = BG_HOVER;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, accent.linear_multiply(0.6));
    v.widgets.hovered.fg_stroke = Stroke::new(1.5, TEXT);
    v.widgets.hovered.corner_radius = r;
    v.widgets.active.bg_fill = accent.linear_multiply(0.3);
    v.widgets.active.weak_bg_fill = accent.linear_multiply(0.3);
    v.widgets.active.bg_stroke = Stroke::new(1.0, accent);
    v.widgets.active.fg_stroke = Stroke::new(1.5, TEXT);
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
    gradient_rect(p, rect, color, lerp(color, VIOLET, 0.55), size * 0.28);
    p.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        egui::FontId::proportional(size * 0.48),
        Color32::from_rgb(12, 14, 20),
    );
    resp
}

/// Brand mark: gradient tile with a four-pointed "aurora" star.
pub fn logo(ui: &mut egui::Ui, size: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    let p = ui.painter();
    gradient_rect(p, rect, TEAL, VIOLET, size * 0.28);
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
    let dark = Color32::from_rgb(10, 12, 18);
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
        Color32::from_rgb(10, 12, 18),
    );
    let size = Vec2::new(galley.size().x + 28.0, 32.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let (l, r) = if resp.hovered() {
        (
            lerp(TEAL, Color32::WHITE, 0.15),
            lerp(VIOLET, Color32::WHITE, 0.15),
        )
    } else {
        (TEAL, VIOLET)
    };
    gradient_rect(ui.painter(), rect, l, r, 9.0);
    ui.painter().galley(
        rect.center() - galley.size() / 2.0,
        galley,
        Color32::from_rgb(10, 12, 18),
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub fn section_title(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text.to_uppercase())
            .size(11.0)
            .color(TEXT_DIM)
            .strong(),
    );
}

pub fn card_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(BG_CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(14))
}
