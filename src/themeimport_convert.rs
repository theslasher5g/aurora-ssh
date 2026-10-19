//! Turns an imported terminal colour scheme into a complete Aurora theme: the terminal
//! colours are kept (only nudged when unreadable), the app colours (panels, text, accent
//! and status colours) are derived from the background so the whole app looks coherent.

use egui::Color32;

use crate::theme::Palette;
use crate::themeimport_parse::{Rgb, Scheme};

fn col(c: Rgb) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

fn lin(v: u8) -> f32 {
    let v = v as f32 / 255.0;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// WCAG relative luminance.
pub fn luminance(c: Color32) -> f32 {
    0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
}

/// WCAG contrast ratio (1 … 21).
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// The scheme's background is dark (white text reads better on it than black).
pub fn is_dark(bg: Color32) -> bool {
    contrast(bg, Color32::WHITE) > contrast(bg, Color32::BLACK)
}

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    crate::theme::lerp(a, b, t.clamp(0.0, 1.0))
}

fn worst(c: Color32, bgs: &[Color32]) -> f32 {
    bgs.iter().map(|b| contrast(c, *b)).fold(f32::MAX, f32::min)
}

/// Moves `c` towards white or black until it reaches `target` contrast on every background
/// in `bgs`; keeps it untouched if it already does. If the target cannot be reached the best
/// of white and black is used.
pub fn ensure(c: Color32, bgs: &[Color32], target: f32) -> Color32 {
    if worst(c, bgs) >= target {
        return c;
    }
    let dir = if worst(Color32::WHITE, bgs) >= worst(Color32::BLACK, bgs) {
        Color32::WHITE
    } else {
        Color32::BLACK
    };
    for step in 1..=20 {
        let t = lerp(c, dir, step as f32 / 20.0);
        if worst(t, bgs) >= target {
            return t;
        }
    }
    dir
}

/// Saturation (HSV) and value of a colour, 0..1.
fn sat_val(c: Color32) -> (f32, f32) {
    let (r, g, b) = (c.r() as f32, c.g() as f32, c.b() as f32);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    if max <= 0.0 {
        (0.0, 0.0)
    } else {
        ((max - min) / max, max / 255.0)
    }
}

fn hue(c: Color32) -> f32 {
    let (r, g, b) = (c.r() as f32, c.g() as f32, c.b() as f32);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    if d <= 0.0 {
        return 0.0;
    }
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    h * 60.0
}

fn hue_distance(a: Color32, b: Color32) -> f32 {
    let d = (hue(a) - hue(b)).abs();
    d.min(360.0 - d)
}

/// Picks the accent: the scheme's blue (bright on dark, normal on light), else cyan, else
/// the most saturated colour of the ANSI palette.
fn pick_accent(ansi: &[Color32; 16], dark: bool) -> Color32 {
    let order: [usize; 4] = if dark { [12, 4, 14, 6] } else { [4, 12, 6, 14] };
    for i in order {
        if sat_val(ansi[i]).0 >= 0.30 {
            return ansi[i];
        }
    }
    let mut best = ansi[4];
    let mut best_score = -1.0;
    for i in [1, 2, 3, 4, 5, 6, 9, 10, 11, 12, 13, 14] {
        let (s, v) = sat_val(ansi[i]);
        let score = s * (0.4 + v);
        if score > best_score {
            best = ansi[i];
            best_score = score;
        }
    }
    best
}

/// A second signal colour that differs from the accent.
fn pick_accent2(ansi: &[Color32; 16], dark: bool, accent: Color32) -> Color32 {
    let order: [usize; 6] = if dark {
        [14, 13, 12, 6, 5, 4]
    } else {
        [6, 5, 4, 14, 13, 12]
    };
    for i in order {
        let c = ansi[i];
        if sat_val(c).0 >= 0.25 && hue_distance(c, accent) > 25.0 {
            return c;
        }
    }
    lerp(
        accent,
        if dark { Color32::WHITE } else { Color32::BLACK },
        0.35,
    )
}

/// An ANSI colour for a status role, or `fallback` when the scheme's slot is grey.
fn role(ansi: &[Color32; 16], dark: bool, slot: usize, fallback: Color32) -> Color32 {
    let c = ansi[if dark { slot + 8 } else { slot }];
    if sat_val(c).0 >= 0.25 { c } else { fallback }
}

