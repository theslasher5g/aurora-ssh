//! `aurora-web` – opens the web interface of a firewall, NAS, hypervisor … in its own
//! window with a private, persistent profile (WebView2 / WKWebView / WebKitGTK via wry).
//!
//! Protocol (see `proto.rs`): Aurora writes one `WebStart` JSON line to stdin, then
//! `WebCmd` lines; the helper answers with `WebEvent` lines on stdout. Nothing sensitive
//! is ever passed through argv or the environment, and nothing is logged.
//!
//! The TLS policy is enforced HERE (not only in Aurora): a certificate error is only
//! overridden when the fingerprint matches the pin Aurora handed over (or the user
//! confirmed it through `TrustCert`).

mod embed;
#[allow(dead_code)]
mod proto;

use std::cell::RefCell;
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use proto::{
    Credentials, TlsMode, TlsVerdict, WebCmd, WebEvent, WebStart, WebUrl, autofill_script,
    navigation_allowed, parse_cmd, parse_start, parse_url, read_bounded_line, safe_download_name,
    short_url, tls_verdict,
};
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::window::WindowBuilder;
use wry::{NewWindowResponse, PageLoadEvent, WebContext, WebView, WebViewBuilder};

enum UserEvent {
    Cmd(WebCmd),
    /// Load `url` in the window (same-origin `window.open`).
    Load(String),
    /// A page finished loading (time to auto-login).
    PageLoaded,
    /// Aurora closed the pipe.
    Gone,
}

/// Mutable state shared by the callbacks (all on the UI thread).
struct State {
    mode: TlsMode,
    trusted: Vec<String>,
    base: WebUrl,
    home: String,
    /// Last URL that failed because of its certificate (reloaded after `TrustCert`).
    pending: Option<String>,
    error_shown: bool,
    credentials: Option<Credentials>,
    user_selector: String,
    pass_selector: String,
    submit: bool,
    /// Auto-submits left (1 per start/reload): a wrong password must not be retried in a
    /// loop, which would lock the account.
    submit_budget: Arc<AtomicU32>,
}

type Shared = Rc<RefCell<State>>;

fn emit(ev: &WebEvent) {
    if let Ok(line) = serde_json::to_string(ev) {
        let out = std::io::stdout();
        let mut out = out.lock();
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }
}

fn fail(message: &str) -> ! {
    emit(&WebEvent::Error {
        message: message.to_string(),
    });
    emit(&WebEvent::Closed);
    std::process::exit(2);
}

#[allow(dead_code)]
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[allow(dead_code)]
fn error_page(title: &str, lines: &[String]) -> String {
    let body: String = lines
        .iter()
        .map(|l| format!("<p>{}</p>", html_escape(l)))
        .collect();
    format!(
        "<!doctype html><meta charset=utf-8><meta http-equiv=\"Content-Security-Policy\" \
         content=\"default-src 'none'; style-src 'unsafe-inline'\"><title>{t}</title>\
         <style>body{{font:15px system-ui,sans-serif;background:#14161c;color:#e6e8ee;\
         margin:12vh auto;max-width:640px;padding:0 24px}}h1{{font-size:22px;color:#ff7a7a}}\
         p{{line-height:1.5;color:#b9bfcc;word-break:break-all}}</style>\
         <h1>{t}</h1>{body}",
        t = html_escape(title),
    )
}

fn secure_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// A file name in `dir` that does not exist yet.
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    for n in 1..1000 {
        let p = dir.join(format!("{stem} ({n}){ext}"));
        if !p.exists() {
            return p;
        }
    }
    dir.join(format!("{stem}-{}{ext}", std::process::id()))
}

fn profile_id_bytes(id: &str) -> [u8; 16] {
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(id.as_bytes());
    let mut out = [0u8; 16];
    out.copy_from_slice(&d[..16]);
    out
}

