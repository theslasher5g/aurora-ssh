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

pub const THEMES: [Palette; 26] = [
    Palette {
        id: "phosphor",
        name: "Phosphor",
        dark: true,
        bg_deep: c(0x000000),
        bg_panel: c(0x161618),
        bg_card: c(0x1C1C1E),
        bg_hover: c(0x2C2C2E),
        border: c(0x2C2C2E),
        text: c(0xF5F5F7),
        text_dim: c(0xA1A1A6),
        accent: c(0xFF9F0A),
        on_accent: c(0x1D1D1F),
        accent2: c(0x64B5FF),
        pink: c(0xFF6FA8),
        amber: c(0xFFB340),
        red: c(0xFF6961),
        green: c(0x32D74B),
        term_bg: c(0x0F0F11),
        term_fg: c(0xF0F0F2),
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
    Palette {
        id: "one-dark",
        name: "One Dark",
        dark: true,
        bg_deep: c(0x21252B),
        bg_panel: c(0x1B1F24),
        bg_card: c(0x282C34),
        bg_hover: c(0x2F343D),
        border: c(0x3A3F4B),
        text: c(0xDCDFE4),
        text_dim: c(0x8B929E),
        accent: c(0x61AFEF),
        on_accent: c(0x0E1620),
        accent2: c(0xC678DD),
        pink: c(0xE06C9F),
        amber: c(0xE5C07B),
        red: c(0xE06C75),
        green: c(0x98C379),
        term_bg: c(0x1E2227),
        term_fg: c(0xDCDFE4),
        ansi: ansi([
            0x282C34, 0xE06C75, 0x98C379, 0xE5C07B, 0x61AFEF, 0xC678DD, 0x56B6C2, 0xDCDFE4,
            0x5C6370, 0xE06C75, 0x98C379, 0xE5C07B, 0x61AFEF, 0xC678DD, 0x56B6C2, 0xFFFFFF,
        ]),
    },
    Palette {
        id: "monokai",
        name: "Monokai",
        dark: true,
        bg_deep: c(0x221F22),
        bg_panel: c(0x1B181B),
        bg_card: c(0x2D2A2E),
        bg_hover: c(0x363337),
        border: c(0x403E41),
        text: c(0xFCFCFA),
        text_dim: c(0x939293),
        accent: c(0xFFD866),
        on_accent: c(0x221F22),
        accent2: c(0x78DCE8),
        pink: c(0xFF6188),
        amber: c(0xFC9867),
        red: c(0xFF6188),
        green: c(0xA9DC76),
        term_bg: c(0x1E1B1E),
        term_fg: c(0xFCFCFA),
        ansi: ansi([
            0x403E41, 0xFF6188, 0xA9DC76, 0xFFD866, 0xFC9867, 0xAB9DF2, 0x78DCE8, 0xFCFCFA,
            0x727072, 0xFF6188, 0xA9DC76, 0xFFD866, 0xFC9867, 0xAB9DF2, 0x78DCE8, 0xFFFFFF,
        ]),
    },
    Palette {
        id: "everforest",
        name: "Everforest",
        dark: true,
        bg_deep: c(0x272E33),
        bg_panel: c(0x1E2326),
        bg_card: c(0x2E383C),
        bg_hover: c(0x374145),
        border: c(0x414B50),
        text: c(0xD3C6AA),
        text_dim: c(0x9DA9A0),
        accent: c(0xA7C080),
        on_accent: c(0x1E2326),
        accent2: c(0x7FBBB3),
        pink: c(0xD699B6),
        amber: c(0xDBBC7F),
        red: c(0xE67E80),
        green: c(0xA7C080),
        term_bg: c(0x232A2E),
        term_fg: c(0xD3C6AA),
        ansi: ansi([
            0x414B50, 0xE67E80, 0xA7C080, 0xDBBC7F, 0x7FBBB3, 0xD699B6, 0x83C092, 0xD3C6AA,
            0x5C6A72, 0xE67E80, 0xA7C080, 0xDBBC7F, 0x7FBBB3, 0xD699B6, 0x83C092, 0xF2EFDF,
        ]),
    },
    Palette {
        id: "kanagawa",
        name: "Kanagawa",
        dark: true,
        bg_deep: c(0x1F1F28),
        bg_panel: c(0x16161D),
        bg_card: c(0x2A2A37),
        bg_hover: c(0x363646),
        border: c(0x3B3B4F),
        text: c(0xDCD7BA),
        text_dim: c(0x938AA9),
        accent: c(0xFFA066),
        on_accent: c(0x1F1F28),
        accent2: c(0x7E9CD8),
        pink: c(0xD27E99),
        amber: c(0xE6C384),
        red: c(0xE46876),
        green: c(0x98BB6C),
        term_bg: c(0x1A1A22),
        term_fg: c(0xDCD7BA),
        ansi: ansi([
            0x16161D, 0xC34043, 0x76946A, 0xC0A36E, 0x7E9CD8, 0x957FB8, 0x6A9589, 0xC8C093,
            0x727169, 0xE82424, 0x98BB6C, 0xE6C384, 0x7FB4CA, 0x938AA9, 0x7AA89F, 0xDCD7BA,
        ]),
    },
    Palette {
        id: "ayu-mirage",
        name: "Ayu Mirage",
        dark: true,
        bg_deep: c(0x1F2430),
        bg_panel: c(0x191E2A),
        bg_card: c(0x242936),
        bg_hover: c(0x2D3343),
        border: c(0x343B4C),
        text: c(0xCCCAC2),
        text_dim: c(0x8A9199),
        accent: c(0xFFCC66),
        on_accent: c(0x1F2430),
        accent2: c(0x73D0FF),
        pink: c(0xF28779),
        amber: c(0xFFD173),
        red: c(0xF28779),
        green: c(0xD5FF80),
        term_bg: c(0x1A1F29),
        term_fg: c(0xCCCAC2),
        ansi: ansi([
            0x171B24, 0xED8274, 0x87D96C, 0xFACC6E, 0x6DCBFA, 0xDABAFA, 0x90E1C6, 0xC7C7C7,
            0x686868, 0xF28779, 0xD5FF80, 0xFFD173, 0x73D0FF, 0xDFBFFF, 0x95E6CB, 0xFFFFFF,
        ]),
    },
    Palette {
        id: "night-owl",
        name: "Night Owl",
        dark: true,
        bg_deep: c(0x011627),
        bg_panel: c(0x010E1A),
        bg_card: c(0x0B2942),
        bg_hover: c(0x112F4A),
        border: c(0x1D3B53),
        text: c(0xD6DEEB),
        text_dim: c(0x7E97AC),
        accent: c(0x82AAFF),
        on_accent: c(0x011627),
        accent2: c(0xC792EA),
        pink: c(0xFF5874),
        amber: c(0xECC48D),
        red: c(0xEF5350),
        green: c(0x22DA6E),
        term_bg: c(0x011221),
        term_fg: c(0xD6DEEB),
        ansi: ansi([
            0x011627, 0xEF5350, 0x22DA6E, 0xADDB67, 0x82AAFF, 0xC792EA, 0x21C7A8, 0xD6DEEB,
            0x575656, 0xEF5350, 0x22DA6E, 0xFFEB95, 0x82AAFF, 0xC792EA, 0x7FDBCA, 0xFFFFFF,
        ]),
    },
    Palette {
        id: "synthwave",
        name: "Synthwave",
        dark: true,
        bg_deep: c(0x241B2F),
        bg_panel: c(0x1C1526),
        bg_card: c(0x2A2139),
        bg_hover: c(0x342A46),
        border: c(0x3E3354),
        text: c(0xF4EEE4),
        text_dim: c(0xA599BE),
        accent: c(0xFF7EDB),
        on_accent: c(0x241B2F),
        accent2: c(0x36F9F6),
        pink: c(0xFF7EDB),
        amber: c(0xFEDE5D),
        red: c(0xFE4450),
        green: c(0x72F1B8),
        term_bg: c(0x1E1727),
        term_fg: c(0xF4EEE4),
        ansi: ansi([
            0x241B2F, 0xFE4450, 0x72F1B8, 0xFEDE5D, 0x03EDF9, 0xFF7EDB, 0x36F9F6, 0xF4EEE4,
            0x6D77B3, 0xFE4450, 0x72F1B8, 0xFFF951, 0x36F9F6, 0xFF7EDB, 0x36F9F6, 0xFFFFFF,
        ]),
    },
    Palette {
        id: "midnight",
        name: "Midnight",
        dark: true,
        bg_deep: c(0x000000),
        bg_panel: c(0x0C0D10),
        bg_card: c(0x15171C),
        bg_hover: c(0x1E2128),
        border: c(0x23262E),
        text: c(0xECEFF4),
        text_dim: c(0x8E95A3),
        accent: c(0x4C8DFF),
        on_accent: c(0xFFFFFF),
        accent2: c(0x9D7BFF),
        pink: c(0xFF6FAE),
        amber: c(0xFFB547),
        red: c(0xFF5C5C),
        green: c(0x3DDC84),
        term_bg: c(0x07080A),
        term_fg: c(0xE6E9EF),
        ansi: ansi([
            0x1E2128, 0xFF5C5C, 0x3DDC84, 0xFFB547, 0x4C8DFF, 0x9D7BFF, 0x3FC8E0, 0xD5DAE3,
            0x5A6170, 0xFF7B7B, 0x6BE8A2, 0xFFCA75, 0x78AAFF, 0xB79CFF, 0x6EDCEF, 0xFFFFFF,
        ]),
    },
    Palette {
        id: "matrix",
        name: "Matrix",
        dark: true,
        bg_deep: c(0x050A06),
        bg_panel: c(0x030603),
        bg_card: c(0x0B140C),
        bg_hover: c(0x122014),
        border: c(0x18301B),
        text: c(0xC8F7CF),
        text_dim: c(0x6FA978),
        accent: c(0x39FF6A),
        on_accent: c(0x031206),
        accent2: c(0x7DE8FF),
        pink: c(0xFF7AC6),
        amber: c(0xE9F56B),
        red: c(0xFF5A5A),
        green: c(0x39FF6A),
        term_bg: c(0x020502),
        term_fg: c(0xA8F5B4),
        ansi: ansi([
            0x0B140C, 0xFF5A5A, 0x39FF6A, 0xE9F56B, 0x4FC3F7, 0xC792EA, 0x5EF2C4, 0xA8F5B4,
            0x3B6B43, 0xFF7B7B, 0x7DFF9C, 0xF3FA9A, 0x7DD6FA, 0xD8B0F0, 0x8FF7D8, 0xE6FFE9,
        ]),
    },
    Palette {
        id: "catppuccin-latte",
        name: "Catppuccin Latte",
        dark: false,
        bg_deep: c(0xEFF1F5),
        bg_panel: c(0xE6E9EF),
        bg_card: c(0xFFFFFF),
        bg_hover: c(0xDCE0E8),
        border: c(0xCCD0DA),
        text: c(0x4C4F69),
        text_dim: c(0x7C7F93),
        accent: c(0x8839EF),
        on_accent: c(0xFFFFFF),
        accent2: c(0x1E66F5),
        pink: c(0xEA76CB),
        amber: c(0xDF8E1D),
        red: c(0xD20F39),
        green: c(0x40A02B),
        term_bg: c(0xF6F7FA),
        term_fg: c(0x4C4F69),
        ansi: ansi([
            0x5C5F77, 0xD20F39, 0x40A02B, 0xDF8E1D, 0x1E66F5, 0xEA76CB, 0x179299, 0xACB0BE,
            0x6C6F85, 0xD20F39, 0x40A02B, 0xDF8E1D, 0x1E66F5, 0xEA76CB, 0x179299, 0xBCC0CC,
        ]),
    },
    Palette {
        id: "rose-pine-dawn",
        name: "Rosé Pine Dawn",
        dark: false,
        bg_deep: c(0xFAF4ED),
        bg_panel: c(0xF2E9E1),
        bg_card: c(0xFFFAF3),
        bg_hover: c(0xF2E9DE),
        border: c(0xDFDAD9),
        text: c(0x464261),
        text_dim: c(0x797593),
        accent: c(0xD7827E),
        on_accent: c(0xFFFAF3),
        accent2: c(0x286983),
        pink: c(0xB4637A),
        amber: c(0xEA9D34),
        red: c(0xB4637A),
        green: c(0x286983),
        term_bg: c(0xFFFAF3),
        term_fg: c(0x464261),
        ansi: ansi([
            0xF2E9E1, 0xB4637A, 0x286983, 0xEA9D34, 0x56949F, 0x907AA9, 0xD7827E, 0x575279,
            0x9893A5, 0xB4637A, 0x286983, 0xEA9D34, 0x56949F, 0x907AA9, 0xD7827E, 0x575279,
        ]),
    },
    Palette {
        id: "one-light",
        name: "One Light",
        dark: false,
        bg_deep: c(0xFAFAFA),
        bg_panel: c(0xF0F0F1),
        bg_card: c(0xFFFFFF),
        bg_hover: c(0xEAEAEB),
        border: c(0xDBDBDC),
        text: c(0x383A42),
        text_dim: c(0x696C77),
        accent: c(0x4078F2),
        on_accent: c(0xFFFFFF),
        accent2: c(0xA626A4),
        pink: c(0xCA1243),
        amber: c(0xC18401),
        red: c(0xE45649),
        green: c(0x50A14F),
        term_bg: c(0xFAFAFA),
        term_fg: c(0x383A42),
        ansi: ansi([
            0x383A42, 0xE45649, 0x50A14F, 0xC18401, 0x4078F2, 0xA626A4, 0x0184BC, 0xA0A1A7,
            0x4F525D, 0xE45649, 0x50A14F, 0x986801, 0x4078F2, 0xA626A4, 0x0997B3, 0x383A42,
        ]),
    },
    Palette {
        id: "ayu-light",
        name: "Ayu Light",
        dark: false,
        bg_deep: c(0xF8F9FA),
        bg_panel: c(0xEEF0F2),
        bg_card: c(0xFFFFFF),
        bg_hover: c(0xE7EAED),
        border: c(0xD8DCE0),
        text: c(0x3D4247),
        text_dim: c(0x6B727A),
        accent: c(0xFA8D3E),
        on_accent: c(0x1F1300),
        accent2: c(0x399EE6),
        pink: c(0xF07171),
        amber: c(0xF2AE49),
        red: c(0xE65050),
        green: c(0x6CBF43),
        term_bg: c(0xFCFCFC),
        term_fg: c(0x3D4247),
        ansi: ansi([
            0x000000, 0xEA6C6D, 0x6CBF43, 0xECA944, 0x3199E1, 0x9E75C7, 0x46BA94, 0xBABABA,
            0x686868, 0xF07171, 0x86B300, 0xF2AE49, 0x399EE6, 0xA37ACC, 0x4CBF99, 0xD1D1D1,
        ]),
    },
    Palette {
        id: "nord-light",
        name: "Nord Light",
        dark: false,
        bg_deep: c(0xECEFF4),
        bg_panel: c(0xE5E9F0),
        bg_card: c(0xFFFFFF),
        bg_hover: c(0xDDE2EA),
        border: c(0xD0D6E0),
        text: c(0x2E3440),
        text_dim: c(0x5E6B80),
        accent: c(0x5E81AC),
        on_accent: c(0xFFFFFF),
        accent2: c(0xB48EAD),
        pink: c(0xB48EAD),
        amber: c(0xD08770),
        red: c(0xBF616A),
        green: c(0x8AA16E),
        term_bg: c(0xF3F5F8),
        term_fg: c(0x2E3440),
        ansi: ansi([
            0x3B4252, 0xBF616A, 0x6F8F55, 0xC08B30, 0x5E81AC, 0xB48EAD, 0x4C8E9E, 0x8E99AE,
            0x4C566A, 0xBF616A, 0x8AA16E, 0xD08770, 0x81A1C1, 0xB48EAD, 0x88C0D0, 0x2E3440,
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
        .or_else(|| custom_palettes().into_iter().find(|t| t.id == id))
        .unwrap_or(THEMES[0])
}

/// An imported theme with the colors a `Palette` cannot hold.
#[derive(Clone, Copy)]
struct CustomEntry {
    palette: Palette,
    cursor: Option<Color32>,
    selection: Option<Color32>,
}

static CUSTOM: RwLock<Vec<CustomEntry>> = RwLock::new(Vec::new());
static INTERNED: RwLock<Vec<&'static str>> = RwLock::new(Vec::new());

/// A `'static` copy of `s` (palettes of imported themes carry `&'static str` names). Equal
/// strings share one copy, so renaming back and forth does not grow memory.
pub fn intern(s: &str) -> &'static str {
    if let Some(found) = INTERNED.read().unwrap().iter().find(|x| **x == s) {
        return found;
    }
    let leaked: &'static str = Box::leak(s.to_string().into_boxed_str());
    INTERNED.write().unwrap().push(leaked);
    leaked
}

/// Replaces the imported themes: (palette, cursor color, selection color).
pub fn set_custom(list: Vec<(Palette, Option<Color32>, Option<Color32>)>) {
    *CUSTOM.write().unwrap() = list
        .into_iter()
        .map(|(palette, cursor, selection)| CustomEntry {
            palette,
            cursor,
            selection,
        })
        .collect();
}

/// Imported themes, in the order they were added.
pub fn custom_palettes() -> Vec<Palette> {
    CUSTOM.read().unwrap().iter().map(|e| e.palette).collect()
}

/// Built-in themes followed by the imported ones.
pub fn all_themes() -> Vec<Palette> {
    THEMES.iter().copied().chain(custom_palettes()).collect()
}

/// Cursor and selection color an imported theme brings along (built-in themes: `None`,
/// the terminal then derives both from the accent).
pub fn term_extras(p: &Palette) -> (Option<Color32>, Option<Color32>) {
    CUSTOM
        .read()
        .unwrap()
        .iter()
        .find(|e| e.palette.id == p.id)
        .map(|e| (e.cursor, e.selection))
        .unwrap_or((None, None))
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

/// Translucent overlay of the text colour (dark) or black (light) at `alpha`.
fn veil(alpha: f32) -> Color32 {
    let p = palette();
    let base = if p.dark { p.text } else { Color32::BLACK };
    let a = if p.dark { alpha } else { alpha * 0.7 };
    Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), (a * 255.0).round() as u8)
}

/// Sidebar background.
pub fn side() -> Color32 {
    palette().bg_panel
}
/// Cards and lists.
pub fn surface() -> Color32 {
    palette().bg_card
}
/// Fill of buttons, fields and segmented controls.
pub fn fill() -> Color32 {
    veil(0.09)
}
pub fn fill_strong() -> Color32 {
    veil(0.15)
}
/// Row hover.
pub fn hover() -> Color32 {
    veil(0.045)
}
/// Hairline between rows.
pub fn line() -> Color32 {
    veil(0.08)
}
/// Selected segment.
pub fn seg_on() -> Color32 {
    let p = palette();
    if p.dark {
        lerp(p.bg_card, p.text, 0.33)
    } else {
        Color32::WHITE
    }
}
/// Selected nav item, placeholders.
pub fn accent_soft() -> Color32 {
    soft(accent())
}
/// Tinted background for a coloured label.
pub fn soft(c: Color32) -> Color32 {
    let a = if palette().dark { 0.18 } else { 0.14 };
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a * 255.0) as u8)
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

pub const SANS: &str = "inter";
pub const SANS_MEDIUM: &str = "inter-medium";
pub const SANS_SEMI: &str = "inter-semibold";
pub const SANS_BOLD: &str = "inter-bold";
pub const MONO: &str = "plex-mono";
pub const MONO_MEDIUM: &str = "plex-mono-medium";

/// Registers Inter for the UI and IBM Plex Mono for technical text (with egui's bundled
/// fonts as fallback for symbols).
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    for (name, bytes) in [
        (
            SANS,
            &include_bytes!("../assets/fonts/Inter-Regular.otf")[..],
        ),
        (
            SANS_MEDIUM,
            &include_bytes!("../assets/fonts/Inter-Medium.otf")[..],
        ),
        (
            SANS_SEMI,
            &include_bytes!("../assets/fonts/Inter-SemiBold.otf")[..],
        ),
        (
            SANS_BOLD,
            &include_bytes!("../assets/fonts/Inter-Bold.otf")[..],
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
    fonts.families.insert(
        FontFamily::Proportional,
        family(&[SANS, "Ubuntu-Light", MONO]),
    );
    // Hack covers box drawing and other terminal symbols Plex Mono lacks.
    fonts.families.insert(
        FontFamily::Monospace,
        family(&[MONO, "Hack", "Ubuntu-Light"]),
    );
    for weight in [SANS_MEDIUM, SANS_SEMI, SANS_BOLD] {
        fonts.families.insert(
            FontFamily::Name(weight.into()),
            family(&[weight, SANS, "Ubuntu-Light", MONO]),
        );
    }
    fonts.families.insert(
        FontFamily::Name(MONO_MEDIUM.into()),
        family(&[MONO_MEDIUM, MONO, "Hack"]),
    );
    fonts.families.insert(
        FontFamily::Name(term_family_name("Hack").into()),
        family(&["Hack", MONO]),
    );
    ctx.set_fonts(fonts.clone());
    *FONT_DEFS.lock().unwrap() = Some(fonts);
}

static FONT_DEFS: std::sync::Mutex<Option<FontDefinitions>> = std::sync::Mutex::new(None);
/// Terminal fonts loaded from disk: name → pass in which it was loaded (`None` = file
/// missing or unusable). A font is usable from the pass after `set_fonts`.
static TERM_FONTS: std::sync::Mutex<Vec<(String, Option<u64>)>> = std::sync::Mutex::new(Vec::new());

pub const DEFAULT_TERM_FONT: &str = "IBM Plex Mono";

/// Monospace fonts commonly installed with the OS: (name, file, collection index).
fn system_fonts() -> Vec<(&'static str, String, u32)> {
    let mut v: Vec<(&'static str, String, u32)> = Vec::new();
    if cfg!(windows) {
        let dir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        for (name, file) in [
            ("Cascadia Mono", "CascadiaMono.ttf"),
            ("Cascadia Code", "CascadiaCode.ttf"),
            ("Consolas", "consola.ttf"),
            ("Courier New", "cour.ttf"),
            ("Lucida Console", "lucon.ttf"),
        ] {
            v.push((name, format!(r"{dir}\Fonts\{file}"), 0));
        }
    } else if cfg!(target_os = "macos") {
        for (name, file, idx) in [
            ("Menlo", "/System/Library/Fonts/Menlo.ttc", 0),
            ("Monaco", "/System/Library/Fonts/Monaco.ttf", 0),
            ("SF Mono", "/System/Library/Fonts/SFNSMono.ttf", 0),
            (
                "Courier New",
                "/System/Library/Fonts/Supplemental/Courier New.ttf",
                0,
            ),
        ] {
            v.push((name, file.into(), idx));
        }
    } else {
        for (name, files) in [
            (
                "DejaVu Sans Mono",
                &[
                    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
                    "/usr/share/fonts/dejavu-sans-mono-fonts/DejaVuSansMono.ttf",
                    "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
                    "/usr/share/fonts/dejavu/DejaVuSansMono.ttf",
                ][..],
            ),
            (
                "Liberation Mono",
                &[
                    "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
                    "/usr/share/fonts/liberation-mono/LiberationMono-Regular.ttf",
                    "/usr/share/fonts/liberation/LiberationMono-Regular.ttf",
                ][..],
            ),
            (
                "Ubuntu Mono",
                &["/usr/share/fonts/truetype/ubuntu/UbuntuMono-R.ttf"][..],
            ),
            (
                "Noto Sans Mono",
                &[
                    "/usr/share/fonts/truetype/noto/NotoSansMono-Regular.ttf",
                    "/usr/share/fonts/noto/NotoSansMono-Regular.ttf",
                ][..],
            ),
            (
                "JetBrains Mono",
                &[
                    "/usr/share/fonts/truetype/jetbrains-mono/JetBrainsMono-Regular.ttf",
                    "/usr/share/fonts/TTF/JetBrainsMono-Regular.ttf",
                ][..],
            ),
            (
                "Fira Code",
                &[
                    "/usr/share/fonts/truetype/firacode/FiraCode-Regular.ttf",
                    "/usr/share/fonts/TTF/FiraCode-Regular.ttf",
                ][..],
            ),
        ] {
            if let Some(f) = files.iter().find(|f| std::path::Path::new(f).exists()) {
                v.push((name, f.to_string(), 0));
            }
        }
    }
    v.retain(|(_, f, _)| std::path::Path::new(f).exists());
    v
}

/// Font names for the terminal font pickers.
pub fn term_fonts() -> Vec<String> {
    let mut v = vec![DEFAULT_TERM_FONT.to_string(), "Hack".to_string()];
    v.extend(system_fonts().into_iter().map(|(n, _, _)| n.to_string()));
    v
}

fn term_family_name(name: &str) -> String {
    format!("term:{name}")
}

/// Font family for a terminal font name or font file path; loads it on first use.
/// Unknown names and unreadable files fall back to IBM Plex Mono.
pub fn term_font(ctx: &egui::Context, name: &str) -> FontFamily {
    let name = name.trim();
    if name.is_empty() || name == DEFAULT_TERM_FONT {
        return FontFamily::Monospace;
    }
    let family = FontFamily::Name(term_family_name(name).into());
    if name == "Hack" {
        return family;
    }
    let pass = ctx.cumulative_pass_nr();
    let mut loaded = TERM_FONTS.lock().unwrap();
    let at = match loaded.iter().find(|(n, _)| n == name) {
        Some((_, at)) => *at,
        None => {
            let at = load_term_font(ctx, name).then_some(pass);
            loaded.push((name.to_string(), at));
            ctx.request_repaint();
            at
        }
    };
    match at {
        Some(p) if pass > p => family,
        _ => FontFamily::Monospace,
    }
}

fn load_term_font(ctx: &egui::Context, name: &str) -> bool {
    let (file, index) = match system_fonts().into_iter().find(|(n, _, _)| *n == name) {
        Some((_, f, i)) => (f, i),
        None => (name.to_string(), 0),
    };
    let lower = file.to_lowercase();
    if ![".ttf", ".otf", ".ttc"].iter().any(|e| lower.ends_with(e)) {
        return false;
    }
    let Ok(bytes) = std::fs::read(&file) else {
        log::warn!("terminal font {file}: unreadable");
        return false;
    };
    // egui panics on fonts it can't parse, so check first.
    let usable = skrifa::FontRef::from_index(&bytes, index).is_ok_and(|f| {
        use skrifa::MetadataProvider;
        f.charmap().map('M').is_some()
    });
    if !usable {
        log::warn!("terminal font {file}: not a usable font");
        return false;
    }
    let mut defs = FONT_DEFS.lock().unwrap();
    let Some(fonts) = defs.as_mut() else {
        return false;
    };
    let key = term_family_name(name);
    let mut data = FontData::from_owned(bytes);
    data.index = index;
    fonts
        .font_data
        .insert(key.clone(), std::sync::Arc::new(data));
    fonts.families.insert(
        FontFamily::Name(key.clone().into()),
        vec![
            key,
            MONO.to_owned(),
            "Hack".to_owned(),
            "NotoEmoji-Regular".to_owned(),
            "emoji-icon-font".to_owned(),
        ],
    );
    ctx.set_fonts(fonts.clone());
    true
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SANS_SEMI.into()))
}

pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SANS_MEDIUM.into()))
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SANS_BOLD.into()))
}

