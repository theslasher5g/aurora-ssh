//! Embedding of the web view into Aurora's window (child window of a native parent).
//!
//! Supported where a window handle is valid across processes:
//! - **X11** (Linux, also XWayland): the helper builds its normal GTK window (never shown
//!   to the window manager), reparents its X window into Aurora's X window
//!   (`XReparentWindow`) and moves/resizes it there with `XMoveResizeWindow`. wry's own
//!   X11 `build_as_child` is not used: it leaves the WebKit widget at 1x1.
//! - **Windows**: wry's `build_as_child` (a `WS_CHILD` WebView2 of Aurora's HWND).
//!
//! Everything platform specific lives here behind `cfg`, so a problem in one backend
//! cannot affect the others; where nothing applies [`parent`] returns `None` and the
//! helper opens its own window.

use tao::event_loop::EventLoopWindowTarget;
use wry::raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, WindowHandle};
use wry::{WebView, WebViewBuilder};

use crate::proto::{EmbedParent, ParentKind, PixelRect};

/// The foreign window Aurora handed over.
pub struct Parent {
    raw: RawWindowHandle,
    /// X11 window id / HWND.
    #[allow(dead_code)]
    id: u64,
}

impl HasWindowHandle for Parent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        // SAFETY: the handle is only used while the helper runs; Aurora destroys the
        // parent window after the helper's pipe closed, and an invalid id makes the
        // window system report an error, not undefined behaviour.
        Ok(unsafe { WindowHandle::borrow_raw(self.raw) })
    }
}

/// The parent window for `p` when this platform can embed into it.
pub fn parent(p: &EmbedParent) -> Option<Parent> {
    if p.handle == 0 {
        return None;
    }
    match p.kind {
        #[cfg(target_os = "linux")]
        ParentKind::X11 => {
            // Wayland has no foreign child windows (and DISPLAY is unset without X).
            std::env::var_os("DISPLAY")?;
            let raw = RawWindowHandle::Xlib(wry::raw_window_handle::XlibWindowHandle::new(
                p.handle as std::os::raw::c_ulong,
            ));
            Some(Parent { raw, id: p.handle })
        }
        #[cfg(windows)]
        ParentKind::Win32 => {
            let hwnd = std::num::NonZeroIsize::new(p.handle as isize)?;
            let raw = RawWindowHandle::Win32(wry::raw_window_handle::Win32WindowHandle::new(hwnd));
            Some(Parent { raw, id: p.handle })
        }
        #[allow(unreachable_patterns)]
        _ => {
            let _ = ParentKind::X11;
            None
        }
    }
}

/// The embedded page: the web view plus whatever the platform needs to place it.
pub struct Embedded {
    #[cfg(target_os = "linux")]
    window: tao::window::Window,
}

/// Creates the web view as a hidden child of `parent`.
pub fn build<T: 'static>(
    target: &EventLoopWindowTarget<T>,
    builder: WebViewBuilder<'_>,
    parent: &Parent,
) -> Result<(WebView, Embedded), String> {
    #[cfg(target_os = "linux")]
    {
        linux::build(target, builder, parent)
    }
    #[cfg(windows)]
    {
        let _ = target;
        use wry::Rect;
        use wry::dpi::{PhysicalPosition, PhysicalSize};
        let view = builder
            .with_bounds(Rect {
                position: PhysicalPosition::new(0, 0).into(),
                size: PhysicalSize::new(1u32, 1u32).into(),
            })
            .with_visible(false)
            .build_as_child(parent)
            .map_err(|e| e.to_string())?;
        Ok((view, Embedded {}))
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = (target, builder, parent);
        Err("not supported on this system".into())
    }
}

impl Embedded {
    /// Moves / resizes the page and shows or hides it.
    pub fn place(&self, view: &WebView, r: PixelRect, visible: bool) {
        #[cfg(target_os = "linux")]
        linux::place(&self.window, r, visible);
        #[cfg(windows)]
        {
            use wry::Rect;
            use wry::dpi::{PhysicalPosition, PhysicalSize};
            let _ = view.set_bounds(Rect {
                position: PhysicalPosition::new(r.x, r.y).into(),
                size: PhysicalSize::new(r.w, r.h).into(),
            });
            let _ = view.set_visible(visible);
        }
        #[cfg(not(any(target_os = "linux", windows)))]
        let _ = (view, r, visible);
        #[cfg(target_os = "linux")]
        let _ = view;
    }

    /// Gives the keyboard focus to the page.
    pub fn focus_page(&self, view: &WebView) {
        let _ = view.focus();
        #[cfg(target_os = "linux")]
        linux::focus(&self.window);
    }

