//! Embedding of web interface tabs: the `aurora-web` helper creates its web view as a
//! CHILD window of Aurora's main window (X11 window / Windows HWND) and Aurora tells it
//! the pixel rectangle of the tab content every frame it changes.
//!
//! Where it applies:
//! - Linux on X11 (also XWayland): the helper builds a child of Aurora's X window.
//! - Windows: the helper builds a `WS_CHILD` WebView2 of Aurora's HWND (cannot be tested
//!   on Linux; the helper falls back to its own window if creation fails).
//! - Wayland and macOS: no cross-process child windows exist (Wayland has no foreign
//!   surfaces, an `NSView` pointer is only valid inside its process) -> own window.
//!
//! A native child window always sits ABOVE everything egui paints. So the page is hidden
//! whenever something has to be drawn over it: another tab/pane is shown, a menu, popup,
//! dialog or the command palette overlaps it, the vault is locked, the window is
//! minimized, or the tab lives in a popped-out window.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use egui::{Order, Rect};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use crate::app::{App, TabKind};
use crate::backend::web::proto::{EmbedParent, ParentKind, PixelRect};
use crate::theme;

/// Aurora's native main window (refreshed every frame; `None` on Wayland/macOS).
static PARENT: Mutex<Option<EmbedParent>> = Mutex::new(None);
/// Settings: show web interfaces in their own window.
static OWN_WINDOW: AtomicBool = AtomicBool::new(false);
/// One-shot override for the next opened web tab (per-tab "own window / embed" button).
static NEXT_OPEN: Mutex<Option<bool>> = Mutex::new(None);

thread_local! {
    /// Rectangles the web tabs occupied in the main window during this frame.
    static RECTS: RefCell<HashMap<u64, Rect>> = RefCell::new(HashMap::new());
}

/// Called first thing in every frame: remembers the native window and the setting.
pub fn begin_frame(own_window: bool, frame: &eframe::Frame) {
    OWN_WINDOW.store(own_window, Ordering::Relaxed);
    let parent = frame
        .window_handle()
        .ok()
        .and_then(|h| parent_from_raw(h.as_raw()));
    if let Ok(mut p) = PARENT.lock()
        && *p != parent
    {
        *p = parent;
    }
}

#[allow(clippy::useless_conversion)] // c_ulong is u32 on Windows targets
fn parent_from_raw(raw: RawWindowHandle) -> Option<EmbedParent> {
    match raw {
        RawWindowHandle::Xlib(x) if x.window != 0 => Some(EmbedParent {
            kind: ParentKind::X11,
            handle: u64::from(x.window),
        }),
        RawWindowHandle::Xcb(x) => Some(EmbedParent {
            kind: ParentKind::X11,
            handle: u64::from(x.window.get()),
        }),
        RawWindowHandle::Win32(w) => Some(EmbedParent {
            kind: ParentKind::Win32,
            handle: w.hwnd.get() as u64,
        }),
        _ => None,
    }
}

/// This system can show web pages inside the Aurora window.
pub fn can_embed() -> bool {
    PARENT.lock().map(|p| p.is_some()).unwrap_or(false)
}

/// The parent window for a web tab that is about to open (`None` = own window).
/// Consumes the one-shot override of the per-tab button.
pub fn parent_for_open() -> Option<EmbedParent> {
    let forced = NEXT_OPEN.lock().ok().and_then(|mut n| n.take());
    let embed = forced.unwrap_or(!OWN_WINDOW.load(Ordering::Relaxed));
    if !embed {
        return None;
    }
    PARENT.lock().ok().and_then(|p| *p)
}

/// The next web tab opens embedded (`true`) or in its own window (`false`), whatever the
/// setting says.
pub fn force_next_open(embed: bool) {
    if let Ok(mut n) = NEXT_OPEN.lock() {
        *n = Some(embed);
    }
}

/// A web tab shows its page in `rect` (clipped to what is visible) in this frame. Ignored
/// outside the main window: popped-out windows have no native handle to embed into.
pub fn register(ui: &egui::Ui, tab_id: u64, rect: Rect) {
    if ui.ctx().viewport_id() != egui::ViewportId::ROOT {
        return;
    }
    let r = rect.intersect(ui.clip_rect());
    if r.is_positive() {
        RECTS.with(|m| {
            m.borrow_mut().insert(tab_id, r);
        });
    }
}

/// Rectangles of everything drawn above the central area (menus, popups, dialogs,
/// palette). Tooltips and toasts are left out: they are short-lived and small.
fn overlay_rects(ctx: &egui::Context) -> Vec<Rect> {
    let toasts = egui::Id::new("toasts");
    ctx.memory(|m| {
        m.areas()
            .visible_layer_ids()
            .into_iter()
            .filter(|l| matches!(l.order, Order::Middle | Order::Foreground) && l.id != toasts)
            .filter_map(|l| m.area_rect(l.id))
            .filter(|r| r.is_positive())
            .collect()
    })
}

/// Where the page goes: `Some` = show at this pixel rectangle, `None` = hide.
/// `rect` and `blockers` are in egui points, `screen` is the window's client area.
pub fn view_rect(
    rect: Rect,
    ppp: f32,
    screen: Rect,
    blockers: &[Rect],
    covered: &mut bool,
) -> Option<PixelRect> {
    let r = rect.intersect(screen);
    if !r.is_positive() {
        return None;
    }
    if blockers.iter().any(|b| {
        let i = b.intersect(r);
        i.width() > 0.5 && i.height() > 0.5
    }) {
        *covered = true;
        return None;
    }
    let x = (r.min.x * ppp).round();
    let y = (r.min.y * ppp).round();
    let w = (r.max.x * ppp).round() - x;
    let h = (r.max.y * ppp).round() - y;
    if w < 4.0 || h < 4.0 {
        return None;
    }
    Some(PixelRect {
        x: x as i32,
        y: y as i32,
        w: w as u32,
        h: h as u32,
    })
}

