//! Adding existing SSH keys to the keychain: drop key files onto the keys page, or pick them
//! from `~/.ssh`. Only the private key text is read (size-capped), nothing is logged.

use std::path::{Path, PathBuf};

use egui::{Color32, RichText};

use crate::app::App;
use crate::keygen;
use crate::model::Identity;
use crate::theme;

/// Largest key file read (real keys are a few KB; RSA 8192 stays below 8 KB).
const MAX_BYTES: u64 = 64 * 1024;
/// Files looked at in `~/.ssh` at most.
const MAX_SCAN: usize = 200;

/// Why a file is not added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reject {
    TooBig,
    NotText,
    /// A `.pub` file / `ssh-ed25519 AAAA…` line: needs the private half.
    PublicOnly,
    /// PuTTY `.ppk`: export as OpenSSH first.
    Putty,
    NoKey,
}

impl Reject {
    pub fn text(&self) -> String {
        match self {
            Reject::TooBig => t!("Datei zu groß für einen Schlüssel").to_string(),
            Reject::NotText => t!("keine Textdatei").to_string(),
            Reject::PublicOnly => {
                t!("nur der öffentliche Schlüssel (.pub) – der private fehlt").to_string()
            }
            Reject::Putty => {
                t!("PuTTY-Format (.ppk) – in PuTTYgen als OpenSSH exportieren").to_string()
            }
            Reject::NoKey => t!("kein privater SSH-Schlüssel").to_string(),
        }
    }
}

/// A private key found in a file.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub label: String,
    pub private_key: String,
    pub kind: String,
    pub fingerprint: String,
    pub encrypted: bool,
    pub path: PathBuf,
}

/// Name for the keychain entry: the file name without directory and `id_` prefix noise.
pub fn label_of(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = name
        .strip_suffix(".pem")
        .or_else(|| name.strip_suffix(".key"))
        .unwrap_or(&name);
    let clean: String = name.chars().filter(|c| !c.is_control()).take(60).collect();
    let clean = clean.trim().to_string();
    if clean.is_empty() {
        "SSH key".into()
    } else {
        clean
    }
}

/// Parses the bytes of a file.
pub fn parse_key(path: &Path, bytes: &[u8]) -> Result<Candidate, Reject> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err(Reject::TooBig);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| Reject::NotText)?;
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let head = text.trim_start();
    if head.starts_with("PuTTY-User-Key-File") {
        return Err(Reject::Putty);
    }
    if !head.starts_with("-----BEGIN") {
        let first = head.split_whitespace().next().unwrap_or("");
        return Err(
            if first.starts_with("ssh-") || first.starts_with("ecdsa-") || first.starts_with("sk-")
            {
                Reject::PublicOnly
            } else {
                Reject::NoKey
            },
        );
    }
    if !text.contains("PRIVATE KEY-----") {
        return Err(Reject::NoKey);
    }
    let mut private_key = text.trim().to_string();
    private_key.push('\n');
    let meta = keygen::key_meta(&private_key);
    let encrypted = keygen::key_encrypted(&private_key);
    // Neither readable nor recognisably encrypted: not a key we can use.
    if meta.is_none() && !encrypted {
        return Err(Reject::NoKey);
    }
    let (kind, fingerprint) =
        meta.map_or((String::new(), String::new()), |m| (m.kind, m.fingerprint));
    Ok(Candidate {
        label: label_of(path),
        private_key,
        kind,
        fingerprint,
        encrypted,
        path: path.to_path_buf(),
    })
}

/// Reads one file (regular files only, size checked before reading).
pub fn read_candidate(path: &Path) -> Result<Candidate, Reject> {
    let meta = std::fs::symlink_metadata(path).map_err(|_| Reject::NoKey)?;
    let meta = if meta.file_type().is_symlink() {
        // A link the user dropped / `~/.ssh/id_x -> …` is fine, but it must lead to a file.
        std::fs::metadata(path).map_err(|_| Reject::NoKey)?
    } else {
        meta
    };
    if !meta.is_file() {
        return Err(Reject::NoKey);
    }
    if meta.len() > MAX_BYTES {
        return Err(Reject::TooBig);
    }
    let bytes = std::fs::read(path).map_err(|_| Reject::NoKey)?;
    parse_key(path, &bytes)
}