pub fn mono_medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(MONO_MEDIUM.into()))
}

/// Page / dialog heading.
pub fn heading(text: impl Into<String>, size: f32) -> RichText {
    let font = if size >= 18.0 {
        bold(size)
    } else {
        semibold(size)
    };
    RichText::new(text).font(font).color(self::text())
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
    v.extreme_bg_color = fill();
    v.faint_bg_color = hover();
    v.code_bg_color = p.term_bg;
    v.window_stroke = Stroke::new(1.0, line());
    v.window_corner_radius = CornerRadius::same(14);
    v.menu_corner_radius = CornerRadius::same(10);
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
    v.hyperlink_color = p.accent2;
    v.override_text_color = Some(p.text);
    v.text_cursor.stroke = Stroke::new(2.0, accent);

    // Filled controls without outlines, as in the design.
    let r = CornerRadius::same(9);
    v.widgets.noninteractive.bg_fill = p.bg_card;
    v.widgets.noninteractive.weak_bg_fill = p.bg_card;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, line());
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.noninteractive.corner_radius = r;
    v.widgets.inactive.bg_fill = fill_strong();
    v.widgets.inactive.weak_bg_fill = fill();
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, p.text);
    v.widgets.inactive.corner_radius = r;
    v.widgets.hovered.bg_fill = fill_strong();
    v.widgets.hovered.weak_bg_fill = fill_strong();
    v.widgets.hovered.bg_stroke = Stroke::NONE;
    v.widgets.hovered.fg_stroke = Stroke::new(1.5, p.text);
    v.widgets.hovered.corner_radius = r;
    v.widgets.hovered.expansion = 0.0;
    v.widgets.active.bg_fill = soft(accent);
    v.widgets.active.weak_bg_fill = fill_strong();
    v.widgets.active.bg_stroke = Stroke::NONE;
    v.widgets.active.fg_stroke = Stroke::new(1.5, p.text);
    v.widgets.active.corner_radius = r;
    v.widgets.active.expansion = 0.0;
    v.widgets.open = v.widgets.active;
    v.widgets.open.weak_bg_fill = fill_strong();
    // Focused text fields get the accent ring.
    v.selection.bg_fill = soft(accent).gamma_multiply(1.6);
    v.selection.stroke = Stroke::new(1.5, accent);

    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(14.0, 7.0);
    style.spacing.window_margin = Margin::same(22);
    style.spacing.menu_margin = Margin::same(6);
    style.spacing.interact_size.y = 32.0;
    style.spacing.text_edit_width = 260.0;
    style
        .text_styles
        .insert(egui::TextStyle::Body, FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, medium(13.5));
    style
        .text_styles
        .insert(egui::TextStyle::Small, FontId::proportional(12.0));
    style
        .text_styles
        .insert(egui::TextStyle::Monospace, FontId::monospace(13.0));
    style
        .text_styles
        .insert(egui::TextStyle::Heading, bold(24.0));
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

