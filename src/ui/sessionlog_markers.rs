//! Markers with notes on a session timeline, and their storage. Recordings keep markers as
//! asciicast v2 `m` events; changes made later (replays, edits, deletions) go to a sidecar
//! file `<name>.cast.notes.json` – the recording itself is never rewritten.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Longest note.
pub const MAX_LABEL: usize = 200;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    /// Seconds from the start of the recording.
    pub t: f64,
    pub label: String,
}

/// Content of the sidecar file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Notes {
    pub markers: Vec<Marker>,
    /// The list is the complete set of markers; markers in the `.cast` are ignored.
    /// Otherwise it only adds to the ones in the recording.
    pub complete: bool,
}

/// A short single-line note.
pub fn clean_label(s: &str) -> String {
    let one: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    one.trim().chars().take(MAX_LABEL).collect()
}

pub fn sidecar_path(cast: &Path) -> PathBuf {
    let mut name = cast.file_name().unwrap_or_default().to_os_string();
    name.push(".notes.json");
    cast.with_file_name(name)
}

pub fn load_notes(cast: &Path) -> Notes {
    std::fs::read_to_string(sidecar_path(cast))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_notes(cast: &Path, notes: &Notes) -> Result<()> {
    let path = sidecar_path(cast);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(notes)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Stores the full marker list of a recording in its sidecar (`complete`).
pub fn save_all(cast: &Path, markers: &[Marker]) -> Result<()> {
    save_notes(
        cast,
        &Notes {
            markers: markers.to_vec(),
            complete: true,
        },
    )
}

/// Markers of the recording merged with the sidecar, sorted by time.
pub fn merge(in_cast: &[(f64, String)], notes: &Notes) -> Vec<Marker> {
    let mut out: Vec<Marker> = if notes.complete {
        Vec::new()
    } else {
        in_cast
            .iter()
            .map(|(t, l)| Marker {
                t: *t,
                label: l.clone(),
            })
            .collect()
    };
    out.extend(notes.markers.iter().cloned());
    out.sort_by(|a, b| a.t.total_cmp(&b.t));
    out
}

/// All markers of a recording file.
pub fn load(cast: &Path, in_cast: &[(f64, String)]) -> Vec<Marker> {
    merge(in_cast, &load_notes(cast))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_adds_or_replaces() {
        let dir = std::env::temp_dir().join(format!("aurora-notes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cast = dir.join("web_2026.cast");
        assert_eq!(
            sidecar_path(&cast).file_name().unwrap(),
            "web_2026.cast.notes.json"
        );
        let in_cast = vec![(5.0, "start".to_string()), (20.0, "done".to_string())];
        // No sidecar: the recording's own markers.
        assert_eq!(load(&cast, &in_cast).len(), 2);
        // Additions are merged in time order.
        save_notes(
            &cast,
            &Notes {
                markers: vec![Marker {
                    t: 10.0,
                    label: "later note".into(),
                }],
                complete: false,
            },
        )
        .unwrap();
        let m = load(&cast, &in_cast);
        assert_eq!(
            m.iter().map(|m| m.label.as_str()).collect::<Vec<_>>(),
            ["start", "later note", "done"]
        );
        // A complete list replaces them (deleting "start", editing "done").
        save_all(
            &cast,
            &[Marker {
                t: 20.0,
                label: "finished".into(),
            }],
        )
        .unwrap();
        let m = load(&cast, &in_cast);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].label, "finished");
        // The recording itself does not exist and was never created.
        assert!(!cast.exists());
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(clean_label("  a\nb\t c "), "a b  c");
    }
}
