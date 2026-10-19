//! Integration tests of the password-manager adapters against the fake CLIs in
//! `tests/pwm_fake/` (shell scripts, so Unix only).

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;

use super::pwmanager_cli::*;
use super::pwmanager_types::*;

const BW: &str = include_str!("../../tests/pwm_fake/bw");
const OP: &str = include_str!("../../tests/pwm_fake/op");
const KP: &str = include_str!("../../tests/pwm_fake/keepassxc-cli");
const PROTON: &str = include_str!("../../tests/pwm_fake/pass-cli");

struct Fake {
    dir: PathBuf,
    ctx: Ctx,
}

impl Fake {
    fn new() -> Fake {
        let dir = crate::store::private_temp_dir("pwm-test").unwrap();
        let mut settings = PwmSettings {
            bw_enabled: true,
            op_enabled: true,
            kp_enabled: true,
            proton_enabled: true,
            ..Default::default()
        };
        for (name, text) in [
            ("bw", BW),
            ("op", OP),
            ("keepassxc-cli", KP),
            ("pass-cli", PROTON),
        ] {
            let p = dir.join(name);
            std::fs::write(&p, text).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).unwrap();
            let s = p.to_string_lossy().to_string();
            match name {
                "bw" => settings.bw_path = s,
                "op" => settings.op_path = s,
                "keepassxc-cli" => settings.kp_path = s,
                _ => settings.proton_path = s,
            }
        }
        std::fs::write(dir.join("db.kdbx"), "x").unwrap();
        settings.kp_db = dir.join("db.kdbx").to_string_lossy().to_string();
        let mut ctx = Ctx::new(settings, Arc::new(Sessions::default()));
        ctx.timeout = std::time::Duration::from_secs(10);
        Fake { dir, ctx }
    }

    fn flag(&self, name: &str, on: bool) {
        let p = self.dir.join(name);
        if on {
            std::fs::write(p, "").unwrap();
        } else {
            let _ = std::fs::remove_file(p);
        }
    }

    fn argv_log(&self) -> String {
        std::fs::read_to_string(self.dir.join("argv.log")).unwrap_or_default()
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn src(p: PwmProvider, item: &str) -> CredentialSource {
    CredentialSource {
        provider: p,
        item: item.into(),
        label: "x".into(),
        ..Default::default()
    }
}

const BW_ID: &str = "11111111-aaaa-bbbb-cccc-000000000001";

#[test]
fn bitwarden_locked_then_unlock_then_resolve() {
    let f = Fake::new();
    let p = PwmProvider::Bitwarden;
    assert_eq!(status(&f.ctx, p), Status::Locked);
    // Locked: search and fetch refuse.
    assert_eq!(search(&f.ctx, p, "aurora").err(), Some(PwmError::Locked(p)));
    assert_eq!(
        fetch(&f.ctx, &src(p, BW_ID)).err(),
        Some(PwmError::Locked(p))
    );
    // Wrong master password.
    assert_eq!(
        unlock(&f.ctx, p, Secret::new("nope")).err(),
        Some(PwmError::WrongPassword)
    );
    unlock(&f.ctx, p, Secret::new("master")).unwrap();
    assert_eq!(status(&f.ctx, p), Status::Ready);
    // Picker.
    let items = search(&f.ctx, p, "AURORA").unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(
        (
            items[0].name.as_str(),
            items[0].username.as_str(),
            items[0].folder.as_str()
        ),
        ("aurora-lab", "aurora", "Lab")
    );
    assert_eq!(search(&f.ctx, p, "").unwrap().len(), 2);
    // Resolve.
    let r = fetch(&f.ctx, &src(p, BW_ID)).unwrap();
    assert_eq!(r.password.as_ref().unwrap().expose(), "testpass123");
    assert_eq!(r.username.as_deref(), Some("aurora"));
    // Unknown item.
    assert_eq!(
        fetch(&f.ctx, &src(p, "11111111-aaaa-bbbb-cccc-0000000000ff")).err(),
        Some(PwmError::NotFound)
    );
    // The master password and session key never appear on argv.
    let log = f.argv_log();
    assert!(!log.contains("master"), "{log}");
    assert!(!log.contains("fake-session-key"), "{log}");
    assert!(log.contains("--passwordenv BW_PASSWORD"));
    // Aurora locking forgets the session.
    f.ctx.sessions.lock_all();
    assert_eq!(status(&f.ctx, p), Status::Locked);
}

#[test]
fn bitwarden_logged_out_and_missing_cli() {
    let f = Fake::new();
    let p = PwmProvider::Bitwarden;
    f.flag("bw_loggedout", true);
    assert_eq!(status(&f.ctx, p), Status::LoggedOut);
    assert_eq!(
        unlock(&f.ctx, p, Secret::new("master")).err(),
        Some(PwmError::LoggedOut(p))
    );
    let mut ctx = f.ctx.clone();
    ctx.settings.bw_path = "/nonexistent/bw".into();
    assert_eq!(status(&ctx, p), Status::NotInstalled);
    assert_eq!(
        fetch(&ctx, &src(p, BW_ID)).err(),
        Some(PwmError::NotInstalled(p))
    );
    ctx.settings.bw_enabled = false;
    assert_eq!(status(&ctx, p), Status::Disabled);
    assert_eq!(
        fetch(&ctx, &src(p, BW_ID)).err(),
        Some(PwmError::Disabled(p))
    );
}

#[test]
fn bitwarden_rejects_injected_reference_without_running() {
    let f = Fake::new();
    let p = PwmProvider::Bitwarden;
    unlock(&f.ctx, p, Secret::new("master")).unwrap();
    let before = f.argv_log();
    for bad in ["--session=x", "id with space", "a\nb", ""] {
        assert_eq!(
            fetch(&f.ctx, &src(p, bad)).err(),
            Some(PwmError::BadReference)
        );
    }
    assert_eq!(f.argv_log(), before);
}

#[test]
fn onepassword_flow() {
    let f = Fake::new();
    let p = PwmProvider::OnePassword;
    assert_eq!(status(&f.ctx, p), Status::Ready);
    let items = search(&f.ctx, p, "lab").unwrap();
    assert_eq!(items.len(), 1);
    let r = fetch(&f.ctx, &src(p, &items[0].id)).unwrap();
    assert_eq!(r.password.unwrap().expose(), "testpass123");
    assert_eq!(r.username.as_deref(), Some("aurora"));
    let r = fetch(&f.ctx, &src(p, "op://Private/aurora-lab/password")).unwrap();
    assert_eq!(r.password.unwrap().expose(), "testpass123");
    assert_eq!(
        fetch(&f.ctx, &src(p, "op://Private/nothing/password")).err(),
        Some(PwmError::NotFound)
    );
    // `--` is passed before the reference.
    assert!(f.argv_log().contains("-- abcd1234efgh5678ijkl9012mn"));
    f.flag("op_locked", true);
    assert_eq!(status(&f.ctx, p), Status::Locked);
    assert_eq!(
        fetch(&f.ctx, &src(p, "abcd1234efgh5678ijkl9012mn")).err(),
        Some(PwmError::Locked(p))
    );
    // Account option is validated.
    let mut ctx = f.ctx.clone();
    ctx.settings.op_account = "--evil".into();
    assert!(matches!(
        status(&ctx, p),
        Status::Error(PwmError::BadConfig)
    ));
}

#[test]
fn keepass_flow() {
    let f = Fake::new();
    let p = PwmProvider::KeePassXc;
    assert_eq!(status(&f.ctx, p), Status::Locked);
    assert_eq!(search(&f.ctx, p, "aurora").err(), Some(PwmError::Locked(p)));
    assert_eq!(
        unlock(&f.ctx, p, Secret::new("wrong")).err(),
        Some(PwmError::WrongPassword)
    );
    unlock(&f.ctx, p, Secret::new("kppass")).unwrap();
    assert_eq!(status(&f.ctx, p), Status::Ready);
    let items = search(&f.ctx, p, "AURORA").unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "/Servers/aurora-lab");
    assert_eq!(items[0].folder, "Servers");
    let r = fetch(&f.ctx, &src(p, &items[0].id)).unwrap();
    assert_eq!(r.password.unwrap().expose(), "testpass123");
    assert_eq!(r.username.as_deref(), Some("aurora"));
    assert_eq!(
        fetch(&f.ctx, &src(p, "/Servers/missing")).err(),
        Some(PwmError::NotFound)
    );
    // Password only via stdin; the entry comes after `--`.
    let log = f.argv_log();
    assert!(!log.contains("kppass"));
    assert!(log.contains("-- "));
    // No username fetched if the host has one.
    let mut s = src(p, "/Servers/aurora-lab");
    s.use_username = false;
    assert!(fetch(&f.ctx, &s).unwrap().username.is_none());
    // Missing database setting.
    let mut ctx = f.ctx.clone();
    ctx.settings.kp_db.clear();
    assert!(matches!(
        status(&ctx, p),
        Status::Error(PwmError::BadConfig)
    ));
}