use crate::ui::icons::{self, Icon};

/// Status dot: filled with a soft halo when online, an outline ring when not.
pub fn led(painter: &egui::Painter, center: Pos2, color: Color32, lit: bool) {
    if lit {
        painter.circle_filled(center, 6.0, soft(color));
        painter.circle_filled(center, 4.0, color);
    } else {
        painter.circle_stroke(center, 3.6, Stroke::new(1.5, color.gamma_multiply(0.8)));
    }
}

/// Outline dot for hosts that are offline / unknown.
pub fn off_color() -> Color32 {
    lerp(text_dim(), bg_deep(), 0.3)
}

/// Rounded tag ("test", "backup", "RDP"): soft fill, coloured text.
pub fn chip(ui: &mut egui::Ui, text: &str, color: Color32) -> egui::Response {
    tag(ui, text, Some(color))
}

/// Rounded tag; `None` = neutral grey.
pub fn tag(ui: &mut egui::Ui, text: &str, color: Option<Color32>) -> egui::Response {
    let fg = color.unwrap_or_else(text_dim);
    let bg = color.map(soft).unwrap_or_else(fill);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), medium(12.0), fg);
    let size = Vec2::new(galley.size().x + 18.0, 22.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    ui.painter().rect_filled(rect, CornerRadius::same(11), bg);
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, fg);
    resp
}

