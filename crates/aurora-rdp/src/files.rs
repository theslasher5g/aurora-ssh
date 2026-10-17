//! File transfer over the RDP clipboard (`cliprdr` file lists).
//!
//! * Local → remote: [`build_file_list`] walks the dropped paths into a flat list with
//!   names relative to the dropped item; [`read_range`] serves `FileContentsRequest`s.
//! * Remote → local: [`Download`] turns the host's file list into safe local paths below
//!   one target folder and writes the chunks as they arrive.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use ironrdp::cliprdr::pdu::{ClipboardFileAttributes, FileDescriptor};

/// Longest wire name (`relative\path\name`) in UTF-16 units, see `initiate_file_copy`.
const MAX_WIRE_NAME: usize = 259;
/// Same bound as `PackedFileList` decoding on the other side.
const MAX_ENTRIES: usize = 100_000;
/// Upper bound for one range answer, whatever the server asks for. `cbRequested` is a
/// maximum (MS-RDPECLIP 2.2.5.3): the host asks again for the rest.
pub const MAX_CHUNK: u32 = 4 * 1024 * 1024;

/// One file or folder offered to the host.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalEntry {
    /// Where to read the data (the resolved target for symlinks).
    pub path: PathBuf,
    /// Components relative to the dropped item's parent, e.g. `["photos", "a.jpg"]`.
    pub rel: Vec<String>,
    pub is_dir: bool,
    pub size: u64,
    /// Windows FILETIME (100 ns since 1601).
    pub mtime: Option<u64>,
}

impl LocalEntry {
    #[cfg(test)]
    pub fn wire_name(&self) -> String {
        self.rel.join("\\")
    }

    pub fn descriptor(&self) -> FileDescriptor {
        let (name, dirs) = self.rel.split_last().expect("non-empty rel");
        let mut d = FileDescriptor::new(name.clone())
            .with_attributes(if self.is_dir {
                ClipboardFileAttributes::DIRECTORY
            } else {
                ClipboardFileAttributes::ARCHIVE
            })
            .with_file_size(if self.is_dir { 0 } else { self.size });
        if let Some(t) = self.mtime {
            d = d.with_last_write_time(t);
        }
        if !dirs.is_empty() {
            d = d.with_relative_path(dirs.join("\\"));
        }
        d
    }
}

/// Result of [`build_file_list`]: the entries plus everything that was left out.
#[derive(Debug, Default)]
pub struct FileList {
    pub entries: Vec<LocalEntry>,
    /// Relative names (or paths) that were refused, for the error message.
    pub skipped: Vec<String>,
}

impl FileList {
    pub fn total_bytes(&self) -> u64 {
        self.entries
            .iter()
            .filter(|e| !e.is_dir)
            .map(|e| e.size)
            .sum()
    }

    pub fn file_count(&self) -> usize {
        self.entries.iter().filter(|e| !e.is_dir).count()
    }
}