/// Whether the keychain already holds this key (same fingerprint, else same text).
pub fn known(identities: &[Identity], c: &Candidate) -> bool {
    identities.iter().any(|i| {
        if i.private_key.trim().is_empty() {
            return false;
        }
        if !c.fingerprint.is_empty() {
            return keygen::key_meta(&i.private_key)
                .is_some_and(|m| m.fingerprint == c.fingerprint);
        }
        i.private_key.trim() == c.private_key.trim()
    })
}

/// Private keys in `~/.ssh` that the keychain does not have yet.
pub fn scan_ssh_dir(dir: &Path, identities: &[Identity]) -> Vec<Candidate> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Candidate> = Vec::new();
    for e in rd.flatten().take(MAX_SCAN) {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_lowercase();
        if name.ends_with(".pub")
            || matches!(
                name.as_str(),
                "known_hosts" | "known_hosts.old" | "config" | "authorized_keys" | "environment"
            )
        {
            continue;
        }
        if let Ok(c) = read_candidate(&p)
            && !known(identities, &c)
            && !out
                .iter()
                .any(|o| !c.fingerprint.is_empty() && o.fingerprint == c.fingerprint)
        {
            out.push(c);
        }
    }
    out.sort_by(|a, b| a.label.cmp(&b.label));
    out
}

fn ssh_dir() -> Option<PathBuf> {
    directories::UserDirs::new().map(|u| u.home_dir().join(".ssh"))
}

/// What happened with a batch of files.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct Report {
    pub added: Vec<String>,
    pub duplicates: usize,
    pub rejected: Vec<(String, String)>,
    /// Encrypted keys among the added ones.
    pub encrypted: usize,
}

/// Adds the candidates to the keychain. Returns the report and the new identity when exactly
/// one key was added (the caller opens it for username / passphrase).
pub fn add_all(
    identities: &mut Vec<Identity>,
    cands: Vec<Candidate>,
    group: Option<uuid::Uuid>,
) -> (Report, Option<Identity>) {
    let mut rep = Report::default();
    let mut last = None;
    for c in cands {
        if known(identities, &c) {
            rep.duplicates += 1;
            continue;
        }
        let mut ident = Identity::new();
        ident.label = c.label.clone();
        ident.private_key = c.private_key;
        ident.group_id = group;
        if c.encrypted {
            rep.encrypted += 1;
        }
        rep.added.push(c.label);
        last = Some(ident.clone());
        identities.push(ident);
    }
    let single = (rep.added.len() == 1).then_some(last).flatten();
    (rep, single)
}

fn finish(app: &mut App, rep: Report, single: Option<Identity>) {
    if !rep.added.is_empty() {
        app.save_keychain();
    }
    let mut parts: Vec<String> = Vec::new();
    if !rep.added.is_empty() {
        parts.push(t!("{} Schlüssel hinzugefügt", rep.added.len()));
    }
    if rep.duplicates > 0 {
        parts.push(t!("{} schon vorhanden", rep.duplicates));
    }
    if let Some((name, why)) = rep.rejected.first() {
        parts.push(format!("{name}: {why}"));
    }
    if rep.encrypted > 0 {
        parts.push(t!("{} mit Passphrase – bitte eintragen", rep.encrypted));
    }
    let color = if rep.added.is_empty() {
        theme::amber()
    } else {
        theme::accent()
    };
    if !parts.is_empty() {
        app.toast(parts.join(" · "), color);
    }
    if let Some(ident) = single {
        // One key: open it so username (and the passphrase of an encrypted key) can be set.
        app.identity_editor = Some((ident, false, String::new()));
    }
}