/// Uppercase environment pill (PROD, STAGING).
pub fn pill(ui: &mut egui::Ui, text: &str, color: Color32) -> egui::Response {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_uppercase(), bold(10.5), color);
    let size = Vec2::new(galley.size().x + 14.0, 18.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(5), soft(color));
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, color);
    resp
}

/// Brand mark: an accent tile with a ring and a pin.
pub fn logo(ui: &mut egui::Ui, size: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    paint_logo(ui.painter(), rect);
    resp
}

pub fn paint_logo(p: &egui::Painter, rect: Rect) {
    let s = rect.width() / 34.0;
    p.rect_filled(rect, CornerRadius::same((9.0 * s) as u8), accent());
    let c = rect.center();
    p.circle_stroke(c, 8.0 * s, Stroke::new(3.2 * s, on_accent()));
    p.circle_filled(c, 2.6 * s, on_accent());
}

/// Button look.
#[derive(Clone, Copy, PartialEq)]
pub enum Btn {
    /// Grey fill.
    Normal,
    /// Accent fill, for the one primary action.
    Primary,
    /// Smaller grey button (28 px).
    Small,
    /// Smaller accent button.
    SmallPrimary,
    /// Text only, grey until hovered ("Alle öffnen", "+ Host").
    Link,
}

/// Filled button with optional leading icon.
pub fn btn(ui: &mut egui::Ui, kind: Btn, icon: Option<Icon>, label: &str) -> egui::Response {
    let text = label;
    let ink = text_color();
    let (h, font, radius, pad) = match kind {
        Btn::Normal | Btn::Primary => (34.0, medium(13.5), 9, 14.0),
        Btn::Small | Btn::SmallPrimary => (28.0, medium(13.0), 7, 12.0),
        Btn::Link => (26.0, medium(12.5), 6, 8.0),
    };
    let primary = matches!(kind, Btn::Primary | Btn::SmallPrimary);
    let font = if primary { semibold(font.size) } else { font };
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font, Color32::PLACEHOLDER);
    let icon_w = if icon.is_some() {
        if text.is_empty() { 15.0 } else { 21.0 }
    } else {
        0.0
    };
    let w = galley.size().x + icon_w + pad * 2.0;
    let w = if text.is_empty() { h } else { w };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), egui::Sense::click());
    let enabled = ui.is_enabled();
    let hot = resp.hovered() && enabled;
    let (bg, fg) = match kind {
        _ if primary => (
            if hot {
                lerp(accent(), Color32::WHITE, 0.08)
            } else {
                accent()
            },
            on_accent(),
        ),
        Btn::Link => (
            if hot { hover() } else { Color32::TRANSPARENT },
            if hot { ink } else { text_dim() },
        ),
        _ => (if hot { fill_strong() } else { fill() }, ink),
    };
    let alpha = if enabled { 1.0 } else { 0.38 };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(radius), bg.gamma_multiply(alpha));
    let fg = fg.gamma_multiply(alpha);
    let mut x = if text.is_empty() {
        rect.center().x - 7.5
    } else {
        rect.left() + pad
    };
    if let Some(icon) = icon {
        let r = Rect::from_center_size(Pos2::new(x + 7.5, rect.center().y), Vec2::splat(15.0));
        icons::paint(ui.painter(), r, icon, fg);
        x += icon_w;
    }
    if !text.is_empty() {
        ui.painter().galley(
            Pos2::new(x, rect.center().y - galley.size().y / 2.0),
            galley,
            fg,
        );
    }
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