/// Whether `name` may be sent as one path component: no separators, no `..`, nothing
/// Windows cannot store (reserved characters and device names, trailing dot or space).
pub fn valid_component(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.ends_with('.')
        && !name.ends_with(' ')
        && !name.chars().any(|c| {
            c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
        && !ironrdp::cliprdr::is_windows_device_name(name)
}

/// Unix time → Windows FILETIME.
fn filetime(t: std::time::SystemTime) -> Option<u64> {
    let d = t.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some((d.as_secs() + 11_644_473_600) * 10_000_000 + u64::from(d.subsec_nanos() / 100))
}

/// Builds the file list for the dropped `paths`. Folders are walked recursively; names
/// are relative to the dropped item. Refused: names that are not valid UTF-8 or not
/// storable on Windows, symlinks that point outside the dropped item, symlinked folders
/// (loops), special files and names longer than the RDP limit.
pub fn build_file_list(paths: &[PathBuf]) -> FileList {
    let mut list = FileList::default();
    for p in paths {
        let shown = p.display().to_string();
        let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
            list.skipped.push(shown);
            continue;
        };
        if !valid_component(name) {
            list.skipped.push(name.to_string());
            continue;
        }
        // The dropped item itself may be a link: the user picked it, so follow it.
        let (Ok(root), Ok(meta)) = (std::fs::canonicalize(p), std::fs::metadata(p)) else {
            list.skipped.push(name.to_string());
            continue;
        };
        let rel = vec![name.to_string()];
        if meta.is_dir() {
            push(&mut list, &root, rel.clone(), true, 0, meta.modified().ok());
            walk(&mut list, &root, &root, &rel);
        } else if meta.is_file() {
            push(
                &mut list,
                &root,
                rel,
                false,
                meta.len(),
                meta.modified().ok(),
            );
        } else {
            list.skipped.push(name.to_string());
        }
    }
    list
}

fn push(
    list: &mut FileList,
    path: &Path,
    rel: Vec<String>,
    is_dir: bool,
    size: u64,
    mtime: Option<std::time::SystemTime>,
) {
    let wire = rel.join("\\");
    if wire.encode_utf16().count() > MAX_WIRE_NAME || list.entries.len() >= MAX_ENTRIES {
        list.skipped.push(rel.join("/"));
        return;
    }
    list.entries.push(LocalEntry {
        path: path.to_path_buf(),
        rel,
        is_dir,
        size,
        mtime: mtime.and_then(filetime),
    });
}

fn walk(list: &mut FileList, root: &Path, dir: &Path, rel: &[String]) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        list.skipped.push(rel.join("/"));
        return;
    };
    let mut children: Vec<_> = rd.flatten().collect();
    children.sort_by_key(|e| e.file_name());
    for e in children {
        let os_name = e.file_name();
        let Some(name) = os_name.to_str() else {
            list.skipped
                .push(format!("{}/{}", rel.join("/"), os_name.to_string_lossy()));
            continue;
        };
        let mut child_rel = rel.to_vec();
        child_rel.push(name.to_string());
        let shown = child_rel.join("/");
        if !valid_component(name) {
            list.skipped.push(shown);
            continue;
        }
        let Ok(lmeta) = std::fs::symlink_metadata(e.path()) else {
            list.skipped.push(shown);
            continue;
        };
        if lmeta.file_type().is_symlink() {
            // Only links to files inside the dropped tree; folders could loop.
            match (std::fs::canonicalize(e.path()), std::fs::metadata(e.path())) {
                (Ok(target), Ok(meta)) if target.starts_with(root) && meta.is_file() => {
                    push(
                        list,
                        &target,
                        child_rel,
                        false,
                        meta.len(),
                        meta.modified().ok(),
                    );
                }
                _ => list.skipped.push(shown),
            }
        } else if lmeta.is_dir() {
            push(
                list,
                &e.path(),
                child_rel.clone(),
                true,
                0,
                lmeta.modified().ok(),
            );
            walk(list, root, &e.path(), &child_rel);
        } else if lmeta.is_file() {
            push(
                list,
                &e.path(),
                child_rel,
                false,
                lmeta.len(),
                lmeta.modified().ok(),
            );
        } else {
            list.skipped.push(shown);
        }
    }
}

/// Reads up to `len` bytes at `pos`; empty at or after the end of the file.
pub fn read_range(file: &mut File, pos: u64, len: u32) -> std::io::Result<Vec<u8>> {
    let len = len.min(MAX_CHUNK);
    file.seek(SeekFrom::Start(pos))?;
    let mut buf = Vec::with_capacity(len as usize);
    file.take(u64::from(len)).read_to_end(&mut buf)?;
    Ok(buf)
}

// ---------------------------------------------------------------- remote → local

/// Makes one remote path component safe to create locally: reserved characters become
/// `_`, trailing dots and spaces go, device names get a `_` prefix, `.`/`..`/empty
/// become `unnamed`.
pub fn safe_component(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    while s.ends_with('.') || s.ends_with(' ') {
        s.pop();
    }
    let s = s.trim_start().to_string();
    if s.is_empty() || s == "." || s == ".." {
        return "unnamed".into();
    }
    if ironrdp::cliprdr::is_windows_device_name(&s) {
        return format!("_{s}");
    }
    s
}

/// Splits a remote `relative\path` + `name` into safe components (`..`, empty parts and
/// absolute prefixes are dropped).
pub fn safe_components(relative_path: Option<&str>, name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in relative_path
        .unwrap_or_default()
        .split(['\\', '/'])
        .chain(name.split(['\\', '/']))
    {
        let raw = raw.trim_end_matches('\0');
        if raw.is_empty() || raw == "." || raw == ".." {
            continue;
        }
        // Drive letters ("C:") would only reveal where the files came from.
        if out.is_empty() && raw.len() == 2 && raw.ends_with(':') {
            continue;
        }
        out.push(safe_component(raw));
    }
    if out.is_empty() {
        out.push("unnamed".into());
    }
    out
}

