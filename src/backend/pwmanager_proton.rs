//! Proton Pass adapter (`pass-cli`). ALL command lines live in the `cmd_*` functions at
//! the top, so they are easy to fix if the CLI changes.
//!
//! Verification status (the real CLI is not available where this was written; the
//! official docs site protonpass.github.io/pass-cli could not be fetched):
//! * `pass-cli info` (exit 0 = active session), `pass-cli login` (user does this in a
//!   terminal), `pass-cli item view pass://SHARE/ITEM --output=json`, the item JSON shape
//!   (`item.content.content.{Login|SshKey|…}`, `item.content.note`, `item.state`) and
//!   the `pass://share/item` reference format: confirmed from the chezmoi documentation /
//!   source and the community.general `proton_pass` lookup plugin (third-party sources
//!   that call the CLI).
//! * `pass-cli vault list --output json` -> `{"vaults":[{"name","share_id"}]}` and
//!   `pass-cli item list --share-id S --output json` -> `{"items":[{"id","share_id",
//!   "title","state"}]}`: only from a secondary summary (an issue of a third-party tool),
//!   NOT confirmed against the official docs.
//! * Everything else (parsers are tolerant) is only tested against the fake CLI.

use serde_json::Value;

use super::pwmanager_cli::{Ctx, build_resolved};
use super::pwmanager_types::{PwmError, PwmItem, PwmProvider, Resolved, SourceKind, wipe};

const P: PwmProvider = PwmProvider::ProtonPass;

// ---------------------------------------------------------------- command lines

/// Is there an active session? (exit code 0 = yes)
fn cmd_info() -> Vec<String> {
    vec!["info".into()]
}

/// All vaults, JSON.
fn cmd_vault_list() -> Vec<String> {
    ["vault", "list", "--output", "json"]
        .map(String::from)
        .to_vec()
}

/// Items of one vault, JSON (no secrets in this listing).
fn cmd_item_list(share_id: &str) -> Vec<String> {
    ["item", "list", "--share-id", share_id, "--output", "json"]
        .map(String::from)
        .to_vec()
}

/// One item with its secrets, JSON. `uri` is `pass://SHARE/ITEM` (validated).
fn cmd_item_view(uri: &str) -> Vec<String> {
    vec![
        "item".into(),
        "view".into(),
        uri.to_string(),
        "--output=json".into(),
    ]
}

// ---------------------------------------------------------------- validation

fn id_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '='))
}

/// `pass://SHARE_ID/ITEM_ID` and nothing else (no field part, no names).
pub fn valid_ref(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("pass://") else {
        return false;
    };
    let mut it = rest.split('/');
    matches!((it.next(), it.next(), it.next()), (Some(a), Some(b), None) if id_ok(a) && id_ok(b))
}

// ---------------------------------------------------------------- operations

pub fn status(ctx: &Ctx) -> Result<(), PwmError> {
    let out = ctx.exec(P, cmd_info(), Vec::new(), None)?;
    if out.ok() {
        Ok(())
    } else {
        Err(PwmError::LoggedOut(P))
    }
}

pub fn search(ctx: &Ctx, text: &str) -> Result<Vec<PwmItem>, PwmError> {
    status(ctx)?;
    let out = ctx.exec_ok(P, cmd_vault_list(), Vec::new(), None)?;
    let vaults = parse_vaults(out.stdout.expose())?;
    let mut items = Vec::new();
    for (share, vault_name) in vaults.iter().take(8) {
        if !id_ok(share) {
            continue;
        }
        let out = ctx.exec_ok(P, cmd_item_list(share), Vec::new(), None)?;
        items.extend(parse_items(out.stdout.expose(), share, vault_name)?);
    }
    Ok(filter(items, text))
}

pub fn fetch(
    ctx: &Ctx,
    reference: &str,
    kind: SourceKind,
    use_username: bool,
) -> Result<Resolved, PwmError> {
    if !valid_ref(reference) {
        return Err(PwmError::BadReference);
    }
    status(ctx)?;
    let out = ctx.exec_ok(P, cmd_item_view(reference), Vec::new(), None)?;
    parse_item(out.stdout.expose(), kind, use_username)
}

// ---------------------------------------------------------------- parsing

pub fn filter(items: Vec<PwmItem>, text: &str) -> Vec<PwmItem> {
    let q = text.trim().to_lowercase();
    items
        .into_iter()
        .filter(|i| {
            q.is_empty()
                || i.name.to_lowercase().contains(&q)
                || i.folder.to_lowercase().contains(&q)
                || i.username.to_lowercase().contains(&q)
        })
        .take(50)
        .collect()
}

