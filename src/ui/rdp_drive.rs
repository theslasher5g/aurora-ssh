//! RDP drive redirection (host side): the host settings rows, the state shown in the
//! session header and the commands for the helper. The file system access itself (and the
//! confinement to the shared folder) lives in `crates/aurora-rdp/src/drive.rs`.

use egui::{Color32, RichText};

use crate::backend::rdp_embed::{DriveEv, RdpCmd, RdpDriveCfg};
use crate::model::RdpSettings;
use crate::theme::{self, Rows};

/// Characters of a drive name Windows shows (the helper cleans it again).
pub const NAME_MAX: usize = 24;

/// Cleans what the user typed or dragged into the folder field: surrounding quotes,
/// `file://` URIs (file managers) and whitespace.
pub fn normalize_path(input: &str) -> String {
    let mut s = input.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    if let Some(rest) = s.strip_prefix("file://") {
        s = rest.strip_prefix("localhost").unwrap_or(rest);
        let decoded = percent_decode(s);
        // `file:///C:/x` -> `C:/x`
        let d = decoded.as_str();
        let trimmed =
            if cfg!(windows) && d.len() > 2 && d.starts_with('/') && d.as_bytes()[2] == b':' {
                &d[1..]
            } else {
                d
            };
        return trimmed.to_string();
    }
    s.to_string()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

/// Is `path` a folder that may be shared? (Same rules as the helper: a real folder, not the
/// top of a file system.)
pub fn check_folder(path: &str) -> Result<std::path::PathBuf, String> {
    let p = normalize_path(path);
    if p.is_empty() {
        return Err(t!("Kein Ordner angegeben").into());
    }
    let canon = std::fs::canonicalize(&p).map_err(|_| t!("Ordner nicht gefunden").to_string())?;
    if !canon.is_dir() {
        return Err(t!("Das ist kein Ordner").into());
    }
    if canon.parent().is_none() {
        return Err(t!("Ein ganzes Laufwerk kann nicht freigegeben werden").into());
    }
    Ok(canon)
}

/// "Laufwerk: Docs (nur lesen)".
pub fn label(name: &str, writable: bool) -> String {
    let mode = if writable {
        t!("lesen und schreiben")
    } else {
        t!("nur lesen")
    };
    t!("Laufwerk: {} ({})", name, mode)
}

/// What the tab knows about the drive of its session.
#[derive(Default, Debug, Clone)]
pub struct DriveState {
    /// The session was started with the drive channel (a host setting, off by default).
    pub enabled: bool,
    pub path: String,
    pub name: String,
    pub write: bool,
    /// A drive is currently shown in the session.
    pub shared: bool,
    pub shown_name: String,
    pub shown_writable: bool,
    pub read: u64,
    pub written: u64,
    pub denied: u64,
    pub last_denied: String,
    toasted_at: u64,
    pub error: Option<String>,
}

impl DriveState {
    pub fn from_settings(s: &RdpSettings) -> Self {
        Self {
            enabled: s.drive_enabled && !s.drive_path.trim().is_empty(),
            path: normalize_path(&s.drive_path),
            name: s.drive_name.clone(),
            write: s.drive_write,
            ..Self::default()
        }
    }

    /// The drive to open with the connection (`None` = no drive channel).
    pub fn connect_cfg(&self) -> Option<RdpDriveCfg> {
        if !self.enabled {
            return None;
        }
        let path = check_folder(&self.path).ok()?;
        Some(RdpDriveCfg {
            path: path.to_string_lossy().into_owned(),
            name: self.name.clone(),
            writable: self.write,
        })
    }

    pub fn apply(&mut self, ev: DriveEv, toasts: &mut Vec<(String, Color32)>) {
        match ev {
            DriveEv::State {
                shared,
                name,
                writable,
            } => {
                self.shared = shared;
                if shared {
                    self.shown_name = name;
                    self.shown_writable = writable;
                    self.error = None;
                }
            }
            DriveEv::Activity {
                read,
                written,
                denied,
                last_denied,
            } => {
                self.read = read;
                self.written = written;
                self.denied = denied;
                self.last_denied = last_denied;
                // Tell the user when the session tries things outside the share's rules:
                // at the first refusal and then whenever the count doubles.
                if denied > 0 && (self.toasted_at == 0 || denied >= self.toasted_at * 2) {
                    self.toasted_at = denied;
                    toasts.push((
                        t!(
                            "Laufwerk: {} Zugriffe wurden blockiert (außerhalb des Ordners oder nur lesen)",
                            denied
                        )
                        .to_string(),
                        theme::amber(),
                    ));
                }
            }
            DriveEv::Error { detail } => {
                toasts.push((t!("Laufwerk: {}", detail).to_string(), theme::red()));
                self.error = Some(detail);
            }
        }
    }

    /// Command to re-share the configured folder in a running session.
    pub fn share_cmd(&self) -> Option<RdpCmd> {
        let cfg = self.connect_cfg()?;
        Some(RdpCmd::DriveShare {
            path: cfg.path,
            name: cfg.name,
            writable: cfg.writable,
        })
    }

    fn menu_id() -> egui::Id {
        egui::Id::new("rdp-drive-menu")
    }

    fn status(&self) -> (String, Color32) {
        if self.shared {
            (
                label(&self.shown_name, self.shown_writable),
                if self.shown_writable {
                    theme::amber()
                } else {
                    theme::green()
                },
            )
        } else {
            (
                t!("Laufwerk: nicht freigegeben").to_string(),
                theme::text_dim(),
            )
        }
    }

    /// Header entry: the indicator "Laufwerk: <name> (nur lesen)". Clicking it opens the
    /// menu of the badge on the screen (a menu inside the folded "…" menu of a narrow header
    /// would close with it).
    pub fn header(&mut self, ui: &mut egui::Ui) {
        if !self.enabled {
            return;
        }
        let (text, color) = self.status();
        let chip = theme::chip(ui, &text, color).on_hover_cursor(egui::CursorIcon::PointingHand);
        if chip.clicked() {
            egui::Popup::toggle_id(ui.ctx(), Self::menu_id());
        }
    }

    /// Badge on the remote screen (always visible, even in a very narrow pane whose header
    /// folds everything into one menu). Its menu shows the traffic and disconnects the
    /// share (or shares the folder again). Returns the command to send, if any.
    pub fn badge(
        &mut self,
        ui: &mut egui::Ui,
        area: egui::Rect,
        connected: bool,
    ) -> Option<RdpCmd> {
        if !self.enabled {
            return None;
        }
        let (text, color) = self.status();
        let galley = ui
            .painter()
            .layout_no_wrap(text.clone(), theme::medium(11.5), Color32::WHITE);
        let size = galley.size() + egui::vec2(26.0, 8.0);
        let max_w = (area.width() - 12.0).max(40.0);
        let size = egui::vec2(size.x.min(max_w), size.y);
        let rect = egui::Rect::from_min_size(
            egui::pos2(area.left() + 6.0, area.bottom() - size.y - 6.0),
            size,
        );
        let resp = ui
            .interact(rect, Self::menu_id().with("badge"), egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let p = ui.painter().with_clip_rect(rect);
        p.rect_filled(
            rect,
            egui::CornerRadius::same(8),
            Color32::from_black_alpha(170),
        );
        p.circle_filled(egui::pos2(rect.left() + 11.0, rect.center().y), 3.5, color);
        p.galley(
            egui::pos2(rect.left() + 20.0, rect.center().y - galley.size().y / 2.0),
            galley,
            Color32::WHITE,
        );
        let mut cmd = None;
        egui::Popup::menu(&resp)
            .id(Self::menu_id())
            .align(egui::RectAlign::TOP_START)
            .show(|ui| {
                ui.set_min_width(250.0);
                if self.shared {
                    ui.label(RichText::new(&text).strong());
                    ui.label(
                        RichText::new(t!(
                            "{} gelesen · {} geschrieben",
                            fmt_bytes(self.read),
                            fmt_bytes(self.written)
                        ))
                        .size(12.5)
                        .color(theme::text_dim()),
                    );
                    if self.denied > 0 {
                        ui.label(
                            RichText::new(t!("{} Zugriffe blockiert", self.denied))
                                .size(12.5)
                                .color(theme::amber()),
                        );
                        if !self.last_denied.is_empty() {
                            ui.label(
                                RichText::new(&self.last_denied)
                                    .size(12.0)
                                    .color(theme::text_dim()),
                            );
                        }
                    }
                    ui.separator();
                    if ui
                        .add_enabled(connected, egui::Button::new(t!("Freigabe trennen")))
                        .clicked()
                    {
                        cmd = Some(RdpCmd::DriveUnshare);
                        ui.close();
                    }
                } else {
                    if let Some(e) = &self.error {
                        ui.label(RichText::new(e).size(12.5).color(theme::red()));
                    }
                    if ui
                        .add_enabled(
                            connected && self.share_cmd().is_some(),
                            egui::Button::new(t!("Ordner wieder freigeben")),
                        )
                        .clicked()
                    {
                        cmd = self.share_cmd();
                        ui.close();
                    }
                }
            });
        cmd
    }
}

pub fn fmt_bytes(n: u64) -> String {
    const K: f64 = 1024.0;
    let f = n as f64;
    if f < K {
        format!("{n} B")
    } else if f < K * K {
        format!("{:.1} KB", f / K)
    } else if f < K * K * K {
        format!("{:.1} MB", f / K / K)
    } else {
        format!("{:.2} GB", f / K / K / K)
    }
}

/// Rows of the host editor: drive redirection (switch, folder, name, write access).
pub fn settings_rows(ui: &mut egui::Ui, f: &mut Rows, rdp: &mut RdpSettings) {
    f.switch(ui, t!("Laufwerk freigeben"), &mut rdp.drive_enabled);
    if !rdp.drive_enabled {
        return;
    }
    // A folder dragged from the file manager onto the window fills the field.
    let dropped = ui.ctx().input(|i| {
        i.raw
            .dropped_files
            .iter()
            .map(|d| d.path().to_path_buf())
            .find(|p| p.is_dir())
    });
    if let Some(p) = dropped {
        rdp.drive_path = p.to_string_lossy().into_owned();
    }
    f.field(ui, t!("Ordner"), |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut rdp.drive_path)
                .hint_text(
                    RichText::new(t!("Pfad eintippen oder Ordner hierher ziehen"))
                        .color(theme::text_dim()),
                )
                .desired_width(f32::INFINITY),
        );
    });
    let status = if rdp.drive_path.trim().is_empty() {
        None
    } else {
        Some(check_folder(&rdp.drive_path))
    };
    f.field(ui, t!("Name"), |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut rdp.drive_name)
                .char_limit(NAME_MAX)
                .hint_text(RichText::new(t!("Ordnername")).color(theme::text_dim()))
                .desired_width(f32::INFINITY),
        );
    });
    f.switch(ui, t!("Schreiben erlauben"), &mut rdp.drive_write);
    f.full(ui, |ui| {
        let (msg, color) = match status {
            None => (
                t!("Ohne Ordner wird kein Laufwerk freigegeben").to_string(),
                theme::text_dim(),
            ),
            Some(Err(e)) => (e, theme::red()),
            Some(Ok(p)) => (
                if rdp.drive_write {
                    t!(
                        "Die Sitzung darf in „{}“ Dateien anlegen, ändern und löschen – nur dort",
                        p.display()
                    )
                    .to_string()
                } else {
                    t!(
                        "Die Sitzung darf „{}“ nur lesen; Änderungen sind nicht möglich",
                        p.display()
                    )
                    .to_string()
                },
                if rdp.drive_write {
                    theme::amber()
                } else {
                    theme::text_dim()
                },
            ),
        };
        ui.add(egui::Label::new(RichText::new(msg).size(12.5).color(color)).wrap());
        ui.add(
            egui::Label::new(
                RichText::new(t!(
                    "Gilt ab der nächsten Verbindung. Der Ordner erscheint im Explorer des Hosts; Verknüpfungen aus dem Ordner heraus werden nicht gefolgt, nichts wird ausgeführt."
                ))
                .size(12.5)
                .color(theme::text_dim()),
            )
            .wrap(),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_input_is_cleaned() {
        assert_eq!(normalize_path("  /home/me/Docs  "), "/home/me/Docs");
        assert_eq!(normalize_path("\"/home/me/My Docs\""), "/home/me/My Docs");
        assert_eq!(normalize_path("'/tmp/x'"), "/tmp/x");
        #[cfg(not(windows))]
        assert_eq!(
            normalize_path("file:///home/me/My%20Docs"),
            "/home/me/My Docs"
        );
        assert_eq!(normalize_path(""), "");
    }

    #[test]
    fn folder_check() {
        let dir = std::env::temp_dir().join(format!("aurora-drive-ui-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(check_folder(&dir.to_string_lossy()).is_ok());
        assert!(check_folder("").is_err());
        assert!(check_folder("/definitely/not/here").is_err());
        let file = dir.join("f.txt");
        std::fs::write(&file, b"x").unwrap();
        assert!(check_folder(&file.to_string_lossy()).is_err());
        #[cfg(unix)]
        assert!(check_folder("/").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn state_follows_settings_and_events() {
        let mut s = RdpSettings::default();
        assert!(DriveState::from_settings(&s).connect_cfg().is_none());
        s.drive_enabled = true;
        assert!(
            DriveState::from_settings(&s).connect_cfg().is_none(),
            "no folder, no drive"
        );
        s.drive_path = std::env::temp_dir().to_string_lossy().into_owned();
        s.drive_name = "Tmp".into();
        let mut st = DriveState::from_settings(&s);
        let cfg = st.connect_cfg().unwrap();
        assert_eq!(cfg.name, "Tmp");
        assert!(!cfg.writable, "read only unless asked");
        let mut toasts = Vec::new();
        st.apply(
            DriveEv::State {
                shared: true,
                name: "Tmp".into(),
                writable: false,
            },
            &mut toasts,
        );
        assert!(st.shared && toasts.is_empty());
        let act = |n| DriveEv::Activity {
            read: 1,
            written: 0,
            denied: n,
            last_denied: "\\..\\x".into(),
        };
        st.apply(act(1), &mut toasts);
        assert_eq!(toasts.len(), 1);
        st.apply(act(1), &mut toasts);
        st.apply(act(1), &mut toasts);
        assert_eq!(toasts.len(), 1, "no repeat for the same count");
        st.apply(act(2), &mut toasts);
        assert_eq!(toasts.len(), 2, "doubling is reported again");
        st.apply(
            DriveEv::State {
                shared: false,
                name: String::new(),
                writable: false,
            },
            &mut toasts,
        );
        assert!(!st.shared);
        assert!(matches!(
            st.share_cmd(),
            Some(RdpCmd::DriveShare {
                writable: false,
                ..
            })
        ));
    }

    #[test]
    fn labels_and_sizes() {
        crate::i18n::set_language("de");
        assert_eq!(label("Docs", false), "Laufwerk: Docs (nur lesen)");
        assert_eq!(label("Docs", true), "Laufwerk: Docs (lesen und schreiben)");
        assert_eq!(fmt_bytes(10), "10 B");
        assert_eq!(fmt_bytes(2048), "2.0 KB");
        assert_eq!(fmt_bytes(5 << 20), "5.0 MB");
    }
}