fn text_color() -> Color32 {
    text()
}

/// Solid accent button for the one primary action on a screen.
pub fn primary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    btn(ui, Btn::Primary, None, text)
}

/// Uppercase caption above a group ("DESIGN", "OHNE GRUPPE").
pub fn section_title(ui: &mut egui::Ui, text: &str) {
    ui.label(
        RichText::new(spaced(&text.to_uppercase()))
            .font(semibold(12.0))
            .color(text_dim()),
    );
}

/// Adds a little letter spacing to uppercase captions.
pub fn spaced(s: &str) -> String {
    s.to_string()
}

/// Big page title with an optional count next to it.
pub fn page_title(ui: &mut egui::Ui, title: &str, count: Option<usize>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.label(RichText::new(title).font(bold(32.0)).color(text()));
        if let Some(n) = count {
            ui.add_space(-2.0);
            ui.label(
                RichText::new(n.to_string())
                    .font(medium(15.0))
                    .color(text_dim()),
            );
        }
    });
}

/// Room the right-hand group of a [`page_header`] gets on the current frame.
pub struct Bar {
    /// Not even the right group fits on a row of its own: drop labels, keep icons.
    pub compact: bool,
    slack: f32,
    extra: f32,
}

impl Bar {
    /// Width for a flexible element (search field): `min` plus whatever room is left,
    /// up to `max`.
    pub fn flex(&mut self, min: f32, max: f32) -> f32 {
        let w = (min + self.slack.max(0.0)).clamp(min, max.max(min));
        self.slack -= w - min;
        self.extra += w - min;
        w
    }
}

/// Measured widths of the header groups (last frame), keyed by the header id.
#[derive(Clone, Copy, Default, PartialEq)]
struct HeaderWidths {
    left: f32,
    middle: f32,
    /// Right group with all labels and flexible elements at their minimum.
    right: f32,
    /// Same, in compact mode.
    right_compact: f32,
    /// `right_compact` was measured (not just copied from `right`).
    compact_measured: bool,
}

/// Header row that never overlaps: `left` (title), an optional `middle` (page switch,
/// filter) and right-aligned controls. Wraps into two or three rows when the groups don't
/// fit side by side; flexible elements (see [`Bar::flex`]) shrink first. Pass `|_| {}` for
/// no middle group.
pub fn page_header(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    left: impl FnOnce(&mut egui::Ui),
    middle: impl FnOnce(&mut egui::Ui),
    right: impl FnOnce(&mut egui::Ui, &mut Bar),
) {
    adaptive_row(ui, id_salt, false, left, middle, right);
}

/// Filter row: `left` (segmented filter) and right-aligned options. Unlike
/// [`page_header`] it first tries the compact right group on the same row, then wraps.
pub fn toolbar(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    left: impl FnOnce(&mut egui::Ui),
    right: impl FnOnce(&mut egui::Ui, &mut Bar),
) {
    adaptive_row(ui, id_salt, true, left, |_| {}, right);
}