fn s(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `(share_id, name)` of every vault.
pub fn parse_vaults(json: &str) -> Result<Vec<(String, String)>, PwmError> {
    let v: Value = serde_json::from_str(json).map_err(|_| PwmError::BadOutput)?;
    let list = v
        .get("vaults")
        .and_then(Value::as_array)
        .or_else(|| v.as_array())
        .ok_or(PwmError::BadOutput)?;
    Ok(list
        .iter()
        .map(|x| (s(x, "share_id"), s(x, "name")))
        .filter(|(id, _)| !id.is_empty())
        .collect())
}

pub fn parse_items(json: &str, share: &str, vault: &str) -> Result<Vec<PwmItem>, PwmError> {
    let v: Value = serde_json::from_str(json).map_err(|_| PwmError::BadOutput)?;
    let list = v
        .get("items")
        .and_then(Value::as_array)
        .or_else(|| v.as_array())
        .ok_or(PwmError::BadOutput)?;
    let mut out = Vec::new();
    for it in list {
        let id = s(it, "id");
        if !id_ok(&id) || s(it, "state").eq_ignore_ascii_case("trashed") {
            continue;
        }
        let share = match s(it, "share_id") {
            x if id_ok(&x) => x,
            _ => share.to_string(),
        };
        out.push(PwmItem {
            id: format!("pass://{share}/{id}"),
            name: s(it, "title"),
            username: String::new(),
            folder: vault.to_string(),
        });
    }
    Ok(out)
}

/// Secrets out of `item view --output=json`.
pub fn parse_item(json: &str, kind: SourceKind, use_username: bool) -> Result<Resolved, PwmError> {
    let raw: Value = serde_json::from_str(json).map_err(|_| PwmError::BadOutput)?;
    let item = raw.get("item").unwrap_or(&raw);
    if s(item, "state").eq_ignore_ascii_case("trashed") {
        return Err(PwmError::NotFound);
    }
    let content = item.get("content").ok_or(PwmError::BadOutput)?;
    let note = s(content, "note");
    let (mut user, mut password, mut key) = (String::new(), String::new(), String::new());
    if let Some(types) = content.get("content").and_then(Value::as_object) {
        for (type_key, data) in types {
            match type_key.to_lowercase().as_str() {
                "login" => {
                    user = s(data, "username");
                    if user.is_empty() {
                        user = s(data, "email");
                    }
                    password = s(data, "password");
                }
                "sshkey" => key = s(data, "private_key"),
                _ => {}
            }
        }
    }
    let r = build_resolved(
        kind,
        use_username,
        opt(&user),
        opt(&password),
        opt(&note),
        opt(&key),
    );
    wipe(unsafe { user.as_bytes_mut() });
    wipe(unsafe { password.as_bytes_mut() });
    wipe(unsafe { key.as_bytes_mut() });
    r
}

fn opt(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_validation() {
        assert!(valid_ref("pass://AbC-_1==/Xy9_z=="));
        for bad in [
            "pass://a",
            "pass://a/b/c",
            "pass://a/b/password",
            "-pass://a/b",
            "pass://a b/c",
            "pass://a/--x\n",
            "op://a/b",
            "",
            "pass:///b",
        ] {
            assert!(!valid_ref(bad), "{bad}");
        }
    }

    #[test]
    fn parses_listing_and_item() {
        let v = parse_vaults(r#"{"vaults":[{"name":"Personal","vault_id":"v","share_id":"S1"}]}"#)
            .unwrap();
        assert_eq!(v, vec![("S1".into(), "Personal".into())]);
        let items = parse_items(
            r#"{"items":[{"id":"I1","share_id":"S1","title":"aurora-lab","state":"Active","item_type":"Login"},
                {"id":"I2","share_id":"S1","title":"old","state":"Trashed"}]}"#,
            "S1",
            "Personal",
        )
        .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "pass://S1/I1");
        assert_eq!(filter(items.clone(), "LAB").len(), 1);
        assert_eq!(filter(items, "zzz").len(), 0);

        let json = r#"{"item":{"state":"Active","content":{"title":"x","note":"hello",
            "content":{"Login":{"username":"aurora","email":"","password":"testpass123","urls":[]}}}}}"#;
        let r = parse_item(json, SourceKind::Password, true).unwrap();
        assert_eq!(r.username.as_deref(), Some("aurora"));
        assert_eq!(r.password.as_ref().unwrap().expose(), "testpass123");
        let key = r#"{"item":{"content":{"note":"","content":{"SshKey":{"private_key":"-----BEGIN OPENSSH PRIVATE KEY-----\nAA\n-----END OPENSSH PRIVATE KEY-----"}}}}}"#;
        let r = parse_item(key, SourceKind::PrivateKey, false).unwrap();
        assert!(r.private_key.unwrap().expose().contains("PRIVATE KEY"));
        assert!(r.username.is_none());
        assert_eq!(
            parse_item("nope", SourceKind::Password, true).err(),
            Some(PwmError::BadOutput)
        );
    }
}