/// Called after the whole UI of a frame: positions every embedded page.
pub fn end_frame(app: &mut App, ctx: &egui::Context) {
    let rects = RECTS.with(|m| std::mem::take(&mut *m.borrow_mut()));
    let locked = app.vault.is_none();
    let (minimized, screen) =
        ctx.input(|i| (i.viewport().minimized.unwrap_or(false), i.viewport_rect()));
    let ppp = ctx.pixels_per_point();
    let blockers = overlay_rects(ctx);
    let pressed = ctx.input(|i| i.pointer.any_pressed());
    let active = app.active;
    let mut covered = false;
    for tab in &mut app.tabs {
        let TabKind::Web(w) = &mut tab.kind else {
            continue;
        };
        if !w.embedded {
            continue;
        }
        let view = match rects.get(&tab.id) {
            Some(r) if !locked && !minimized => view_rect(*r, ppp, screen, &blockers, &mut covered),
            _ => None,
        };
        w.apply_view(view, active == Some(tab.id), pressed);
    }
    if covered {
        // The overlay going away is only noticed in a later frame.
        ctx.request_repaint_after(std::time::Duration::from_millis(120));
    }
}

/// Settings card "Weboberflächen".
pub fn settings_card(app: &mut crate::app::App, ui: &mut egui::Ui, changed: &mut bool) {
    ui.add_space(18.0);
    ui.horizontal(|ui| {
        ui.add_space(18.0);
        theme::section_title(ui, t!("Weboberflächen"));
    });
    ui.add_space(2.0);
    let can = can_embed();
    let s = &mut app.lib.settings;
    theme::settings_card(ui, |ui, rows| {
        rows.setting(
            ui,
            t!("Anzeige"),
            Some(if can {
                t!("eingebettet im Tab oder in einem eigenen Fenster")
            } else {
                t!("Einbetten ist auf diesem System nicht möglich (Wayland/macOS): eigenes Fenster")
            }),
            |ui| {
                let cur = usize::from(s.web_own_window || !can);
                if let Some(i) = theme::segmented(
                    ui,
                    &[t!("Im Tab (eingebettet)"), t!("Eigenes Fenster")],
                    cur,
                ) {
                    s.web_own_window = i == 1;
                    *changed = true;
                }
            },
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2};

    fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(pos2(x, y), vec2(w, h))
    }

    #[test]
    fn rect_is_scaled_to_pixels() {
        let mut c = false;
        let v = view_rect(
            r(100.0, 50.5, 400.0, 300.0),
            1.5,
            r(0.0, 0.0, 1000.0, 800.0),
            &[],
            &mut c,
        )
        .unwrap();
        assert_eq!((v.x, v.y, v.w, v.h), (150, 76, 600, 450));
        assert!(!c);
    }

    #[test]
    fn rect_is_clipped_to_the_window() {
        let mut c = false;
        let v = view_rect(
            r(900.0, 700.0, 400.0, 300.0),
            1.0,
            r(0.0, 0.0, 1000.0, 800.0),
            &[],
            &mut c,
        )
        .unwrap();
        assert_eq!((v.x, v.y, v.w, v.h), (900, 700, 100, 100));
        // Completely outside / too small: hidden.
        assert!(
            view_rect(
                r(1200.0, 0.0, 10.0, 10.0),
                1.0,
                r(0.0, 0.0, 1000.0, 800.0),
                &[],
                &mut c
            )
            .is_none()
        );
        assert!(
            view_rect(
                r(0.0, 0.0, 3.0, 300.0),
                1.0,
                r(0.0, 0.0, 1000.0, 800.0),
                &[],
                &mut c
            )
            .is_none()
        );
    }

    #[test]
    fn overlays_hide_the_page_only_when_they_overlap() {
        let screen = r(0.0, 0.0, 1000.0, 800.0);
        let page = r(100.0, 100.0, 500.0, 500.0);
        let mut c = false;
        // A menu next to the page: no problem.
        assert!(view_rect(page, 1.0, screen, &[r(0.0, 0.0, 90.0, 800.0)], &mut c).is_some());
        assert!(!c);
        // A menu that only touches the edge: still fine.
        assert!(view_rect(page, 1.0, screen, &[r(0.0, 0.0, 100.0, 800.0)], &mut c).is_some());
        // A dialog over the page hides it.
        assert!(view_rect(page, 1.0, screen, &[r(300.0, 300.0, 100.0, 100.0)], &mut c).is_none());
        assert!(c);
    }

    #[test]
    fn handles_map_to_parents() {
        use raw_window_handle::{WaylandWindowHandle, XlibWindowHandle};
        let x = RawWindowHandle::Xlib(XlibWindowHandle::new(0x40_0005));
        assert_eq!(
            parent_from_raw(x),
            Some(EmbedParent {
                kind: ParentKind::X11,
                handle: 0x40_0005
            })
        );
        let w = RawWindowHandle::Wayland(WaylandWindowHandle::new(std::ptr::NonNull::<
            std::ffi::c_void,
        >::dangling()));
        assert_eq!(parent_from_raw(w), None);
    }
}