fn adaptive_row(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    compact_first: bool,
    left: impl FnOnce(&mut egui::Ui),
    middle: impl FnOnce(&mut egui::Ui),
    right: impl FnOnce(&mut egui::Ui, &mut Bar),
) {
    const GAP: f32 = 16.0;
    let id = ui.id().with(("adaptive-header", id_salt));
    let old: Option<HeaderWidths> = ui.ctx().data(|d| d.get_temp(id));
    let w = old.unwrap_or_default();
    let avail = ui.available_width();
    let fits = |parts: &[f32]| {
        let used: Vec<f32> = parts.iter().copied().filter(|p| *p > 0.5).collect();
        used.iter().sum::<f32>() + GAP * (used.len().max(1) - 1) as f32 <= avail
    };
    let compact = w.right > avail + 0.5
        || (compact_first
            && !fits(&[w.left, w.middle, w.right])
            && (!w.compact_measured || fits(&[w.left, w.middle, w.right_compact])));
    let right_w = if compact { w.right_compact } else { w.right };
    // Rows: which group starts a new row.
    let (middle_row, right_row) = if fits(&[w.left, w.middle, right_w]) {
        (0, 0)
    } else if fits(&[w.left, right_w]) {
        (1, 0)
    } else if fits(&[w.left, w.middle]) {
        (0, 1)
    } else if fits(&[w.middle, right_w]) {
        (1, 1)
    } else {
        (1, 2)
    };
    let mut new = w;
    let right_group = move |ui: &mut egui::Ui, gap: f32, new: &mut HeaderWidths| {
        let mut bar = Bar {
            compact,
            slack: ui.available_width() - gap - right_w,
            extra: 0.0,
        };
        let r = ui
            .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                right(ui, &mut bar)
            })
            .response;
        let width = (r.rect.width() - bar.extra).max(0.0);
        if compact {
            new.right_compact = width;
            new.compact_measured = true;
        } else {
            new.right = width;
            if new.right_compact <= 0.0 {
                new.right_compact = width;
            }
        }
    };
    let mut right_cell = Some(right_group);
    let mut middle_cell = Some(middle);
    let mut left_cell = Some(left);
    for row in 0..3 {
        if row > 0 && middle_row < row && right_row < row {
            break;
        }
        if row > 0 {
            ui.add_space(12.0);
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let start = ui.cursor().left();
            if let Some(l) = left_cell.take() {
                new.left = ui.horizontal(l).response.rect.width();
            }
            if middle_row == row
                && let Some(m) = middle_cell.take()
            {
                if row == 0 {
                    ui.add_space(GAP - 10.0);
                }
                new.middle = ui.horizontal(m).response.rect.width();
            }
            if right_row == row
                && let Some(r) = right_cell.take()
            {
                let gap = if ui.cursor().left() - start > 0.5 {
                    GAP
                } else {
                    0.0
                };
                r(ui, gap, &mut new);
            }
        });
    }
    if old != Some(new) {
        ui.ctx().data_mut(|d| d.insert_temp(id, new));
        let moved = |a: f32, b: f32| (a - b).abs() > 0.5;
        if old.is_none()
            || moved(w.left, new.left)
            || moved(w.middle, new.middle)
            || moved(w.right, new.right)
            || moved(w.right_compact, new.right_compact)
        {
            ui.ctx().request_discard("page header layout");
        }
    }
}

/// Text cut to `max_w` with "…" (single line).
pub fn truncated(
    painter: &egui::Painter,
    text: impl Into<String>,
    font: FontId,
    color: Color32,
    max_w: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.into(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_w.max(1.0));
    painter.layout_job(job)
}

/// Covers `rect` with the given background layers, fading in over `fade` px on the left,
/// so row text never shows through hover buttons.
pub fn fade_cover(painter: &egui::Painter, rect: Rect, fade: f32, layers: &[Color32]) {
    for &c in layers {
        let mut mesh = egui::Mesh::default();
        let l = rect.left() - fade;
        let (t, b) = (rect.top(), rect.bottom());
        let pts = [
            (Pos2::new(l, t), Color32::TRANSPARENT),
            (Pos2::new(rect.left(), t), c),
            (Pos2::new(rect.right(), t), c),
            (Pos2::new(l, b), Color32::TRANSPARENT),
            (Pos2::new(rect.left(), b), c),
            (Pos2::new(rect.right(), b), c),
        ];
        for (p, col) in pts {
            mesh.colored_vertex(p, col);
        }
        mesh.add_triangle(0, 1, 4);
        mesh.add_triangle(0, 4, 3);
        mesh.add_triangle(1, 2, 5);
        mesh.add_triangle(1, 5, 4);
        painter.add(egui::Shape::mesh(mesh));
    }
}

/// Width [`btn`] will take for these arguments.
pub fn btn_width(ui: &egui::Ui, kind: Btn, icon: Option<Icon>, label: &str) -> f32 {
    let (h, font, pad) = match kind {
        Btn::Normal | Btn::Primary => (34.0, medium(13.5), 14.0),
        Btn::Small | Btn::SmallPrimary => (28.0, medium(13.0), 12.0),
        Btn::Link => (26.0, medium(12.5), 8.0),
    };
    if label.is_empty() {
        return h;
    }
    let font = if matches!(kind, Btn::Primary | Btn::SmallPrimary) {
        semibold(font.size)
    } else {
        font
    };
    let g = ui
        .painter()
        .layout_no_wrap(label.to_string(), font, Color32::PLACEHOLDER);
    g.size().x + if icon.is_some() { 21.0 } else { 0.0 } + pad * 2.0
}

/// Rounded card for lists and grouped settings.
pub fn card_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(surface())
        .stroke(Stroke::new(1.0, card_edge()))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::same(16))
}

/// Card without padding; rows bring their own.
pub fn list_card() -> egui::Frame {
    card_frame().inner_margin(Margin::ZERO)
}

/// Barely visible card edge (stronger on light themes).
pub fn card_edge() -> Color32 {
    if palette().dark {
        Color32::from_white_alpha(13)
    } else {
        Color32::from_black_alpha(18)
    }
}

/// Square icon button (32 px, rounded) with tooltip.
pub fn icon_btn(ui: &mut egui::Ui, icon: Icon, tooltip: &str) -> egui::Response {
    icon_toggle(ui, icon, tooltip, false)
}

/// Icon button that stays highlighted while `on`.
pub fn icon_toggle(ui: &mut egui::Ui, icon: Icon, tooltip: &str, on: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(32.0), egui::Sense::click());
    let enabled = ui.is_enabled();
    let hot = resp.hovered() && enabled;
    if on {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(9), fill_strong());
    } else if hot {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(9), fill());
    }
    let color = if !enabled {
        text_dim().gamma_multiply(0.4)
    } else if hot || on {
        text()
    } else {
        text_dim()
    };
    icons::paint(
        ui.painter(),
        Rect::from_center_size(rect.center(), Vec2::splat(17.0)),
        icon,
        color,
    );
    resp.on_hover_text(tooltip)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Icon button for a text glyph (for symbols without a line icon).
pub fn icon_button(ui: &mut egui::Ui, glyph: &str, tooltip: &str) -> egui::Response {
    if let Some(icon) = glyph_icon(glyph) {
        return icon_btn(ui, icon, tooltip);
    }
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(32.0), egui::Sense::click());
    let enabled = ui.is_enabled();
    if resp.hovered() && enabled {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(9), fill());
    }
    let color = if !enabled {
        text_dim().gamma_multiply(0.4)
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

/// Line icon for the glyphs the old toolbar used.
fn glyph_icon(glyph: &str) -> Option<Icon> {
    Some(match glyph {
        "⚡" => Icon::Bolt,
        "🔍" => Icon::Search,
        "⟳" | "↻" => Icon::Refresh,
        "🗀" | "📁" => Icon::Folder,
        "⚙" => Icon::Gear,
        "🔑" => Icon::Key,
        "＋" | "+" => Icon::Plus,
        "×" | "✕" => Icon::Close,
        "⬆" | "↑" => Icon::Up,
        "⤢" => Icon::Expand,
        "⌨" => Icon::Keyboard,
        "↗" => Icon::External,
        "📝" => Icon::File,
        "⊞" => Icon::Windows,
        "📤" => Icon::Upload,
        "📥" => Icon::Download,
        "🗑" => Icon::Trash,
        "📋" => Icon::Copy,
        "↩" => Icon::Wrap,
        "⏸" => Icon::Pause,
        "▶" => Icon::Play,
        "⏹" => Icon::Stop,
        "🖥" => Icon::Monitor,
        _ => return None,
    })
}

/// Segmented control (grey track, raised selected segment); returns the clicked index.
pub fn segmented(ui: &mut egui::Ui, items: &[&str], selected: usize) -> Option<usize> {
    let font = medium(13.0);
    let galleys: Vec<_> = items
        .iter()
        .map(|l| {
            ui.painter()
                .layout_no_wrap(l.to_string(), font.clone(), Color32::PLACEHOLDER)
        })
        .collect();
    // Segments give up padding before the control runs out of room.
    let n = items.len().max(1) as f32;
    let text_w: f32 = galleys.iter().map(|g| g.size().x).sum();
    let chrome = 2.0 * (n - 1.0) + 4.0;
    let room = ui.available_width();
    let pad = ((room - text_w - chrome) / n).clamp(12.0, 28.0);
    let widths: Vec<f32> = galleys.iter().map(|g| g.size().x + pad).collect();
    let total = widths.iter().sum::<f32>() + chrome;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(total, 32.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(9), fill());
    let mut x = rect.left() + 2.0;
    let mut clicked = None;
    for (i, (g, w)) in galleys.into_iter().zip(widths).enumerate() {
        let r = Rect::from_min_size(Pos2::new(x, rect.top() + 2.0), Vec2::new(w, 28.0));
        let resp = ui.interact(r, ui.id().with(("seg", items[i], i)), egui::Sense::click());
        let on = i == selected;
        if on {
            ui.painter().rect_filled(
                r.translate(Vec2::new(0.0, 1.0)),
                CornerRadius::same(7),
                Color32::from_black_alpha(if palette().dark { 70 } else { 18 }),
            );
            ui.painter().rect_filled(r, CornerRadius::same(7), seg_on());
        } else if resp.hovered() {
            ui.painter().rect_filled(r, CornerRadius::same(7), hover());
        }
        let color = if on || resp.hovered() {
            text()
        } else {
            text_dim()
        };
        ui.painter().galley(r.center() - g.size() / 2.0, g, color);
        if resp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
        {
            clicked = Some(i);
        }
        x += w + 2.0;
    }
    clicked
}

/// Search field with a magnifier inside.
pub fn search_field(
    ui: &mut egui::Ui,
    text: &mut String,
    hint: &str,
    width: f32,
    id: egui::Id,
) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 34.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, CornerRadius::same(9), fill());
    icons::paint(
        ui.painter(),
        Rect::from_center_size(
            Pos2::new(rect.left() + 18.0, rect.center().y),
            Vec2::splat(15.0),
        ),
        Icon::Search,
        text_dim(),
    );
    let inner = Rect::from_min_max(
        Pos2::new(rect.left() + 32.0, rect.top()),
        Pos2::new(rect.right() - 8.0, rect.bottom()),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    child.add(
        egui::TextEdit::singleline(text)
            .id(id)
            .hint_text(RichText::new(hint).color(text_dim()))
            .frame(egui::Frame::NONE)
            .font(FontId::proportional(13.5))
            .desired_width(inner.width()),
    )
}