/// Decision for a certificate error, shared by all platforms. Reports to Aurora and
/// returns whether the page may load.
fn judge_certificate(state: &Shared, fingerprint: &str) -> (bool, Vec<String>) {
    let (mode, trusted) = {
        let s = state.borrow();
        (s.mode, s.trusted.clone())
    };
    match tls_verdict(mode, &trusted, fingerprint) {
        TlsVerdict::Allow => (true, Vec::new()),
        TlsVerdict::Refuse => {
            emit(&WebEvent::Cert {
                fingerprint: fingerprint.to_string(),
                expected: None,
                strict: true,
            });
            (
                false,
                vec![
                    "Das Zertifikat des Servers ist nicht vertrauenswürdig (strenge Prüfung)."
                        .into(),
                    format!("SHA-256: {fingerprint}"),
                ],
            )
        }
        TlsVerdict::Unknown => {
            emit(&WebEvent::Cert {
                fingerprint: fingerprint.to_string(),
                expected: None,
                strict: false,
            });
            (
                false,
                vec![
                    "Unbekanntes Zertifikat. Aurora fragt, ob es angeheftet werden soll.".into(),
                    format!("SHA-256: {fingerprint}"),
                ],
            )
        }
        TlsVerdict::Mismatch { expected } => {
            emit(&WebEvent::Cert {
                fingerprint: fingerprint.to_string(),
                expected: Some(expected.clone()),
                strict: false,
            });
            (
                false,
                vec![
                    "ACHTUNG: Das Zertifikat unterscheidet sich vom angehefteten!".into(),
                    format!("Angeheftet: {expected}"),
                    format!("Aktuell:    {fingerprint}"),
                ],
            )
        }
    }
}

#[cfg(target_os = "linux")]
fn install_tls_hook(view: &WebView, state: &Shared) {
    use webkit2gtk::gio::prelude::TlsCertificateExt;
    use webkit2gtk::{WebContextExt, WebViewExt};
    use wry::WebViewExtUnix;

    let wv = view.webview();
    let state = state.clone();
    wv.connect_load_failed_with_tls_errors(move |wv, uri, cert, _errors| {
        let der = cert.certificate().map(|b| b.to_vec()).unwrap_or_default();
        let fp = proto::fingerprint_sha256(&der);
        let host = parse_url(uri).map(|u| u.host).unwrap_or_default();
        let (allow, lines) = judge_certificate(&state, &fp);
        if allow && !der.is_empty() && !host.is_empty() {
            if let Some(ctx) = wv.context() {
                ctx.allow_tls_certificate_for_host(cert, &host);
            }
            wv.load_uri(uri);
            return true;
        }
        state.borrow_mut().pending = Some(uri.to_string());
        state.borrow_mut().error_shown = true;
        wv.load_html(&error_page("Zertifikat nicht akzeptiert", &lines), None);
        true
    });
}

#[cfg(target_os = "windows")]
fn install_tls_hook(view: &WebView, state: &Shared) {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_ALWAYS_ALLOW,
        COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_CANCEL, ICoreWebView2_14,
    };
    use webview2_com::ServerCertificateErrorDetectedEventHandler;
    use windows::core::{Interface, PWSTR};
    use wry::WebViewExtWindows;

    let Ok(core) = view.webview().cast::<ICoreWebView2_14>() else {
        emit(&WebEvent::Error {
            message: "WebView2-Laufzeit zu alt für die Zertifikatsprüfung (Edge aktualisieren)"
                .into(),
        });
        return;
    };
    let state = state.clone();
    let handler = ServerCertificateErrorDetectedEventHandler::create(Box::new(move |_wv, args| {
        let Some(args) = args else { return Ok(()) };
        unsafe {
            let mut pem = PWSTR::null();
            let der = args
                .ServerCertificate()
                .ok()
                .and_then(|c| {
                    c.ToPemEncoding(&mut pem).ok()?;
                    Some(webview2_com::take_pwstr(pem))
                })
                .and_then(|p| proto::pem_to_der(&p))
                .unwrap_or_default();
            let fp = proto::fingerprint_sha256(&der);
            let (allow, _) = judge_certificate(&state, &fp);
            if allow && !der.is_empty() {
                let _ = args.SetAction(COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_ALWAYS_ALLOW);
            } else {
                let mut uri = PWSTR::null();
                if args.RequestUri(&mut uri).is_ok() {
                    state.borrow_mut().pending = Some(webview2_com::take_pwstr(uri));
                }
                let _ = args.SetAction(COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_CANCEL);
            }
        }
        Ok(())
    }));
    let mut token = Default::default();
    unsafe {
        let _ = core.add_ServerCertificateErrorDetected(&handler, &mut token);
    }
}