/// Handles files dropped onto the window and draws the drop hint. Call while the keys page is
/// shown.
pub fn drop_zone(app: &mut App, ctx: &egui::Context) {
    let (hovering, dropped) = ctx.input(|i| {
        (
            !i.raw.hovered_files.is_empty(),
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect::<Vec<_>>(),
        )
    });
    if hovering {
        let r = ctx.content_rect().shrink(12.0);
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("key-drop-hint"),
        ));
        painter.rect_filled(r, 12.0, Color32::from_black_alpha(150));
        painter.rect_stroke(
            r,
            12.0,
            egui::Stroke::new(2.0, theme::accent()),
            egui::StrokeKind::Inside,
        );
        painter.text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            t!("Schlüsseldatei hier ablegen – sie wird in den Schlüsselbund aufgenommen"),
            egui::FontId::proportional(18.0),
            Color32::WHITE,
        );
    }
    if dropped.is_empty() {
        return;
    }
    let mut cands = Vec::new();
    let mut rep = Report::default();
    for p in dropped.iter().take(MAX_SCAN) {
        if p.is_dir() {
            // A folder (e.g. a dropped ~/.ssh): every key in it.
            cands.extend(scan_dir_all(p));
            continue;
        }
        match read_candidate(p) {
            Ok(c) => cands.push(c),
            Err(why) => rep.rejected.push((label_of(p), why.text())),
        }
    }
    let (added, single) = add_all(&mut app.keychain.identities, cands, None);
    let rep = Report {
        added: added.added,
        duplicates: added.duplicates,
        encrypted: added.encrypted,
        rejected: rep.rejected,
    };
    finish(app, rep, single);
}

/// Every private key in a dropped folder (not recursive), duplicates by fingerprint removed.
fn scan_dir_all(dir: &Path) -> Vec<Candidate> {
    scan_ssh_dir(dir, &[])
}

/// "Import from ~/.ssh": a list of the keys that are not in the keychain yet.
pub fn ssh_dir_button(app: &mut App, ui: &mut egui::Ui) {
    let id = egui::Id::new("key-ssh-scan");
    if ssh_button(ui).clicked() {
        let found = ssh_dir()
            .map(|d| scan_ssh_dir(&d, &app.keychain.identities))
            .unwrap_or_default();
        if found.is_empty() {
            app.toast(
                t!("In ~/.ssh gibt es keine neuen Schlüssel"),
                theme::amber(),
            );
        } else {
            ui.ctx().data_mut(|d| {
                d.insert_temp(id, found.into_iter().map(|c| (c, true)).collect::<Vec<_>>())
            });
        }
    }
}

fn ssh_button(ui: &mut egui::Ui) -> egui::Response {
    theme::btn(
        ui,
        theme::Btn::Normal,
        Some(crate::ui::icons::Icon::Download),
        t!("Aus ~/.ssh"),
    )
    .on_hover_text(t!(
        "Sucht private Schlüssel in ~/.ssh, die noch nicht im Schlüsselbund sind"
    ))
}