/// Key cap for shortcut lists.
pub fn kbd(ui: &mut egui::Ui, key: &str) -> egui::Response {
    let galley = ui
        .painter()
        .layout_no_wrap(key.to_string(), mono_medium(12.5), text());
    let w = (galley.size().x + 16.0).max(26.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 26.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(
        rect.translate(Vec2::new(0.0, 1.0)),
        CornerRadius::same(7),
        Color32::from_black_alpha(if palette().dark { 120 } else { 30 }),
    );
    p.rect(
        rect,
        CornerRadius::same(7),
        lerp(surface(), text(), 0.1),
        Stroke::new(1.0, line()),
        egui::StrokeKind::Inside,
    );
    p.galley(rect.center() - galley.size() / 2.0, galley, text());
    resp
}

/// Accent-tinted label for mouse gestures in shortcut lists ("Mausrad").
pub fn gesture(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), semibold(12.5), accent());
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::new(galley.size().x + 20.0, 26.0),
        egui::Sense::hover(),
    );
    ui.painter()
        .rect_filled(rect, CornerRadius::same(7), accent_soft());
    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, accent());
    resp
}

/// Thin full-width rule.
pub fn hairline(ui: &mut egui::Ui) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::ZERO, line());
}

/// Paints the background colour over the four corners of `rect`, so content drawn
/// with square corners looks like a rounded card.
pub fn round_corners(p: &egui::Painter, rect: Rect, radius: f32, bg: Color32) {
    let n = 8;
    for (corner, center, start) in [
        (
            rect.left_top(),
            rect.left_top() + Vec2::splat(radius),
            180.0f32,
        ),
        (
            rect.right_top(),
            rect.right_top() + Vec2::new(-radius, radius),
            270.0,
        ),
        (
            rect.right_bottom(),
            rect.right_bottom() - Vec2::splat(radius),
            0.0,
        ),
        (
            rect.left_bottom(),
            rect.left_bottom() + Vec2::new(radius, -radius),
            90.0,
        ),
    ] {
        let mut pts = vec![corner];
        for i in 0..=n {
            let a = (start + 90.0 * i as f32 / n as f32).to_radians();
            pts.push(center + Vec2::new(a.cos(), a.sin()) * radius);
        }
        // Each wedge between the corner and the arc is convex enough to fan out.
        for w in pts[1..].windows(2) {
            p.add(egui::Shape::convex_polygon(
                vec![corner, w[0], w[1]],
                bg,
                Stroke::NONE,
            ));
        }
    }
}

/// Rounded list card: rows fill it edge to edge, the corners stay round even when a row
/// paints a hover background. `bg` is the colour behind the card.
pub fn card<R>(ui: &mut egui::Ui, bg: Color32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let out = list_card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 0.0;
        add(ui)
    });
    let r = out.response.rect;
    round_corners(ui.painter(), r, 14.0, bg);
    ui.painter().rect_stroke(
        r,
        CornerRadius::same(14),
        Stroke::new(1.0, card_edge()),
        egui::StrokeKind::Inside,
    );
    out.inner
}

/// Background of a form group inside a sheet (slightly lighter than the sheet).
pub fn group_fill() -> Color32 {
    let p = palette();
    if p.dark {
        lerp(p.bg_card, p.text, 0.065)
    } else {
        lerp(p.bg_card, Color32::BLACK, 0.035)
    }
}

/// Rows of a card or form group, separated by inset hairlines.
pub struct Rows {
    n: usize,
    /// Left inset of the hairlines and the row padding.
    pad: f32,
    /// Label column width for form rows.
    label_w: f32,
}

impl Rows {
    fn divider(&mut self, ui: &mut egui::Ui) {
        if self.n > 0 {
            // End at the width the rows really use (the frame is drawn around that).
            let r = ui.min_rect();
            let y = ui.cursor().top();
            ui.painter().line_segment(
                [Pos2::new(r.left() + self.pad, y), Pos2::new(r.right(), y)],
                Stroke::new(1.0, line()),
            );
        }
        self.n += 1;
    }

    /// Settings row: label (and optional help text) left, control right.
    pub fn setting(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        help: Option<&str>,
        add: impl FnOnce(&mut egui::Ui),
    ) {
        self.divider(ui);
        let w = ui.available_width();
        let pad = self.pad;
        ui.allocate_ui_with_layout(
            Vec2::new(w, 52.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_min_size(Vec2::new(w, 52.0));
                ui.add_space(pad);
                // Centre the label block vertically in the 52 px row.
                let text_w = (w * 0.55).max(200.0);
                let mut h = ui
                    .painter()
                    .layout(
                        label.to_string(),
                        FontId::proportional(14.5),
                        text(),
                        text_w,
                    )
                    .size()
                    .y;
                if let Some(help) = help {
                    h += 2.0
                        + ui.painter()
                            .layout(help.to_string(), FontId::proportional(12.5), text(), text_w)
                            .size()
                            .y;
                }
                let top = ((52.0 - h) / 2.0).max(8.0);
                ui.vertical(|ui| {
                    ui.set_max_width(text_w);
                    ui.add_space(top);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.add(egui::Label::new(RichText::new(label).size(14.5).color(text())).wrap());
                    if let Some(h) = help {
                        ui.add(
                            egui::Label::new(RichText::new(h).size(12.5).color(text_dim())).wrap(),
                        );
                    }
                    ui.add_space(top);
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(pad);
                    add(ui);
                });
            },
        );
    }

    /// Free-form row with the usual padding.
    pub fn full(&mut self, ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
        self.divider(ui);
        let pad = self.pad as i8;
        egui::Frame::NONE
            .inner_margin(Margin::symmetric(pad, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                add(ui)
            });
    }