pub fn palette_of(s: &Scheme) -> Palette {
    let bg = col(s.bg);
    let fg = col(s.fg);
    let ansi: [Color32; 16] = std::array::from_fn(|i| col(s.ansi[i]));
    let dark = is_dark(bg);
    let (white, black) = (Color32::WHITE, Color32::BLACK);

    // Surfaces.
    let bg_deep = bg;
    let (bg_panel, bg_card, bg_hover, border) = if dark {
        let panel = if luminance(bg) < 0.004 {
            lerp(bg, white, 0.04)
        } else {
            lerp(bg, black, 0.25)
        };
        (
            panel,
            lerp(bg, white, 0.05),
            lerp(bg, white, 0.10),
            lerp(bg, white, 0.17),
        )
    } else {
        (
            lerp(bg, black, 0.04),
            lerp(bg, white, 0.60),
            lerp(bg, black, 0.06),
            lerp(bg, black, 0.15),
        )
    };
    let mut surfaces = [bg_deep, bg_panel, bg_card, bg_hover];
    let (mut bg_panel, mut bg_card, mut bg_hover, mut border) =
        (bg_panel, bg_card, bg_hover, border);
    // A mid-grey background leaves no room for readable text on raised surfaces: then the
    // surfaces move away from the text colour instead.
    if worst(ensure(fg, &surfaces, 4.5), &surfaces) < 4.5 {
        let away = if dark { black } else { white };
        bg_panel = lerp(bg, away, 0.12);
        bg_card = lerp(bg, away, 0.06);
        bg_hover = lerp(bg, away, 0.18);
        border = lerp(bg, away, 0.28);
        surfaces = [bg_deep, bg_panel, bg_card, bg_hover];
    }

    // Text.
    let text = ensure(fg, &surfaces, 7.0);
    let text = if worst(text, &surfaces) < 4.5 {
        ensure(fg, &surfaces, 4.5)
    } else {
        text
    };
    let text_dim = ensure(lerp(text, bg_deep, 0.38), &surfaces, 4.5);
    let term_fg = ensure(fg, &[bg], 4.5);

    // Accent and signal colours.
    let accent = ensure(pick_accent(&ansi, dark), &[bg_deep, bg_card], 3.0);
    let accent2 = ensure(pick_accent2(&ansi, dark, accent), &[bg_deep, bg_card], 4.5);
    let on_accent = {
        let dk = lerp(accent, black, 0.88);
        let lt = lerp(accent, white, 0.94);
        let best = if contrast(dk, accent) >= contrast(lt, accent) {
            dk
        } else {
            lt
        };
        if contrast(best, accent) >= 4.5 {
            best
        } else if contrast(black, accent) >= contrast(white, accent) {
            black
        } else {
            white
        }
    };
    let signal = |slot: usize, fallback: u32| {
        let fb = Color32::from_rgb(
            (fallback >> 16) as u8,
            (fallback >> 8) as u8,
            fallback as u8,
        );
        ensure(role(&ansi, dark, slot, fb), &[bg_deep, bg_card], 4.5)
    };
    let (red, green, amber, pink) = if dark {
        (
            signal(1, 0xFF6961),
            signal(2, 0x32D74B),
            signal(3, 0xFFB340),
            signal(5, 0xFF6FA8),
        )
    } else {
        (
            signal(1, 0xC0392B),
            signal(2, 0x2E8540),
            signal(3, 0xB7791F),
            signal(5, 0xB8336A),
        )
    };

    Palette {
        id: crate::theme::intern(&s.id),
        name: crate::theme::intern(&s.name),
        dark,
        bg_deep,
        bg_panel,
        bg_card,
        bg_hover,
        border,
        text,
        text_dim,
        accent,
        on_accent,
        accent2,
        pink,
        amber,
        red,
        green,
        term_bg: bg,
        term_fg,
        ansi,
    }
}