    /// Gives the keyboard focus back to Aurora's window.
    pub fn blur(&self, view: &WebView, parent: &Parent) {
        let _ = view.focus_parent();
        #[cfg(target_os = "linux")]
        linux::set_x_focus(parent.id as std::os::raw::c_ulong);
        #[cfg(not(target_os = "linux"))]
        let _ = parent;
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::os::raw::{c_int, c_ulong};

    use gdkx11::X11Window;
    use gtk::glib::Propagation;
    use gtk::glib::object::Cast;
    use gtk::glib::translate::ToGlibPtr;
    use gtk::prelude::*;
    use tao::event_loop::EventLoopWindowTarget;
    use tao::platform::unix::{WindowBuilderExtUnix, WindowExtUnix};
    use tao::window::{Window, WindowBuilder};
    use wry::{WebViewBuilder, WebViewBuilderExtUnix, WebViewExtUnix};
    use x11_dl::xlib::{Display, XErrorEvent, Xlib};

    use super::{Embedded, Parent};
    use crate::proto::PixelRect;

    /// The X window of a GTK window and the X display GDK uses.
    fn x_ids(window: &Window) -> Option<(c_ulong, *mut Display)> {
        let gdk = window.gtk_window().window()?;
        let xid = gdk.clone().downcast::<X11Window>().ok()?.xid();
        let display = gdk.display().downcast::<gdkx11::X11Display>().ok()?;
        let raw: *mut gdkx11::ffi::GdkX11Display = display.to_glib_none().0;
        let xdisplay = unsafe { gdkx11::ffi::gdk_x11_display_get_xdisplay(raw) };
        Some((xid, xdisplay as *mut Display))
    }

    pub fn build<T: 'static>(
        target: &EventLoopWindowTarget<T>,
        builder: WebViewBuilder<'_>,
        parent: &Parent,
    ) -> Result<(wry::WebView, Embedded), String> {
        // A normal GTK window that is never shown to the window manager: it is moved into
        // Aurora's window before it is first mapped.
        let window = WindowBuilder::new()
            .with_decorations(false)
            .with_visible(false)
            .with_skip_taskbar(true)
            .build(target)
            .map_err(|e| e.to_string())?;
        let vbox = window
            .default_vbox()
            .ok_or_else(|| "window without GTK container".to_string())?;
        let view = builder.build_gtk(vbox).map_err(|e| e.to_string())?;
        window.gtk_window().realize();
        let (xid, display) = x_ids(&window).ok_or_else(|| "no X11 window".to_string())?;
        let xlib = Xlib::open().map_err(|e| e.to_string())?;
        unsafe {
            (xlib.XReparentWindow)(display, xid, parent.id as c_ulong, 0, 0);
            (xlib.XFlush)(display);
        }
        install_focus_hook(&view, &window);
        Ok((view, Embedded { window }))
    }

    pub fn place(window: &Window, r: PixelRect, visible: bool) {
        let Some((xid, display)) = x_ids(window) else {
            return;
        };
        let Ok(xlib) = Xlib::open() else { return };
        unsafe {
            (xlib.XMoveResizeWindow)(display, xid, r.x, r.y, r.w, r.h);
            (xlib.XFlush)(display);
        }
        window.set_visible(visible);
    }

    /// X input focus to `window` (own short-lived connection; focus is server-global).
    pub fn set_x_focus(window: c_ulong) {
        use x11_dl::xlib::{CurrentTime, RevertToParent};

        // Focusing a window that is not viewable (minimized, hidden) is a BadMatch,
        // which Xlib's default handler turns into exit(): ignore it.
        unsafe extern "C" fn ignore_x_error(_: *mut Display, _: *mut XErrorEvent) -> c_int {
            0
        }

        let Ok(xlib) = Xlib::open() else { return };
        unsafe {
            let display = (xlib.XOpenDisplay)(std::ptr::null());
            if display.is_null() {
                return;
            }
            let old = (xlib.XSetErrorHandler)(Some(ignore_x_error));
            (xlib.XSetInputFocus)(display, window, RevertToParent, CurrentTime);
            (xlib.XSync)(display, 0);
            (xlib.XSetErrorHandler)(old);
            (xlib.XCloseDisplay)(display);
        }
    }

    pub fn focus(window: &Window) {
        if let Some((xid, _)) = x_ids(window) {
            set_x_focus(xid);
        }
    }

    /// A click in the page gives it the keyboard focus (X11 hands focus to a foreign
    /// child window only on request).
    fn install_focus_hook(view: &wry::WebView, window: &Window) {
        let gtk_window = window.gtk_window().clone();
        view.webview().connect_button_press_event(move |w, _| {
            if let Some(xid) = gtk_window
                .window()
                .and_then(|g| g.downcast::<X11Window>().ok())
                .map(|g| g.xid())
            {
                set_x_focus(xid);
            }
            w.grab_focus();
            Propagation::Proceed
        });
    }
}