/// The selection sheet opened by [`ssh_dir_button`].
pub fn scan_sheet(app: &mut App, ctx: &egui::Context) {
    let id = egui::Id::new("key-ssh-scan");
    let Some(mut list) = ctx.data(|d| d.get_temp::<Vec<(Candidate, bool)>>(id)) else {
        return;
    };
    let (mut add, mut cancel) = (false, false);
    egui::Modal::new(egui::Id::new("key-ssh-scan-modal"))
        .frame(theme::dialog_frame())
        .backdrop_color(theme::backdrop())
        .show(ctx, |ui| {
            ui.set_width(460.0_f32.min(ui.available_width()));
            ui.label(theme::heading(t!("Schlüssel aus ~/.ssh hinzufügen"), 20.0));
            ui.add_space(8.0);
            egui::ScrollArea::vertical()
                .max_height(300.0)
                .show(ui, |ui| {
                    for (c, on) in list.iter_mut() {
                        ui.horizontal(|ui| {
                            ui.checkbox(on, "");
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&c.label).strong());
                                let mut sub = c.kind.clone();
                                if c.encrypted {
                                    sub.push_str(" · ");
                                    sub.push_str(t!("mit Passphrase"));
                                }
                                ui.label(RichText::new(sub).color(theme::text_dim()).small());
                            });
                        });
                    }
                });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let n = list.iter().filter(|(_, on)| *on).count();
                    if ui
                        .add_enabled_ui(n > 0, |ui| {
                            theme::primary_button(ui, &t!("{} hinzufügen", n))
                        })
                        .inner
                        .clicked()
                    {
                        add = true;
                    }
                    if ui.button(t!("Abbrechen")).clicked() {
                        cancel = true;
                    }
                });
            });
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                cancel = true;
            }
        });
    if cancel {
        ctx.data_mut(|d| d.remove::<Vec<(Candidate, bool)>>(id));
    } else if add {
        ctx.data_mut(|d| d.remove::<Vec<(Candidate, bool)>>(id));
        let cands = list
            .into_iter()
            .filter(|(_, on)| *on)
            .map(|(c, _)| c)
            .collect();
        let (rep, single) = add_all(&mut app.keychain.identities, cands, None);
        finish(app, rep, single);
    } else {
        ctx.data_mut(|d| d.insert_temp(id, list));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_key() -> String {
        keygen::generate_ed25519("ops@aurora").unwrap().0
    }

    #[test]
    fn reads_a_private_key_and_names_it_after_the_file() {
        let key = make_key();
        let c = parse_key(Path::new("/home/u/.ssh/id_ed25519"), key.as_bytes()).unwrap();
        assert_eq!(c.label, "id_ed25519");
        assert_eq!(c.kind, "Ed25519");
        assert!(c.fingerprint.starts_with("SHA256:"));
        assert!(!c.encrypted);
        assert!(c.private_key.ends_with('\n'));
        // Windows line endings and a BOM do not matter.
        let crlf = format!("\u{feff}{}", key.replace('\n', "\r\n"));
        assert!(parse_key(Path::new("k"), crlf.as_bytes()).is_ok());
    }

    #[test]
    fn rejects_everything_that_is_not_a_private_key() {
        let p = Path::new("x");
        assert_eq!(
            parse_key(p, b"ssh-ed25519 AAAAC3Nza user@host\n"),
            Err(Reject::PublicOnly)
        );
        assert_eq!(
            parse_key(p, b"PuTTY-User-Key-File-3: ssh-ed25519\n"),
            Err(Reject::Putty)
        );
        assert_eq!(parse_key(p, b"hello world"), Err(Reject::NoKey));
        assert_eq!(parse_key(p, &[0xff, 0xfe, 0x00]), Err(Reject::NotText));
        assert_eq!(parse_key(p, &vec![b'a'; 70_000]), Err(Reject::TooBig));
        // Looks like a key but is garbage.
        assert_eq!(
            parse_key(
                p,
                b"-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA\n-----END OPENSSH PRIVATE KEY-----\n"
            ),
            Err(Reject::NoKey)
        );
    }

    #[test]
    fn duplicates_are_recognised_by_fingerprint() {
        let key = make_key();
        let c = parse_key(Path::new("a"), key.as_bytes()).unwrap();
        let mut ids = Vec::new();
        let (r, single) = add_all(&mut ids, vec![c.clone()], None);
        assert_eq!((r.added.len(), r.duplicates), (1, 0));
        assert!(single.is_some());
        let (r, single) = add_all(&mut ids, vec![c], None);
        assert_eq!((r.added.len(), r.duplicates, ids.len()), (0, 1, 1));
        assert!(single.is_none());
    }

    #[test]
    fn scan_finds_new_private_keys_only() {
        let dir = std::env::temp_dir().join(format!("aurora-keyscan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (k1, pub1) = keygen::generate_ed25519("a").unwrap();
        let (k2, _) = keygen::generate_ed25519("b").unwrap();
        std::fs::write(dir.join("id_a"), &k1).unwrap();
        std::fs::write(dir.join("id_a.pub"), &pub1).unwrap();
        std::fs::write(dir.join("id_b"), &k2).unwrap();
        std::fs::write(dir.join("config"), "Host x\n").unwrap();
        std::fs::write(dir.join("known_hosts"), "h ssh-ed25519 AAAA\n").unwrap();
        let mut have = Identity::new();
        have.private_key = k2;
        let found = scan_ssh_dir(&dir, &[have]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].label, "id_a");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn label_is_cleaned() {
        assert_eq!(label_of(Path::new("/x/prod.pem")), "prod");
        assert_eq!(label_of(Path::new("/x/")), "x");
        assert_eq!(label_of(Path::new("..")), "SSH key");
    }
}