#[test]
fn proton_flow() {
    let f = Fake::new();
    let p = PwmProvider::ProtonPass;
    assert_eq!(status(&f.ctx, p), Status::Ready);
    let items = search(&f.ctx, p, "aurora").unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "pass://SHARE1/ITEM1");
    assert_eq!(items[0].folder, "Personal");
    let r = fetch(&f.ctx, &src(p, "pass://SHARE1/ITEM1")).unwrap();
    assert_eq!(r.password.unwrap().expose(), "testpass123");
    assert_eq!(r.username.as_deref(), Some("aurora"));
    assert_eq!(
        fetch(&f.ctx, &src(p, "pass://SHARE1/NOPE")).err(),
        Some(PwmError::NotFound)
    );
    assert_eq!(
        fetch(&f.ctx, &src(p, "pass://SHARE1/ITEM1/password")).err(),
        Some(PwmError::BadReference)
    );
    f.flag("proton_loggedout", true);
    assert_eq!(status(&f.ctx, p), Status::LoggedOut);
    assert_eq!(search(&f.ctx, p, "x").err(), Some(PwmError::LoggedOut(p)));
}

#[test]
fn resolve_all_merges_per_host() {
    let f = Fake::new();
    unlock(&f.ctx, PwmProvider::Bitwarden, Secret::new("master")).unwrap();
    let h1 = uuid::Uuid::new_v4();
    let h2 = uuid::Uuid::new_v4();
    let wants = vec![
        (h1, src(PwmProvider::Bitwarden, BW_ID)),
        (
            h2,
            src(PwmProvider::OnePassword, "abcd1234efgh5678ijkl9012mn"),
        ),
    ];
    let r = resolve_all(&f.ctx, &wants).unwrap();
    assert_eq!(r.len(), 2);
    assert_eq!(r[0].0, h1);
    assert!(format!("{:?}", r[0].1).contains("***"));
    assert!(!format!("{:?}", r).contains("testpass123"));
    // One failing source fails the whole set.
    let bad = vec![(
        h1,
        src(
            PwmProvider::Bitwarden,
            "11111111-aaaa-bbbb-cccc-0000000000ff",
        ),
    )];
    assert_eq!(resolve_all(&f.ctx, &bad).err(), Some(PwmError::NotFound));
}