    /// Form row with a switch on the right ("In Favoriten anzeigen").
    pub fn switch(&mut self, ui: &mut egui::Ui, label: &str, on: &mut bool) -> egui::Response {
        self.divider(ui);
        let (row, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 44.0), egui::Sense::hover());
        ui.painter().text(
            Pos2::new(row.left() + self.pad, row.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            FontId::proportional(14.0),
            text(),
        );
        let r = Rect::from_center_size(
            Pos2::new(row.right() - self.pad - 19.0, row.center().y),
            Vec2::new(46.0, 28.0),
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(r)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        toggle(&mut child, on)
    }

    /// Form row: fixed-width label, frameless control filling the rest (44 px high).
    pub fn field(&mut self, ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
        self.divider(ui);
        let (row, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 44.0), egui::Sense::hover());
        ui.painter().text(
            Pos2::new(row.left() + self.pad, row.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            FontId::proportional(14.0),
            text(),
        );
        // The control gets exactly the rest of the row, so it can never widen the form.
        let rest = Rect::from_min_max(
            Pos2::new(row.left() + self.pad + self.label_w + 8.0, row.top()),
            Pos2::new(row.right() - self.pad + 4.0, row.bottom()),
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rest)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        frameless(&mut child);
        add(&mut child);
    }
}

/// Text fields and combo boxes without a frame (inside form rows).
pub fn frameless(ui: &mut egui::Ui) {
    let v = &mut ui.style_mut().visuals;
    v.extreme_bg_color = Color32::TRANSPARENT;
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    v.widgets.hovered.bg_stroke = Stroke::NONE;
    v.widgets.hovered.weak_bg_fill = hover();
    v.widgets.active.bg_stroke = Stroke::NONE;
    v.selection.stroke = Stroke::NONE;
}

/// Settings card: rounded surface with rows.
pub fn settings_card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui, &mut Rows) -> R) -> R {
    card(ui, bg_deep(), |ui| {
        let mut rows = Rows {
            n: 0,
            pad: 18.0,
            label_w: 120.0,
        };
        add(ui, &mut rows)
    })
}

/// Form group inside a sheet; `label_w` is the label column.
pub fn form<R>(
    ui: &mut egui::Ui,
    label_w: f32,
    add: impl FnOnce(&mut egui::Ui, &mut Rows) -> R,
) -> R {
    let bg = ui.style().visuals.window_fill;
    let out = egui::Frame::NONE
        .fill(group_fill())
        .corner_radius(CornerRadius::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            let mut rows = Rows {
                n: 0,
                pad: 14.0,
                label_w,
            };
            add(ui, &mut rows)
        });
    round_corners(ui.painter(), out.response.rect, 12.0, bg);
    out.inner
}

/// iOS-style switch.
pub fn toggle(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(46.0, 28.0), egui::Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    let off = if palette().dark {
        lerp(surface(), text(), 0.2)
    } else {
        lerp(surface(), Color32::BLACK, 0.12)
    };
    let track = lerp(off, accent(), t);
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(14), track);
    let x = egui::lerp(rect.left() + 14.0..=rect.right() - 14.0, t);
    let c = Pos2::new(x, rect.center().y);
    p.circle_filled(c + Vec2::new(0.0, 1.0), 12.0, Color32::from_black_alpha(50));
    p.circle_filled(c, 12.0, Color32::WHITE);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Round × button in a sheet header.
pub fn close_button(ui: &mut egui::Ui) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(30.0), egui::Sense::click());
    ui.painter().circle_filled(
        rect.center(),
        15.0,
        if resp.hovered() {
            fill_strong()
        } else {
            fill()
        },
    );
    icons::paint_weighted(
        ui.painter(),
        Rect::from_center_size(rect.center(), Vec2::splat(11.0)),
        Icon::Close,
        text(),
        3.0,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Sheet (modal dialog): title with a round close button, scrollable body, footer row.
/// Returns the footer's result and whether the sheet was dismissed (× or Esc).
pub fn sheet<B, F>(
    ctx: &egui::Context,
    id: egui::Id,
    width: f32,
    title: &str,
    body: impl FnOnce(&mut egui::Ui) -> B,
    footer: impl FnOnce(&mut egui::Ui, B) -> F,
) -> (F, bool) {
    let mut dismissed = false;
    let frame = egui::Frame::NONE
        .fill(surface())
        .corner_radius(CornerRadius::same(18))
        .stroke(Stroke::new(1.0, line()))
        .shadow(Shadow {
            offset: [0, 24],
            blur: 70,
            spread: 0,
            color: Color32::from_black_alpha(if palette().dark { 115 } else { 40 }),
        });
    let out = egui::Modal::new(id)
        .frame(frame)
        .backdrop_color(Color32::from_black_alpha(if palette().dark {
            150
        } else {
            70
        }))
        .show(ctx, |ui| {
            // Small windows: the sheet shrinks with them (the body scrolls).
            ui.set_width(width.min(ctx.content_rect().width() - 48.0).max(260.0));
            ui.spacing_mut().item_spacing.y = 0.0;
            egui::Frame::NONE
                .inner_margin(Margin {
                    left: 20,
                    right: 18,
                    top: 18,
                    bottom: 14,
                })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(title).font(bold(20.0)).color(text()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if close_button(ui).clicked() {
                                dismissed = true;
                            }
                        });
                    });
                });
            let screen_h = ctx.content_rect().height();
            let max_h = (screen_h * 0.72).min(screen_h - 170.0).max(160.0);
            let b = egui::ScrollArea::vertical()
                .id_salt(id.with("body"))
                .max_height(max_h)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    egui::Frame::NONE
                        .inner_margin(Margin {
                            left: 20,
                            right: 20,
                            top: 0,
                            bottom: 18,
                        })
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 8.0;
                            body(ui)
                        })
                        .inner
                })
                .inner;
            let r = ui.max_rect();
            let y = ui.cursor().top();
            ui.painter().line_segment(
                [Pos2::new(r.left(), y), Pos2::new(r.right(), y)],
                Stroke::new(1.0, line()),
            );
            egui::Frame::NONE
                .inner_margin(Margin::symmetric(20, 14))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            footer(ui, b)
                        })
                        .inner
                    })
                    .inner
                })
                .inner
        });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        dismissed = true;
    }
    (out.inner, dismissed)
}

/// Caption above a form group inside a sheet.
pub fn form_caption(ui: &mut egui::Ui, text: &str) {
    ui.add_space(14.0);
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        section_title(ui, text);
    });
    ui.add_space(2.0);
}

/// Frame of small dialogs (same look as a sheet, with padding).
pub fn dialog_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(surface())
        .corner_radius(CornerRadius::same(18))
        .stroke(Stroke::new(1.0, line()))
        .inner_margin(Margin::same(22))
        .shadow(Shadow {
            offset: [0, 24],
            blur: 70,
            spread: 0,
            color: Color32::from_black_alpha(if palette().dark { 115 } else { 40 }),
        })
}

/// Dimmed background behind dialogs.
pub fn backdrop() -> Color32 {
    Color32::from_black_alpha(if palette().dark { 150 } else { 70 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(c: Color32) -> f32 {
        let f = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f32 {
        let (la, lb) = (luminance(a), luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    #[test]
    fn theme_ids_are_unique() {
        for (i, a) in THEMES.iter().enumerate() {
            assert!(
                THEMES[i + 1..]
                    .iter()
                    .all(|b| b.id != a.id && b.name != a.name),
                "{}",
                a.id
            );
        }
    }

    #[test]
    fn themes_are_readable() {
        let mut bad = Vec::new();
        for t in THEMES {
            let checks = [
                ("text", contrast(t.text, t.bg_deep), 7.0),
                ("text on card", contrast(t.text, t.bg_card), 7.0),
                ("dim", contrast(t.text_dim, t.bg_card), 3.5),
                ("terminal", contrast(t.term_fg, t.term_bg), 4.5),
                ("accent label", contrast(t.on_accent, t.accent), 2.5),
            ];
            for (what, got, min) in checks {
                if got < min {
                    bad.push(format!("{} {what}: {got:.2} < {min}", t.id));
                }
            }
            if t.dark != (luminance(t.bg_deep) < 0.2) {
                bad.push(format!("{} dark flag", t.id));
            }
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }
}