/// `dir/name`, or `dir/name (1)`, `dir/name (2)` … if that exists (extension kept).
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if std::fs::symlink_metadata(&first).is_err() {
        return first;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    for n in 1.. {
        let p = dir.join(format!("{stem} ({n}){ext}"));
        if std::fs::symlink_metadata(&p).is_err() {
            return p;
        }
    }
    unreachable!()
}

/// One entry of the host's file list, mapped to a local path.
#[derive(Debug, Clone)]
pub struct RemoteItem {
    /// Index in the host's list (`lindex`).
    pub index: usize,
    pub target: PathBuf,
    pub is_dir: bool,
    pub size: Option<u64>,
}

/// Maps the host's list below `root`. The first component of every entry is made unique
/// in `root` (never overwrite), deeper ones live inside that fresh folder.
pub fn plan_download(
    root: &Path,
    files: &[(Option<String>, String, bool, Option<u64>)],
) -> Vec<RemoteItem> {
    let mut top: HashMap<String, PathBuf> = HashMap::new();
    // Lower-cased, since Windows and macOS folders ignore case.
    let mut taken: Vec<String> = Vec::new();
    let lower = |p: &Path| p.to_string_lossy().to_lowercase();
    let mut out = Vec::new();
    for (index, (rel, name, is_dir, size)) in files.iter().enumerate() {
        let comps = safe_components(rel.as_deref(), name);
        let first = &comps[0];
        let key = first.to_lowercase();
        let base = match top.get(&key) {
            Some(p) if comps.len() > 1 || *is_dir => p.clone(),
            _ => {
                let mut p = unique_path(root, first);
                // Also unique against what this download already planned.
                let mut n = 1;
                while taken.contains(&lower(&p)) {
                    let (stem, ext) = match first.rfind('.') {
                        Some(i) if i > 0 => (&first[..i], &first[i..]),
                        _ => (first.as_str(), ""),
                    };
                    p = unique_path(root, &format!("{stem} ({n}){ext}"));
                    n += 1;
                }
                taken.push(lower(&p));
                top.insert(key, p.clone());
                p
            }
        };
        let target = comps[1..].iter().fold(base, |p, c| p.join(c));
        out.push(RemoteItem {
            index,
            target,
            is_dir: *is_dir,
            size: *size,
        });
    }
    out
}

/// Download state: one file at a time, one range request in flight.
pub struct Download {
    pub root: PathBuf,
    pub items: Vec<RemoteItem>,
    pub next: usize,
    pub current: Option<Current>,
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub files_written: usize,
    pub clip_data_id: Option<u32>,
}

pub struct Current {
    pub item: RemoteItem,
    pub file: Option<File>,
    pub pos: u64,
    pub size: Option<u64>,
}

/// What the downloader wants next.
#[derive(Debug, PartialEq)]
pub enum Step {
    /// Ask for the size of entry `index`.
    Size {
        index: usize,
    },
    /// Ask for `len` bytes of entry `index` at `pos`.
    Range {
        index: usize,
        pos: u64,
        len: u32,
    },
    Finished,
}

/// Range size asked for per request.
pub const DOWNLOAD_CHUNK: u64 = 1024 * 1024;

impl Download {
    pub fn new(root: PathBuf, items: Vec<RemoteItem>, clip_data_id: Option<u32>) -> Self {
        let total_bytes = items
            .iter()
            .filter(|i| !i.is_dir)
            .filter_map(|i| i.size)
            .sum();
        Self {
            root,
            items,
            next: 0,
            current: None,
            done_bytes: 0,
            total_bytes,
            files_written: 0,
            clip_data_id,
        }
    }

    /// Creates folders and empty files until a request is needed.
    pub fn advance(&mut self) -> std::io::Result<Step> {
        loop {
            if let Some(cur) = &mut self.current {
                let Some(size) = cur.size else {
                    return Ok(Step::Size {
                        index: cur.item.index,
                    });
                };
                if cur.file.is_none() {
                    if let Some(parent) = cur.item.target.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    cur.file = Some(
                        std::fs::OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .open(&cur.item.target)?,
                    );
                }
                if cur.pos < size {
                    let len = (size - cur.pos).min(DOWNLOAD_CHUNK) as u32;
                    return Ok(Step::Range {
                        index: cur.item.index,
                        pos: cur.pos,
                        len,
                    });
                }
                // `File` is unbuffered: the data is with the OS already. No fsync per
                // file, it would stall big folders of small files.
                cur.file = None;
                self.files_written += 1;
                self.current = None;
            }
            let Some(item) = self.items.get(self.next).cloned() else {
                return Ok(Step::Finished);
            };
            self.next += 1;
            if item.is_dir {
                std::fs::create_dir_all(&item.target)?;
                continue;
            }
            self.current = Some(Current {
                size: item.size,
                item,
                file: None,
                pos: 0,
            });
        }
    }