/// Makes the imported themes known to the theme picker, the palette and the terminal.
pub fn register(list: &[Scheme]) {
    crate::theme::set_custom(
        list.iter()
            .map(|s| (palette_of(s), s.cursor.map(col), s.selection.map(col)))
            .collect(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::themeimport_parse::{Scheme, XTERM};

    fn scheme(bg: Rgb, fg: Rgb) -> Scheme {
        Scheme {
            id: "custom-test".into(),
            name: "Test".into(),
            bg,
            fg,
            ..Default::default()
        }
    }

    /// Dracula, Solarized Light, Solarized Dark, Gruvbox light, plus awkward ones.
    fn samples() -> Vec<Scheme> {
        let mut v = vec![
            scheme([0x28, 0x2a, 0x36], [0xf8, 0xf8, 0xf2]),
            scheme([0xfd, 0xf6, 0xe3], [0x65, 0x7b, 0x83]),
            scheme([0x00, 0x2b, 0x36], [0x83, 0x94, 0x96]),
            scheme([0xfb, 0xf1, 0xc7], [0x3c, 0x38, 0x36]),
            // Pathological: no contrast at all, pure black/white, mid grey, saturated.
            scheme([0x80, 0x80, 0x80], [0x82, 0x82, 0x82]),
            scheme([0, 0, 0], [0, 0, 0]),
            scheme([255, 255, 255], [255, 255, 255]),
            scheme([0x76, 0x76, 0x76], [0x77, 0x77, 0x77]),
            scheme([0x00, 0x00, 0x80], [0xff, 0xff, 0x00]),
            scheme([0xff, 0x00, 0xff], [0x00, 0xff, 0x00]),
        ];
        // A scheme whose ANSI colours are all grey.
        let mut grey = scheme([0x1e, 0x1e, 0x1e], [0xcc, 0xcc, 0xcc]);
        grey.ansi = [[0x88, 0x88, 0x88]; 16];
        v.push(grey);
        v
    }

    #[test]
    fn contrast_rules_hold_for_every_sample() {
        for s in samples() {
            let p = palette_of(&s);
            let bgs = [p.bg_deep, p.bg_panel, p.bg_card, p.bg_hover];
            let tag = format!("bg {:?} fg {:?}", s.bg, s.fg);
            assert!(
                worst(p.text, &bgs) >= 4.5,
                "text {tag}: {}",
                worst(p.text, &bgs)
            );
            assert!(worst(p.text_dim, &bgs) >= 4.5, "dim {tag}");
            assert!(contrast(p.term_fg, p.term_bg) >= 4.5, "term {tag}");
            assert!(contrast(p.on_accent, p.accent) >= 4.5, "on accent {tag}");
            assert!(
                worst(p.accent, &[p.bg_deep, p.bg_card]) >= 3.0,
                "accent {tag}"
            );
            for (n, c) in [("red", p.red), ("green", p.green), ("amber", p.amber)] {
                assert!(worst(c, &[p.bg_deep, p.bg_card]) >= 4.5, "{n} {tag}");
            }
            assert_eq!(p.dark, is_dark(col(s.bg)), "dark {tag}");
            assert_eq!(p.term_bg, col(s.bg));
        }
    }

    #[test]
    fn readable_terminal_colours_are_not_touched() {
        let s = scheme([0x28, 0x2a, 0x36], [0xf8, 0xf8, 0xf2]);
        let p = palette_of(&s);
        assert_eq!(p.term_fg, col(s.fg));
        assert_eq!(p.term_bg, col(s.bg));
        assert_eq!(p.ansi[1], col(s.ansi[1]));
    }

    #[test]
    fn unreadable_terminal_text_is_corrected_towards_contrast() {
        let s = scheme([0x20, 0x20, 0x20], [0x30, 0x30, 0x30]);
        let p = palette_of(&s);
        assert!(contrast(p.term_fg, p.term_bg) >= 4.5);
        assert!(luminance(p.term_fg) > luminance(p.term_bg));
    }

    #[test]
    fn light_and_dark_are_detected_from_the_background() {
        assert!(palette_of(&scheme([0x10, 0x10, 0x10], [0xee, 0xee, 0xee])).dark);
        assert!(!palette_of(&scheme([0xf5, 0xf5, 0xf0], [0x22, 0x22, 0x22])).dark);
    }

    #[test]
    fn panels_follow_the_background() {
        let dark = palette_of(&scheme([0x28, 0x2a, 0x36], [0xf8, 0xf8, 0xf2]));
        assert!(luminance(dark.bg_card) > luminance(dark.bg_deep));
        assert!(luminance(dark.bg_panel) < luminance(dark.bg_deep));
        assert!(luminance(dark.bg_hover) > luminance(dark.bg_card));
        let light = palette_of(&scheme([0xf5, 0xf3, 0xee], [0x1e, 0x1c, 0x19]));
        assert!(luminance(light.bg_card) > luminance(light.bg_deep));
        assert!(luminance(light.bg_panel) < luminance(light.bg_deep));
    }

    #[test]
    fn accent_is_the_blue_of_the_scheme() {
        let mut s = scheme([0x28, 0x2a, 0x36], [0xf8, 0xf8, 0xf2]);
        s.ansi[12] = [0x5a, 0x9b, 0xff];
        let p = palette_of(&s);
        assert!(
            hue(p.accent) > 190.0 && hue(p.accent) < 260.0,
            "{:?}",
            p.accent
        );
    }

    #[test]
    fn accent_falls_back_to_the_most_saturated_colour() {
        let mut s = scheme([0x1e, 0x1e, 0x1e], [0xdd, 0xdd, 0xdd]);
        s.ansi = [[0x80, 0x80, 0x80]; 16];
        s.ansi[3] = [0xff, 0xa0, 0x00];
        let p = palette_of(&s);
        assert!(
            hue(p.accent) > 20.0 && hue(p.accent) < 50.0,
            "{:?}",
            p.accent
        );
        // Grey slots fall back to the built-in status colours.
        assert!(sat_val(p.red).0 > 0.3);
    }

    #[test]
    fn accent2_differs_from_accent() {
        let p = palette_of(&Scheme {
            id: "x".into(),
            name: "X".into(),
            ..Default::default()
        });
        assert!(hue_distance(p.accent, p.accent2) > 20.0 || p.accent == p.accent2);
        let _ = XTERM;
    }

    #[test]
    fn ensure_keeps_good_colours_and_fixes_bad_ones() {
        let bg = Color32::from_rgb(0x20, 0x20, 0x20);
        let good = Color32::from_rgb(0xee, 0xee, 0xee);
        assert_eq!(ensure(good, &[bg], 7.0), good);
        let bad = Color32::from_rgb(0x30, 0x30, 0x30);
        assert!(contrast(ensure(bad, &[bg], 4.5), bg) >= 4.5);
        // Unreachable target: still the better extreme.
        let mid = Color32::from_rgb(0x77, 0x77, 0x77);
        let e = ensure(mid, &[mid], 12.0);
        assert!(e == Color32::WHITE || e == Color32::BLACK);
    }
}