#[cfg(target_os = "macos")]
fn install_tls_hook(_view: &WebView, state: &Shared) {
    // WKWebView offers no certificate hook through wry: only certificates the system
    // trusts load. Say so instead of pretending to pin.
    if state.borrow().mode != TlsMode::Strict {
        emit(&WebEvent::Error {
            message: "Zertifikat-Pinning wird unter macOS noch nicht unterstützt – nur vom System \
                      anerkannte Zertifikate laden"
                .into(),
        });
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn install_tls_hook(_view: &WebView, _state: &Shared) {}

fn main() {
    let mut stdin = BufReader::new(std::io::stdin());
    let start: WebStart = match read_bounded_line(&mut stdin, proto::MAX_LINE) {
        Ok(Some(line)) => match parse_start(&line) {
            Ok(s) => s,
            Err(e) => fail(&e),
        },
        _ => fail("Kein Startbefehl erhalten"),
    };
    let base = match parse_url(&start.url) {
        Ok(u) => u,
        Err(e) => fail(e.message()),
    };
    let profile = PathBuf::from(&start.profile_dir);
    if let Err(e) = secure_dir(&profile) {
        fail(&format!("Profilordner: {e}"));
    }
    let download_dir = (!start.download_dir.is_empty()).then(|| PathBuf::from(&start.download_dir));
    if let Some(d) = &download_dir {
        let _ = std::fs::create_dir_all(d);
    }

    let state: Shared = Rc::new(RefCell::new(State {
        mode: start.tls,
        trusted: if start.tls == TlsMode::Pinned && !start.pinned_fp.is_empty() {
            vec![start.pinned_fp.clone()]
        } else {
            Vec::new()
        },
        home: base.to_url(),
        base: base.clone(),
        pending: None,
        error_shown: false,
        credentials: start
            .credentials
            .clone()
            .filter(|c| !c.password.is_empty() || !c.username.is_empty()),
        user_selector: start.user_selector.clone(),
        pass_selector: start.pass_selector.clone(),
        submit: start.submit,
        submit_budget: Arc::new(AtomicU32::new(1)),
    }));
    let extra_hosts = start.extra_hosts.clone();

    #[cfg(target_os = "linux")]
    {
        // GTK must be initialised before the event loop is created.
        if gtk_init_failed() {
            fail("Kein Display verfügbar (GTK konnte nicht gestartet werden)");
        }
    }

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy: EventLoopProxy<UserEvent> = event_loop.create_proxy();

    let embed_parent = start.embed.as_ref().and_then(embed::parent);

    let mut context = WebContext::new(Some(profile));

    // Everything the page callbacks need is cloned per call, so the builder can be made
    // twice (embedded attempt, then the own-window fallback).
    macro_rules! make_builder {
        ($context:expr) => {{
            let nav_state = state.clone();
            let nav_extra = extra_hosts.clone();
            let win_state = state.clone();
            let win_extra = extra_hosts.clone();
            let win_proxy = proxy.clone();
            let load_proxy = proxy.clone();
            let download_dir = download_dir.clone();
            let builder = WebViewBuilder::new_with_web_context($context)
                .with_url(base.to_url())
                .with_devtools(cfg!(debug_assertions))
                .with_navigation_handler(move |url| {
                    let base = nav_state.borrow().base.clone();
                    if navigation_allowed(&base, &nav_extra, &url) {
                        if url != "about:blank" {
                            // A real page is being loaded: whatever error page was shown is gone.
                            nav_state.borrow_mut().error_shown = false;
                        }
                        return true;
                    }
                    report_foreign(&url);
                    false
                })
                .with_new_window_req_handler(move |url, _features| {
                    let base = win_state.borrow().base.clone();
                    if navigation_allowed(&base, &win_extra, &url) && url != "about:blank" {
                        let _ = win_proxy.send_event(UserEvent::Load(url));
                    } else {
                        report_foreign(&url);
                    }
                    // Never a second window: same-origin targets load in this one.
                    NewWindowResponse::Deny
                })
                .with_document_title_changed_handler(|title| {
                    let title: String = title.chars().take(200).collect();
                    emit(&WebEvent::Title { title });
                })
                .with_on_page_load_handler(move |event, url| {
                    if !matches!(event, PageLoadEvent::Finished) {
                        return;
                    }
                    emit(&WebEvent::Loaded {
                        url: short_url(&url),
                    });
                    let _ = load_proxy.send_event(UserEvent::PageLoaded);
                })
                .with_download_started_handler(move |url, dest| {
                    let Some(dir) = download_dir.as_ref() else {
                        emit(&WebEvent::Blocked {
                            url: short_url(&url),
                        });
                        return false;
                    };
                    let name = safe_download_name(
                        &dest
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                    );
                    *dest = unique_path(dir, &name);
                    true
                })
                .with_download_completed_handler(|url, path, ok| {
                    let name = path
                        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                        .unwrap_or_else(|| safe_download_name(&short_url(&url)));
                    emit(&WebEvent::Download { name, ok });
                });
            #[cfg(target_os = "macos")]
            let builder = {
                use wry::WebViewBuilderExtDarwin;
                builder.with_data_store_identifier(profile_id_bytes(&start.profile_id))
            };
            #[cfg(not(target_os = "macos"))]
            let _ = profile_id_bytes;
            builder
        }};
    }

    // Embedded: the page is a child window of Aurora's window. If the window system
    // refuses (old WebView2, no X11 ...) the helper falls back to its own window.
    let mut window: Option<tao::window::Window> = None;
    let mut view: Option<WebView> = None;
    let mut embedded_ctl: Option<embed::Embedded> = None;
    if let Some(parent) = &embed_parent {
        match embed::build(&event_loop, make_builder!(&mut context), parent) {
            Ok((v, ctl)) => {
                view = Some(v);
                embedded_ctl = Some(ctl);
            }
            Err(e) => emit(&WebEvent::Error {
                message: format!("Einbetten nicht möglich, eigenes Fenster: {e}"),
            }),
        }
    }
    let embedded = embedded_ctl.is_some();
    if view.is_none() {
        let w = match WindowBuilder::new()
            .with_title(&start.title)
            .with_inner_size(LogicalSize::new(1280.0, 840.0))
            .build(&event_loop)
        {
            Ok(w) => w,
            Err(e) => fail(&format!("Fenster: {e}")),
        };
        let builder = make_builder!(&mut context);
        #[cfg(target_os = "linux")]
        let built = {
            use tao::platform::unix::WindowExtUnix;
            use wry::WebViewBuilderExtUnix;
            let vbox = w
                .default_vbox()
                .unwrap_or_else(|| fail("Fenster ohne GTK-Container"));
            builder.build_gtk(vbox)
        };
        #[cfg(not(target_os = "linux"))]
        let built = builder.build(&w);
        view = Some(match built {
            Ok(v) => v,
            Err(e) => fail(&format!(
                "Webansicht: {e}{}",
                if cfg!(windows) {
                    " (ist die Microsoft-Edge-WebView2-Laufzeit installiert?)"
                } else {
                    ""
                }
            )),
        });
        window = Some(w);
    }
    let Some(view) = view else {
        fail("Webansicht konnte nicht erstellt werden")
    };
    install_tls_hook(&view, &state);
    emit(&WebEvent::Ready {
        engine: engine_name(),
        embedded,
    });

    let autofill_state = state.clone();

    // Commands from Aurora.
    {
        let proxy = proxy.clone();
        std::thread::spawn(move || {
            let mut stdin = stdin;
            loop {
                match read_bounded_line(&mut stdin, proto::MAX_LINE) {
                    Ok(Some(line)) => {
                        if line.is_empty() {
                            continue;
                        }
                        match parse_cmd(&line) {
                            Ok(cmd) => {
                                if proxy.send_event(UserEvent::Cmd(cmd)).is_err() {
                                    break;
                                }
                            }
                            Err(e) => emit(&WebEvent::Error { message: e }),
                        }
                    }
                    // Oversized or malformed line: ignore it, keep running.
                    Err(e) if e.kind() == std::io::ErrorKind::InvalidData => continue,
                    Err(_) | Ok(None) => break,
                }
            }
            let _ = proxy.send_event(UserEvent::Gone);
        });
    }

    let view = Rc::new(view);
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                emit(&WebEvent::Closed);
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(UserEvent::Gone) => {
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(UserEvent::Load(url)) => {
                let _ = view.load_url(&url);
            }
            Event::UserEvent(UserEvent::Cmd(cmd)) => match cmd {
                WebCmd::Focus => {
                    if let Some(window) = &window {
                        window.set_minimized(false);
                        window.set_focus();
                    } else if let Some(ctl) = &embedded_ctl {
                        ctl.focus_page(&view);
                    }
                }
                WebCmd::Bounds {
                    x,
                    y,
                    w,
                    h,
                    visible,
                } => {
                    if let Some(ctl) = &embedded_ctl {
                        ctl.place(&view, proto::sanitize_bounds(x, y, w, h), visible);
                    }
                }
                WebCmd::Blur => {
                    if let (Some(ctl), Some(parent)) = (&embedded_ctl, &embed_parent) {
                        ctl.blur(&view, parent);
                    }
                }
                WebCmd::Reload => {
                    let (err, home, pending) = {
                        // The submit budget is per opening, not per reload: a wrong stored
                        // password must not be sent again with every reload.
                        let s = autofill_state.borrow_mut();
                        (s.error_shown, s.home.clone(), s.pending.clone())
                    };
                    if err || pending.is_some() {
                        let _ = view.load_url(&pending.unwrap_or(home));
                    } else {
                        let _ = view.reload();
                    }
                }
                WebCmd::Home => {
                    let home = {
                        let s = autofill_state.borrow_mut();
                        s.home.clone()
                    };
                    let _ = view.load_url(&home);
                }
                WebCmd::TrustCert { fingerprint } => {
                    let pending = {
                        let mut s = autofill_state.borrow_mut();
                        if s.mode != TlsMode::Pinned {
                            None
                        } else {
                            s.trusted = vec![fingerprint];
                            s.pending.take()
                        }
                    };
                    if let Some(url) = pending {
                        let _ = view.load_url(&url);
                    }
                }
                WebCmd::Close => {
                    emit(&WebEvent::Closed);
                    *control_flow = ControlFlow::Exit;
                }
            },
            Event::UserEvent(UserEvent::PageLoaded) => {
                // Auto-login: once per loaded document, only on the entry's own origin.
                maybe_autofill(&view, &autofill_state);
            }
            _ => {}
        }
    });
}