    /// Size answer for the current file.
    pub fn got_size(&mut self, size: u64) {
        if let Some(cur) = &mut self.current {
            if cur.size.is_none() {
                self.total_bytes += size;
            }
            cur.size = Some(size);
        }
    }

    /// Range answer for the current file.
    pub fn got_data(&mut self, data: &[u8]) -> std::io::Result<()> {
        let Some(cur) = &mut self.current else {
            return Ok(());
        };
        if data.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "empty answer",
            ));
        }
        let size = cur.size.unwrap_or(0);
        let take = (data.len() as u64).min(size.saturating_sub(cur.pos)) as usize;
        if let Some(f) = &mut cur.file {
            f.write_all(&data[..take])?;
        }
        cur.pos += take as u64;
        self.done_bytes += take as u64;
        Ok(())
    }

    /// Removes the half-written file after an error.
    pub fn abort(&mut self) {
        if let Some(cur) = self.current.take()
            && cur.file.is_some()
        {
            drop(cur.file);
            let _ = std::fs::remove_file(&cur.item.target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("aurora-rdp-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn components_are_validated() {
        assert!(valid_component("Übersicht März.txt"));
        assert!(valid_component(".bashrc"));
        for bad in [
            "", ".", "..", "a/b", "a\\b", "C:", "x?", "CON", "nul.txt", "end.", "end ",
        ] {
            assert!(!valid_component(bad), "{bad:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn file_list_walks_folders_and_refuses_links_out() {
        let d = tmp("list");
        let src = d.join("Fotos");
        std::fs::create_dir_all(src.join("Ärger/tief")).unwrap();
        std::fs::write(src.join("a.txt"), b"hello").unwrap();
        std::fs::write(src.join("Ärger/tief/b.bin"), vec![1u8; 300]).unwrap();
        std::fs::write(d.join("outside.txt"), b"secret").unwrap();
        std::os::unix::fs::symlink(d.join("outside.txt"), src.join("leak.txt")).unwrap();
        std::os::unix::fs::symlink(src.join("a.txt"), src.join("inner.txt")).unwrap();
        std::os::unix::fs::symlink(&src, src.join("loop")).unwrap();
        std::fs::write(src.join("bad?.txt"), b"").unwrap();
        let single = d.join("einzeln.txt");
        std::fs::write(&single, b"12").unwrap();

        let list = build_file_list(&[src.clone(), single.clone()]);
        let names: Vec<String> = list.entries.iter().map(LocalEntry::wire_name).collect();
        assert_eq!(
            names,
            [
                "Fotos",
                "Fotos\\a.txt",
                "Fotos\\inner.txt",
                "Fotos\\Ärger",
                "Fotos\\Ärger\\tief",
                "Fotos\\Ärger\\tief\\b.bin",
                "einzeln.txt",
            ]
        );
        let mut skipped = list.skipped.clone();
        skipped.sort();
        assert_eq!(skipped, ["Fotos/bad?.txt", "Fotos/leak.txt", "Fotos/loop"]);
        assert_eq!(list.total_bytes(), 5 + 5 + 300 + 2);
        assert_eq!(list.file_count(), 4);
        // The in-tree link is read from its target.
        assert_eq!(
            list.entries[2].path,
            std::fs::canonicalize(src.join("a.txt")).unwrap()
        );

        let desc = list.entries[5].descriptor();
        assert_eq!(desc.name, "b.bin");
        assert_eq!(desc.relative_path.as_deref(), Some("Fotos\\Ärger\\tief"));
        assert_eq!(desc.file_size, Some(300));
        assert!(desc.last_write_time.is_some());
        assert_eq!(
            list.entries[0].descriptor().attributes,
            Some(ClipboardFileAttributes::DIRECTORY)
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn missing_and_dot_paths_are_refused() {
        let list = build_file_list(&[PathBuf::from("/definitely/not/here"), PathBuf::from("..")]);
        assert!(list.entries.is_empty());
        assert_eq!(list.skipped.len(), 2);
    }

    #[test]
    fn range_reads_at_and_after_eof() {
        let d = tmp("range");
        let p = d.join("f");
        std::fs::write(&p, b"0123456789").unwrap();
        let mut f = File::open(&p).unwrap();
        assert_eq!(read_range(&mut f, 0, 4).unwrap(), b"0123");
        assert_eq!(read_range(&mut f, 8, 100).unwrap(), b"89");
        assert_eq!(read_range(&mut f, 10, 5).unwrap(), b"");
        assert_eq!(read_range(&mut f, 1 << 40, 5).unwrap(), b"");
        // Huge requests are capped instead of allocating gigabytes.
        assert_eq!(read_range(&mut f, 0, u32::MAX).unwrap().len(), 10);
        let big = d.join("big");
        std::fs::write(&big, vec![7u8; MAX_CHUNK as usize + 10]).unwrap();
        let mut f = File::open(&big).unwrap();
        assert_eq!(
            read_range(&mut f, 0, u32::MAX).unwrap().len(),
            MAX_CHUNK as usize
        );
        assert_eq!(
            read_range(&mut f, u64::from(MAX_CHUNK), u32::MAX)
                .unwrap()
                .len(),
            10
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn remote_names_are_sanitized() {
        assert_eq!(safe_component("ok.txt"), "ok.txt");
        assert_eq!(safe_component("a:b*c?.txt"), "a_b_c_.txt");
        assert_eq!(safe_component("CON"), "_CON");
        assert_eq!(safe_component("lpt1.log"), "_lpt1.log");
        assert_eq!(safe_component(".."), "unnamed");
        assert_eq!(safe_component("name. . "), "name");
        assert_eq!(safe_component("tab\there"), "tab_here");
        assert_eq!(
            safe_components(Some("..\\..\\etc"), "passwd"),
            ["etc", "passwd"]
        );
        assert_eq!(
            safe_components(Some("C:\\Users\\x"), "f.txt"),
            ["Users", "x", "f.txt"]
        );
        assert_eq!(safe_components(None, "/abs/path.txt"), ["abs", "path.txt"]);
        assert_eq!(safe_components(None, ""), ["unnamed"]);
    }

    #[test]
    fn download_never_overwrites() {
        let d = tmp("dl");
        std::fs::write(d.join("report.pdf"), b"old").unwrap();
        std::fs::create_dir_all(d.join("Ordner")).unwrap();
        let files = vec![
            (None, "report.pdf".to_string(), false, Some(3)),
            (None, "Ordner".to_string(), true, Some(0)),
            (
                Some("Ordner".to_string()),
                "x.txt".to_string(),
                false,
                Some(0),
            ),
            (None, "neu.txt".to_string(), false, None),
        ];
        let plan = plan_download(&d, &files);
        assert_eq!(plan[0].target, d.join("report (1).pdf"));
        assert_eq!(plan[1].target, d.join("Ordner (1)"));
        assert_eq!(plan[2].target, d.join("Ordner (1)").join("x.txt"));
        assert_eq!(plan[3].target, d.join("neu.txt"));

        let mut dl = Download::new(d.clone(), plan, None);
        assert_eq!(dl.total_bytes, 3);
        assert_eq!(
            dl.advance().unwrap(),
            Step::Range {
                index: 0,
                pos: 0,
                len: 3
            }
        );
        dl.got_data(b"new!").unwrap(); // longer than announced: cut
        assert_eq!(dl.advance().unwrap(), Step::Size { index: 3 });
        dl.got_size(2);
        assert_eq!(
            dl.advance().unwrap(),
            Step::Range {
                index: 3,
                pos: 0,
                len: 2
            }
        );
        assert!(dl.got_data(b"").is_err());
        dl.got_data(b"hi").unwrap();
        assert_eq!(dl.advance().unwrap(), Step::Finished);
        assert_eq!(std::fs::read(d.join("report.pdf")).unwrap(), b"old");
        assert_eq!(std::fs::read(d.join("report (1).pdf")).unwrap(), b"new");
        assert_eq!(std::fs::read(d.join("Ordner (1)/x.txt")).unwrap(), b"");
        assert_eq!(std::fs::read(d.join("neu.txt")).unwrap(), b"hi");
        assert_eq!(dl.files_written, 3);
        assert_eq!(dl.done_bytes, 5);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn duplicate_names_in_one_list_stay_apart() {
        let d = tmp("dup");
        let files = vec![
            (None, "a.txt".to_string(), false, Some(0)),
            (None, "A.txt".to_string(), false, Some(0)),
        ];
        let plan = plan_download(&d, &files);
        assert_eq!(plan[0].target, d.join("a.txt"));
        assert_ne!(plan[1].target, plan[0].target);
        let _ = std::fs::remove_dir_all(&d);
    }
}