/// Fills the login form when the current page is on the entry's origin. The page checks
/// its own origin again, so a load event that arrives late cannot misfire.
fn maybe_autofill(view: &WebView, state: &Shared) {
    let (creds, us, ps, submit, allowed, base, budget) = {
        let s = state.borrow();
        let Some(c) = s.credentials.clone() else {
            return;
        };
        (
            c,
            s.user_selector.clone(),
            s.pass_selector.clone(),
            s.submit && s.submit_budget.load(Ordering::SeqCst) > 0,
            !s.error_shown,
            s.base.clone(),
            s.submit_budget.clone(),
        )
    };
    if !allowed {
        return;
    }
    let script = autofill_script(&creds, &base.origin(), &us, &ps, submit);
    let _ = view.evaluate_script_with_callback(&script, move |result| {
        if submit && result.contains("armed") {
            let _ = budget.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
        }
    });
}

/// A link to another origin: Aurora opens it in the system browser (after validating it).
fn report_foreign(url: &str) {
    if parse_url(url).is_ok() {
        emit(&WebEvent::External {
            url: url.to_string(),
        });
    } else {
        emit(&WebEvent::Blocked {
            url: short_url(url),
        });
    }
}

fn engine_name() -> String {
    wry::webview_version().unwrap_or_else(|_| "?".into())
}

#[cfg(target_os = "linux")]
fn gtk_init_failed() -> bool {
    gtk_init().is_err()
}

#[cfg(target_os = "linux")]
fn gtk_init() -> Result<(), ()> {
    // tao initialises GTK itself when the event loop is built; verify a display exists so
    // we can fail with a message instead of a panic.
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return Err(());
    }
    Ok(())
}
