//! RDPDR drive redirection: shows ONE local folder as a drive in the remote session.
//!
//! Security model (see SECURITY.md, "RDP-Laufwerksfreigabe"): the remote server is not
//! trusted. Every path it sends is parsed by [`parse_remote_path`] (no `..`, no UNC or
//! drive-letter paths, no device names, no trailing dots or spaces, no stream syntax) and
//! then resolved component by component by [`Share::resolve`] (no symlinks anywhere, the
//! canonical result has to stay below the canonical share root, Windows-style
//! case-insensitive lookup). The share is read-only unless the user turned writing on, and
//! nothing the server sends is ever executed (new files get mode 0644).

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use ironrdp::pdu::PduResult;
use ironrdp::rdpdr::Rdpdr;
use ironrdp::rdpdr::backend::RdpdrBackend;
use ironrdp::rdpdr::pdu::RdpdrPdu;
use ironrdp::rdpdr::pdu::efs::{
    Boolean, Characteristics, ClientDriveQueryDirectoryResponse,
    ClientDriveQueryInformationResponse, ClientDriveQueryVolumeInformationResponse,
    ClientDriveSetInformationResponse, CreateDisposition, CreateOptions, DeviceCloseResponse,
    DeviceControlRequest, DeviceControlResponse, DeviceCreateRequest, DeviceCreateResponse,
    DeviceIoRequest, DeviceIoResponse, DeviceReadResponse, DeviceWriteResponse,
    FileAttributeTagInformation, FileAttributes, FileBasicInformation,
    FileBothDirectoryInformation, FileDirectoryInformation, FileFsAttributeInformation,
    FileFsDeviceInformation, FileFsFullSizeInformation, FileFsSizeInformation,
    FileFsVolumeInformation, FileFullDirectoryInformation, FileInformationClass,
    FileInformationClassLevel, FileNamesInformation, FileStandardInformation, FileSystemAttributes,
    FileSystemInformationClass, FileSystemInformationClassLevel, Information, NtStatus,
    ServerDeviceAnnounceResponse, ServerDriveIoRequest, ServerDriveSetInformationRequest,
};
use ironrdp::rdpdr::pdu::esc::{ScardCall, ScardIoCtlCode};
use ironrdp::svc::SvcMessage;

/// Largest chunk returned for one read request.
pub const MAX_READ: usize = 1 << 20;
/// Largest write request accepted.
pub const MAX_WRITE: usize = 4 << 20;
/// Files bigger than this cannot be created or extended through the share.
pub const MAX_FILE_SIZE: u64 = 4 << 30;
/// Total bytes the server may write through the share during one session.
pub const MAX_SESSION_WRITE: u64 = 16 << 30;
/// Open handles at the same time.
pub const MAX_HANDLES: usize = 256;
/// Entries of one directory listing.
pub const MAX_DIR_ENTRIES: usize = 50_000;
/// Path components and depth.
pub const MAX_COMPONENT_UTF16: usize = 255;
pub const MAX_DEPTH: usize = 64;
pub const MAX_PATH_CHARS: usize = 32_000;
/// Drive names shown in Explorer.
pub const MAX_NAME_CHARS: usize = 24;

/// NTSTATUS values used by this module.
mod nt {
    pub const SUCCESS: u32 = 0;
    pub const NO_MORE_FILES: u32 = 0x8000_0006;
    pub const UNSUCCESSFUL: u32 = 0xC000_0001;
    pub const INVALID_HANDLE: u32 = 0xC000_0008;
    pub const NO_SUCH_DEVICE: u32 = 0xC000_000E;
    pub const INVALID_PARAMETER: u32 = 0xC000_000D;
    pub const ACCESS_DENIED: u32 = 0xC000_0022;
    pub const OBJECT_NAME_INVALID: u32 = 0xC000_0033;
    /// STATUS_NO_SUCH_FILE: what FreeRDP answers; xrdp does not understand the more precise codes.
    pub const OBJECT_NAME_NOT_FOUND: u32 = 0xC000_000F;
    pub const OBJECT_NAME_COLLISION: u32 = 0xC000_0035;
    pub const OBJECT_PATH_NOT_FOUND: u32 = 0xC000_000F;
    pub const DISK_FULL: u32 = 0xC000_007F;
    pub const FILE_IS_A_DIRECTORY: u32 = 0xC000_00BA;
    pub const NOT_SUPPORTED: u32 = 0xC000_00BB;
    pub const DIRECTORY_NOT_EMPTY: u32 = 0xC000_0101;
    pub const NOT_A_DIRECTORY: u32 = 0xC000_0103;
    pub const TOO_MANY_OPENED_FILES: u32 = 0xC000_011F;
}

// ------------------------------------------------------------------ errors

/// Why a request was refused. [`Fail::security`] marks refusals worth showing to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fail {
    Traversal,
    Unc,
    InvalidChar,
    Reserved,
    TrailingDotSpace,
    TooLong,
    TooDeep,
    /// A symlink or reparse point (or a path that leaves the share).
    Escape,
    /// Two entries differ only by case; Windows could not tell them apart.
    Ambiguous,
    NotFound,
    PathNotFound,
    NotADirectory,
    IsADirectory,
    ReadOnly,
    Exists,
    NotEmpty,
    TooBig,
    Busy,
    Full,
    BadHandle,
    BadParam,
    NotSupported,
    Io,
}

impl Fail {
    pub fn status(self) -> u32 {
        match self {
            Fail::Traversal | Fail::Unc | Fail::Escape | Fail::ReadOnly | Fail::Ambiguous => {
                nt::ACCESS_DENIED
            }
            Fail::InvalidChar
            | Fail::Reserved
            | Fail::TrailingDotSpace
            | Fail::TooLong
            | Fail::TooDeep => nt::OBJECT_NAME_INVALID,
            Fail::NotFound => nt::OBJECT_NAME_NOT_FOUND,
            Fail::PathNotFound => nt::OBJECT_PATH_NOT_FOUND,
            Fail::NotADirectory => nt::NOT_A_DIRECTORY,
            Fail::IsADirectory => nt::FILE_IS_A_DIRECTORY,
            Fail::Exists => nt::OBJECT_NAME_COLLISION,
            Fail::NotEmpty => nt::DIRECTORY_NOT_EMPTY,
            Fail::TooBig | Fail::Full => nt::DISK_FULL,
            Fail::Busy => nt::TOO_MANY_OPENED_FILES,
            Fail::BadHandle => nt::INVALID_HANDLE,
            Fail::BadParam => nt::INVALID_PARAMETER,
            Fail::NotSupported => nt::NOT_SUPPORTED,
            Fail::Io => nt::UNSUCCESSFUL,
        }
    }

    /// Refusals that mean the server tried something it should not (as opposed to ordinary
    /// "file not found" answers).
    pub fn security(self) -> bool {
        matches!(
            self,
            Fail::Traversal
                | Fail::Unc
                | Fail::Escape
                | Fail::ReadOnly
                | Fail::Reserved
                | Fail::InvalidChar
                | Fail::TrailingDotSpace
                | Fail::Ambiguous
        )
    }
}

fn io_fail(e: &std::io::Error) -> Fail {
    use std::io::ErrorKind as K;
    match e.kind() {
        K::NotFound => Fail::NotFound,
        K::PermissionDenied => Fail::ReadOnly,
        K::AlreadyExists => Fail::Exists,
        K::DirectoryNotEmpty => Fail::NotEmpty,
        K::StorageFull | K::QuotaExceeded | K::FileTooLarge => Fail::Full,
        K::NotADirectory => Fail::NotADirectory,
        K::IsADirectory => Fail::IsADirectory,
        _ => Fail::Io,
    }
}

// ------------------------------------------------------------- path parsing

/// Device names Windows resolves anywhere in a path, whatever the extension.
pub fn is_reserved_device_name(component: &str) -> bool {
    // `CON.txt`, `nul.tar.gz`, `COM1 .x`: the part before the first dot, without trailing
    // spaces, decides.
    let stem = component
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end_matches(' ');
    let up = stem.to_uppercase();
    match up.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$" => return true,
        _ => {}
    }
    // COM0-9 / LPT0-9 including the superscript digits Windows also treats as numbers.
    for prefix in ["COM", "LPT"] {
        if let Some(rest) = up.strip_prefix(prefix) {
            let mut chars = rest.chars();
            if let (Some(d), None) = (chars.next(), chars.next())
                && (d.is_ascii_digit() || matches!(d, '\u{b9}' | '\u{b2}' | '\u{b3}'))
            {
                return true;
            }
        }
    }
    false
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Checks one file name as the server sends it (no separators in it).
pub fn check_component(c: &str) -> Result<(), Fail> {
    if c.is_empty() {
        return Err(Fail::InvalidChar);
    }
    if c == ".." {
        return Err(Fail::Traversal);
    }
    if c == "." {
        return Err(Fail::InvalidChar);
    }
    if utf16_len(c) > MAX_COMPONENT_UTF16 {
        return Err(Fail::TooLong);
    }
    for ch in c.chars() {
        // Control characters, wildcards, stream and drive syntax, and both separators.
        if (ch as u32) < 0x20
            || matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '/' | '\\')
            || ch == '\u{7f}'
        {
            return Err(Fail::InvalidChar);
        }
    }
    // Windows silently strips trailing dots and spaces: `..` + spaces would become `..`.
    if c.ends_with('.') || c.ends_with(' ') {
        return Err(Fail::TrailingDotSpace);
    }
    if is_reserved_device_name(c) {
        return Err(Fail::Reserved);
    }
    Ok(())
}

/// Splits the path of a server request (`\dir\file.txt`, `\`, ``) into checked components.
/// The empty list is the share root.
pub fn parse_remote_path(raw: &str) -> Result<Vec<String>, Fail> {
    if raw.contains('\0') {
        return Err(Fail::InvalidChar);
    }
    if raw.chars().count() > MAX_PATH_CHARS {
        return Err(Fail::TooLong);
    }
    let sep = |c: char| c == '\\' || c == '/';
    // `\\server\share`, `\\?\C:\x`, `//host/x`: never.
    let mut it = raw.chars();
    if let (Some(a), Some(b)) = (it.next(), it.next())
        && sep(a)
        && sep(b)
    {
        return Err(Fail::Unc);
    }
    // Windows servers send backslashes; xrdp sends slashes. Both are separators, and no
    // component can contain either, so the meaning is the same everywhere.
    // One leading and one trailing separator are normal ("\dir\").
    let s = raw.strip_prefix(sep).unwrap_or(raw);
    let s = s.strip_suffix(sep).unwrap_or(s);
    if s.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for c in s.split(sep) {
        check_component(c)?;
        out.push(c.to_string());
        if out.len() > MAX_DEPTH {
            return Err(Fail::TooDeep);
        }
    }
    Ok(out)
}

/// A drive name that is safe to announce (shown as "<name> on <computer>").
pub fn sanitize_drive_name(name: &str, fallback: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'))
        .take(MAX_NAME_CHARS)
        .collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() {
        fallback.to_string()
    } else {
        cleaned
    }
}

// --------------------------------------------------------------------- share

#[derive(Debug)]
pub enum Resolved {
    Existing(PathBuf),
    /// The last component does not exist (yet); `parent` exists and is inside the share.
    Missing {
        parent: PathBuf,
        name: String,
    },
}

/// The confinement: a canonical root folder plus the write permission.
#[derive(Debug, Clone)]
pub struct Share {
    root: PathBuf,
    writable: bool,
}

impl Share {
    pub fn new(root: &Path, writable: bool) -> Result<Self, String> {
        let canon = fs::canonicalize(root).map_err(|e| format!("{}: {e}", root.display()))?;
        if !canon.is_dir() {
            return Err(format!("{}: kein Ordner", root.display()));
        }
        // Refuse the top of a file system: sharing "/" or "C:\" is never what anybody wants.
        if canon.parent().is_none() {
            return Err("ganzes Laufwerk".into());
        }
        Ok(Self {
            root: canon,
            writable,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn inside(&self, canonical: &Path) -> bool {
        canonical.starts_with(&self.root)
    }

    /// Looks up `name` in `dir` the way Windows would: exact match first, then
    /// case-insensitively. Returns `None` when absent.
    fn lookup(&self, dir: &Path, name: &str) -> Result<Option<PathBuf>, Fail> {
        let exact = dir.join(name);
        match fs::symlink_metadata(&exact) {
            Ok(_) => return Ok(Some(exact)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_fail(&e)),
        }
        let wanted = name.to_lowercase();
        let mut found: Option<PathBuf> = None;
        let rd = fs::read_dir(dir).map_err(|e| io_fail(&e))?;
        for (n, entry) in rd.enumerate() {
            if n >= MAX_DIR_ENTRIES {
                return Err(Fail::TooBig);
            }
            let Ok(entry) = entry else { continue };
            let fname = entry.file_name();
            let Some(fname) = fname.to_str() else {
                continue;
            };
            if fname.to_lowercase() == wanted {
                if found.is_some() {
                    return Err(Fail::Ambiguous);
                }
                found = Some(entry.path());
            }
        }
        Ok(found)
    }

    /// Maps checked components to a path inside the share.
    pub fn resolve(&self, comps: &[String]) -> Result<Resolved, Fail> {
        let mut cur = self.root.clone();
        for (i, comp) in comps.iter().enumerate() {
            let last = i + 1 == comps.len();
            match self.lookup(&cur, comp)? {
                Some(p) => {
                    let meta = fs::symlink_metadata(&p).map_err(|e| io_fail(&e))?;
                    if meta.file_type().is_symlink() {
                        return Err(Fail::Escape);
                    }
                    if !last && !meta.is_dir() {
                        return Err(Fail::PathNotFound);
                    }
                    cur = p;
                }
                None if last => {
                    let parent = self.checked_canonical(&cur)?;
                    return Ok(Resolved::Missing {
                        parent,
                        name: comp.clone(),
                    });
                }
                None => return Err(Fail::PathNotFound),
            }
        }
        Ok(Resolved::Existing(self.checked_canonical(&cur)?))
    }

    /// Second line of defence: the canonical form has to be inside the root.
    fn checked_canonical(&self, p: &Path) -> Result<PathBuf, Fail> {
        let canon = fs::canonicalize(p).map_err(|e| io_fail(&e))?;
        if self.inside(&canon) {
            Ok(canon)
        } else {
            Err(Fail::Escape)
        }
    }

    /// Re-checks an already resolved path right before a destructive operation.
    pub fn revalidate(&self, p: &Path) -> Result<(), Fail> {
        let meta = fs::symlink_metadata(p).map_err(|e| io_fail(&e))?;
        if meta.file_type().is_symlink() {
            return Err(Fail::Escape);
        }
        self.checked_canonical(p).map(|_| ())
    }
}

// ----------------------------------------------------------------- time/attr

const FILETIME_UNIX_EPOCH: i64 = 116_444_736_000_000_000;

pub fn filetime(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => {
            FILETIME_UNIX_EPOCH
                + (d.as_secs() as i64) * 10_000_000
                + i64::from(d.subsec_nanos() / 100)
        }
        Err(_) => FILETIME_UNIX_EPOCH,
    }
}

struct Stat {
    is_dir: bool,
    size: u64,
    created: i64,
    accessed: i64,
    modified: i64,
}

impl Stat {
    fn of(m: &fs::Metadata) -> Self {
        let modified = m.modified().map(filetime).unwrap_or(FILETIME_UNIX_EPOCH);
        Stat {
            is_dir: m.is_dir(),
            size: if m.is_dir() { 0 } else { m.len() },
            created: m.created().map(filetime).unwrap_or(modified),
            accessed: m.accessed().map(filetime).unwrap_or(modified),
            modified,
        }
    }
}

// ------------------------------------------------------------------ wildcard

/// Windows directory-listing wildcards: `*`, `?` (and the DOS forms `<`, `>`), case-insensitive.
pub fn wild_match(pattern: &str, name: &str) -> bool {
    if pattern.is_empty() || pattern == "*" || pattern == "*.*" {
        return true;
    }
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let n: Vec<char> = name.to_lowercase().chars().collect();
    // Iterative matching with one backtrack point: O(pattern * name), however many
    // stars a hostile server sends.
    let is_star = |c: char| matches!(c, '*' | '<');
    let (mut pi, mut ni) = (0usize, 0usize);
    let mut back: Option<(usize, usize)> = None;
    while ni < n.len() {
        if pi < p.len() && is_star(p[pi]) {
            back = Some((pi + 1, ni));
            pi += 1;
        } else if pi < p.len() && (matches!(p[pi], '?' | '>') || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if let Some((bp, bn)) = back {
            back = Some((bp, bn + 1));
            pi = bp;
            ni = bn + 1;
        } else {
            return false;
        }
    }
    while pi < p.len() && is_star(p[pi]) {
        pi += 1;
    }
    pi == p.len()
}

// -------------------------------------------------------------------- handles

struct Entry {
    name: String,
    stat: Stat,
}

enum Kind {
    File(File),
    Dir {
        entries: Option<Vec<Entry>>,
        pos: usize,
    },
}

struct Open {
    path: PathBuf,
    kind: Kind,
    can_write: bool,
    delete_pending: bool,
}

/// Counters for the UI.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Activity {
    pub read: u64,
    pub written: u64,
    /// Requests refused for security reasons (traversal, symlinks, writes to a read-only share…).
    pub denied: u64,
    /// The last refused remote path, cleaned for display.
    pub last_denied: String,
}

/// The file system behind one announced drive.
pub struct DriveFs {
    share: Share,
    name: String,
    device_id: u32,
    handles: HashMap<u32, Open>,
    next_id: u32,
    written_total: u64,
    pub activity: Activity,
}

fn clean_for_display(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(120).collect()
}

fn access_writes(bits: u32) -> bool {
    // FILE_WRITE_DATA, FILE_APPEND_DATA, FILE_WRITE_EA, FILE_DELETE_CHILD, DELETE, WRITE_DAC,
    // WRITE_OWNER, GENERIC_ALL, GENERIC_WRITE. (FILE_WRITE_ATTRIBUTES is harmless: ignored.)
    const MASK: u32 =
        0x2 | 0x4 | 0x10 | 0x40 | 0x1_0000 | 0x4_0000 | 0x8_0000 | 0x1000_0000 | 0x4000_0000;
    bits & MASK != 0
}

fn access_writes_data(bits: u32) -> bool {
    const MASK: u32 = 0x2 | 0x4 | 0x1000_0000 | 0x4000_0000;
    bits & MASK != 0
}

impl DriveFs {
    pub fn new(share: Share, name: String, device_id: u32) -> Self {
        Self {
            share,
            name,
            device_id,
            handles: HashMap::new(),
            next_id: 1,
            written_total: 0,
            activity: Activity::default(),
        }
    }

    pub fn device_id(&self) -> u32 {
        self.device_id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    #[cfg(test)]
    pub fn open_handles(&self) -> usize {
        self.handles.len()
    }

    /// Forget all open files (new connection).
    pub fn reset(&mut self) {
        self.handles.clear();
    }

    fn deny(&mut self, f: Fail, what: &str) -> Fail {
        if f.security() {
            self.activity.denied += 1;
            self.activity.last_denied = clean_for_display(what);
        }
        f
    }

    /// Share-relative name of an open handle, for the denied-access note.
    fn handle_name(&self, id: u32) -> String {
        self.handles
            .get(&id)
            .and_then(|h| h.path.strip_prefix(&self.share.root).ok())
            .map(|p| format!("\\{}", p.display()))
            .unwrap_or_default()
    }

    fn new_id(&mut self) -> u32 {
        loop {
            let id = self.next_id;
            self.next_id = self.next_id.wrapping_add(1).max(1);
            if !self.handles.contains_key(&id) {
                return id;
            }
        }
    }

    fn file_attrs(&self, is_dir: bool) -> FileAttributes {
        if is_dir {
            FileAttributes::FILE_ATTRIBUTE_DIRECTORY
        } else if self.share.writable {
            FileAttributes::FILE_ATTRIBUTE_NORMAL
        } else {
            FileAttributes::FILE_ATTRIBUTE_READONLY
        }
    }

    // ----------------------------------------------------------- dispatch

    /// Handles one server request; the result is what goes back on the channel.
    pub fn handle(&mut self, req: ServerDriveIoRequest) -> Vec<RdpdrPdu> {
        match req {
            ServerDriveIoRequest::ServerCreateDriveRequest(r) => vec![self.create(r)],
            ServerDriveIoRequest::DeviceCloseRequest(r) => {
                let io = r.device_io_request;
                let st = match self.close(io.file_id) {
                    Ok(()) => nt::SUCCESS,
                    Err(f) => f.status(),
                };
                vec![close_pdu(io, st)]
            }
            ServerDriveIoRequest::DeviceReadRequest(r) => {
                vec![self.read(r.device_io_request, r.length, r.offset)]
            }
            ServerDriveIoRequest::DeviceWriteRequest(r) => {
                vec![self.write(r.device_io_request, r.offset, &r.write_data)]
            }
            ServerDriveIoRequest::ServerDriveQueryInformationRequest(r) => {
                vec![self.query_information(r.device_io_request, r.file_info_class_lvl)]
            }
            ServerDriveIoRequest::ServerDriveSetInformationRequest(r) => {
                vec![self.set_information(r)]
            }
            ServerDriveIoRequest::ServerDriveQueryDirectoryRequest(r) => {
                vec![self.query_directory(
                    r.device_io_request,
                    r.file_info_class_lvl,
                    r.initial_query != 0,
                    &r.path,
                )]
            }
            // Change notifications stay pending: nothing is watched.
            ServerDriveIoRequest::ServerDriveNotifyChangeDirectoryRequest(_) => Vec::new(),
            ServerDriveIoRequest::ServerDriveQueryVolumeInformationRequest(r) => {
                vec![self.query_volume(r.device_io_request, r.fs_info_class_lvl)]
            }
            ServerDriveIoRequest::DeviceControlRequest(r) => {
                // IOCTLs on files do nothing here; FreeRDP answers the same way.
                vec![RdpdrPdu::DeviceControlResponse(DeviceControlResponse::new(
                    r,
                    NtStatus::SUCCESS,
                    None,
                ))]
            }
            ServerDriveIoRequest::ServerDriveLockControlRequest(r) => {
                vec![close_pdu(r.device_io_request, nt::SUCCESS)]
            }
        }
    }

    // -------------------------------------------------------------- create

    fn create(&mut self, req: DeviceCreateRequest) -> RdpdrPdu {
        let io = req.device_io_request.clone();
        match self.create_inner(&req) {
            Ok((file_id, info)) => RdpdrPdu::DeviceCreateResponse(DeviceCreateResponse {
                device_io_reply: DeviceIoResponse::new(io, NtStatus::SUCCESS),
                file_id,
                information: info,
            }),
            Err(f) => {
                let f = self.deny(f, &req.path);
                RdpdrPdu::DeviceCreateResponse(DeviceCreateResponse {
                    device_io_reply: DeviceIoResponse::new(io, NtStatus::from(f.status())),
                    file_id: 0,
                    information: Information::FILE_SUPERSEDED,
                })
            }
        }
    }

    fn create_inner(&mut self, req: &DeviceCreateRequest) -> Result<(u32, Information), Fail> {
        if self.handles.len() >= MAX_HANDLES {
            return Err(Fail::Busy);
        }
        let comps = parse_remote_path(&req.path)?;
        let access = req.desired_access.bits();
        let delete_on_close = req
            .create_options
            .contains(CreateOptions::FILE_DELETE_ON_CLOSE);
        let disp = req.create_disposition;
        let overwrites = matches!(
            disp,
            CreateDisposition::FILE_SUPERSEDE
                | CreateDisposition::FILE_OVERWRITE
                | CreateDisposition::FILE_OVERWRITE_IF
        );
        let creates = matches!(
            disp,
            CreateDisposition::FILE_SUPERSEDE
                | CreateDisposition::FILE_CREATE
                | CreateDisposition::FILE_OPEN_IF
                | CreateDisposition::FILE_OVERWRITE_IF
        );
        let writable = self.share.writable;
        if !writable {
            if access_writes(access) || delete_on_close {
                return Err(Fail::ReadOnly);
            }
            if overwrites || disp == CreateDisposition::FILE_CREATE {
                return Err(Fail::ReadOnly);
            }
        }
        let want_dir = req
            .create_options
            .contains(CreateOptions::FILE_DIRECTORY_FILE);
        let want_file = req
            .create_options
            .contains(CreateOptions::FILE_NON_DIRECTORY_FILE);

        match self.share.resolve(&comps)? {
            Resolved::Existing(path) => {
                let meta = fs::symlink_metadata(&path).map_err(|e| io_fail(&e))?;
                if disp == CreateDisposition::FILE_CREATE {
                    return Err(Fail::Exists);
                }
                let is_dir = meta.is_dir();
                if is_dir && want_file {
                    return Err(Fail::IsADirectory);
                }
                if !is_dir && want_dir {
                    return Err(Fail::NotADirectory);
                }
                if is_dir && overwrites {
                    return Err(Fail::ReadOnly);
                }
                if comps.is_empty() && (delete_on_close || access & 0x1_0000 != 0) {
                    // The share root cannot be deleted.
                    return Err(Fail::ReadOnly);
                }
                let info = match disp {
                    CreateDisposition::FILE_SUPERSEDE => Information::FILE_SUPERSEDED,
                    CreateDisposition::FILE_OVERWRITE | CreateDisposition::FILE_OVERWRITE_IF => {
                        Information::FILE_OVERWRITTEN
                    }
                    _ => Information::FILE_OPENED,
                };
                let can_write = writable && (access_writes_data(access) || overwrites && !is_dir);
                let kind = if is_dir {
                    Kind::Dir {
                        entries: None,
                        pos: 0,
                    }
                } else {
                    let mut opts = OpenOptions::new();
                    opts.read(true).write(can_write);
                    let f = opts.open(&path).map_err(|e| io_fail(&e))?;
                    verify_same_file(&f, &path)?;
                    if overwrites {
                        // Truncate only after the identity check above.
                        f.set_len(0).map_err(|e| io_fail(&e))?;
                    }
                    Kind::File(f)
                };
                let id = self.new_id();
                self.handles.insert(
                    id,
                    Open {
                        path,
                        kind,
                        can_write,
                        delete_pending: delete_on_close,
                    },
                );
                Ok((id, info))
            }
            Resolved::Missing { parent, name } => {
                if !creates {
                    return Err(Fail::NotFound);
                }
                if !writable {
                    return Err(Fail::ReadOnly);
                }
                let path = parent.join(&name);
                self.share.revalidate(&parent)?;
                let kind = if want_dir {
                    make_dir(&path).map_err(|e| io_fail(&e))?;
                    Kind::Dir {
                        entries: None,
                        pos: 0,
                    }
                } else {
                    let f = create_new_file(&path).map_err(|e| io_fail(&e))?;
                    verify_same_file(&f, &path)?;
                    Kind::File(f)
                };
                let id = self.new_id();
                self.handles.insert(
                    id,
                    Open {
                        path,
                        kind,
                        can_write: true,
                        delete_pending: delete_on_close,
                    },
                );
                Ok((id, Information::FILE_SUPERSEDED))
            }
        }
    }

    // --------------------------------------------------------------- close

    fn close(&mut self, file_id: u32) -> Result<(), Fail> {
        let h = self.handles.remove(&file_id).ok_or(Fail::BadHandle)?;
        let Open {
            path,
            kind,
            delete_pending,
            ..
        } = h;
        let is_dir = matches!(kind, Kind::Dir { .. });
        drop(kind);
        if delete_pending && self.share.writable {
            self.share.revalidate(&path)?;
            if path == self.share.root {
                return Err(Fail::ReadOnly);
            }
            let r = if is_dir {
                fs::remove_dir(&path)
            } else {
                fs::remove_file(&path)
            };
            r.map_err(|e| io_fail(&e))?;
        }
        Ok(())
    }

    // ---------------------------------------------------------- read/write

    fn read(&mut self, io: DeviceIoRequest, length: u32, offset: u64) -> RdpdrPdu {
        let result = (|| -> Result<Vec<u8>, Fail> {
            let h = self.handles.get_mut(&io.file_id).ok_or(Fail::BadHandle)?;
            let Kind::File(f) = &mut h.kind else {
                return Err(Fail::IsADirectory);
            };
            let want = (length as usize).min(MAX_READ);
            f.seek(SeekFrom::Start(offset)).map_err(|e| io_fail(&e))?;
            let mut buf = vec![0u8; want];
            let mut got = 0;
            while got < want {
                match f.read(&mut buf[got..]) {
                    Ok(0) => break,
                    Ok(n) => got += n,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(io_fail(&e)),
                }
            }
            buf.truncate(got);
            Ok(buf)
        })();
        match result {
            Ok(data) => {
                self.activity.read += data.len() as u64;
                RdpdrPdu::DeviceReadResponse(DeviceReadResponse {
                    device_io_reply: DeviceIoResponse::new(io, NtStatus::SUCCESS),
                    read_data: data,
                })
            }
            Err(f) => RdpdrPdu::DeviceReadResponse(DeviceReadResponse {
                device_io_reply: DeviceIoResponse::new(io, NtStatus::from(f.status())),
                read_data: Vec::new(),
            }),
        }
    }

    fn write(&mut self, io: DeviceIoRequest, offset: u64, data: &[u8]) -> RdpdrPdu {
        let result = (|| -> Result<usize, Fail> {
            if !self.share.writable {
                return Err(Fail::ReadOnly);
            }
            if data.len() > MAX_WRITE {
                return Err(Fail::TooBig);
            }
            let end = offset.checked_add(data.len() as u64).ok_or(Fail::TooBig)?;
            if end > MAX_FILE_SIZE
                || self.written_total.saturating_add(data.len() as u64) > MAX_SESSION_WRITE
            {
                return Err(Fail::TooBig);
            }
            let h = self.handles.get_mut(&io.file_id).ok_or(Fail::BadHandle)?;
            if !h.can_write {
                return Err(Fail::ReadOnly);
            }
            let Kind::File(f) = &mut h.kind else {
                return Err(Fail::IsADirectory);
            };
            f.seek(SeekFrom::Start(offset)).map_err(|e| io_fail(&e))?;
            f.write_all(data).map_err(|e| io_fail(&e))?;
            Ok(data.len())
        })();
        match result {
            Ok(n) => {
                self.written_total += n as u64;
                self.activity.written += n as u64;
                RdpdrPdu::DeviceWriteResponse(DeviceWriteResponse {
                    device_io_reply: DeviceIoResponse::new(io, NtStatus::SUCCESS),
                    length: n as u32,
                })
            }
            Err(f) => {
                let what = self.handle_name(io.file_id);
                let f = self.deny(f, &what);
                RdpdrPdu::DeviceWriteResponse(DeviceWriteResponse {
                    device_io_reply: DeviceIoResponse::new(io, NtStatus::from(f.status())),
                    length: 0,
                })
            }
        }
    }

    // --------------------------------------------------------- information

    fn stat_of(&self, file_id: u32) -> Result<Stat, Fail> {
        let h = self.handles.get(&file_id).ok_or(Fail::BadHandle)?;
        let meta = match &h.kind {
            Kind::File(f) => f.metadata(),
            Kind::Dir { .. } => fs::symlink_metadata(&h.path),
        }
        .map_err(|e| io_fail(&e))?;
        Ok(Stat::of(&meta))
    }

    fn query_information(
        &mut self,
        io: DeviceIoRequest,
        level: FileInformationClassLevel,
    ) -> RdpdrPdu {
        let result = (|| -> Result<FileInformationClass, Fail> {
            let st = self.stat_of(io.file_id)?;
            let pending = self
                .handles
                .get(&io.file_id)
                .is_some_and(|h| h.delete_pending);
            let attrs = self.file_attrs(st.is_dir);
            match level {
                FileInformationClassLevel::FILE_BASIC_INFORMATION => Ok(FileBasicInformation {
                    creation_time: st.created,
                    last_access_time: st.accessed,
                    last_write_time: st.modified,
                    change_time: st.modified,
                    file_attributes: attrs,
                }
                .into()),
                FileInformationClassLevel::FILE_STANDARD_INFORMATION => {
                    Ok(FileStandardInformation {
                        allocation_size: st.size as i64,
                        end_of_file: st.size as i64,
                        number_of_links: 1,
                        delete_pending: if pending {
                            Boolean::True
                        } else {
                            Boolean::False
                        },
                        directory: if st.is_dir {
                            Boolean::True
                        } else {
                            Boolean::False
                        },
                    }
                    .into())
                }
                FileInformationClassLevel::FILE_ATTRIBUTE_TAG_INFORMATION => {
                    Ok(FileAttributeTagInformation {
                        file_attributes: attrs,
                        reparse_tag: 0,
                    }
                    .into())
                }
                _ => Err(Fail::NotSupported),
            }
        })();
        match result {
            Ok(buf) => {
                RdpdrPdu::ClientDriveQueryInformationResponse(ClientDriveQueryInformationResponse {
                    device_io_response: DeviceIoResponse::new(io, NtStatus::SUCCESS),
                    buffer: Some(buf),
                })
            }
            Err(f) => {
                RdpdrPdu::ClientDriveQueryInformationResponse(ClientDriveQueryInformationResponse {
                    device_io_response: DeviceIoResponse::new(io, NtStatus::from(f.status())),
                    buffer: None,
                })
            }
        }
    }

    fn set_information(&mut self, req: ServerDriveSetInformationRequest) -> RdpdrPdu {
        let result = self.set_information_inner(&req.device_io_request, &req.set_buffer);
        let status = match result {
            Ok(()) => nt::SUCCESS,
            Err(f) => {
                let what = self.handle_name(req.device_io_request.file_id);
                self.deny(f, &what).status()
            }
        };
        match ClientDriveSetInformationResponse::new(&req, NtStatus::from(status)) {
            Ok(r) => RdpdrPdu::ClientDriveSetInformationResponse(r),
            // Cannot happen for the small buffers we accept; fall back to a close-style answer.
            Err(_) => close_pdu(req.device_io_request, nt::UNSUCCESSFUL),
        }
    }

    fn set_information_inner(
        &mut self,
        io: &DeviceIoRequest,
        info: &FileInformationClass,
    ) -> Result<(), Fail> {
        if !self.handles.contains_key(&io.file_id) {
            return Err(Fail::BadHandle);
        }
        match info {
            // Timestamps and attributes are not changed; Explorer asks anyway.
            FileInformationClass::Basic(_) | FileInformationClass::Allocation(_) => Ok(()),
            FileInformationClass::EndOfFile(e) => {
                if !self.share.writable {
                    return Err(Fail::ReadOnly);
                }
                let len = u64::try_from(e.end_of_file).map_err(|_| Fail::BadParam)?;
                if len > MAX_FILE_SIZE {
                    return Err(Fail::TooBig);
                }
                let h = self.handles.get_mut(&io.file_id).ok_or(Fail::BadHandle)?;
                if !h.can_write {
                    return Err(Fail::ReadOnly);
                }
                match &mut h.kind {
                    Kind::File(f) => f.set_len(len).map_err(|e| io_fail(&e)),
                    Kind::Dir { .. } => Err(Fail::IsADirectory),
                }
            }
            FileInformationClass::Disposition(d) => {
                if !self.share.writable {
                    return Err(Fail::ReadOnly);
                }
                let h = self.handles.get_mut(&io.file_id).ok_or(Fail::BadHandle)?;
                if h.path == self.share.root {
                    return Err(Fail::ReadOnly);
                }
                h.delete_pending = d.delete_pending != 0;
                Ok(())
            }
            FileInformationClass::Rename(r) => {
                if !self.share.writable {
                    return Err(Fail::ReadOnly);
                }
                let comps = parse_remote_path(&r.file_name)?;
                if comps.is_empty() {
                    return Err(Fail::BadParam);
                }
                let src = self
                    .handles
                    .get(&io.file_id)
                    .ok_or(Fail::BadHandle)?
                    .path
                    .clone();
                if src == self.share.root {
                    return Err(Fail::ReadOnly);
                }
                self.share.revalidate(&src)?;
                let replace = r.replace_if_exists == Boolean::True;
                let dest = match self.share.resolve(&comps)? {
                    Resolved::Missing { parent, name } => parent.join(name),
                    Resolved::Existing(p) => {
                        if p == src {
                            return Ok(());
                        }
                        let m = fs::symlink_metadata(&p).map_err(|e| io_fail(&e))?;
                        if !replace || m.is_dir() {
                            return Err(Fail::Exists);
                        }
                        p
                    }
                };
                fs::rename(&src, &dest).map_err(|e| io_fail(&e))?;
                if let Some(h) = self.handles.get_mut(&io.file_id) {
                    h.path = dest;
                }
                Ok(())
            }
            _ => Err(Fail::NotSupported),
        }
    }

    // ----------------------------------------------------------- directory

    fn snapshot(&self, dir: &Path) -> Result<Vec<Entry>, Fail> {
        let self_meta = fs::symlink_metadata(dir).map_err(|e| io_fail(&e))?;
        let mut out = vec![
            Entry {
                name: ".".into(),
                stat: Stat::of(&self_meta),
            },
            Entry {
                name: "..".into(),
                stat: Stat::of(&self_meta),
            },
        ];
        let mut found = Vec::new();
        for (n, entry) in fs::read_dir(dir).map_err(|e| io_fail(&e))?.enumerate() {
            if n >= MAX_DIR_ENTRIES {
                break;
            }
            let Ok(entry) = entry else { continue };
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            // Anything Windows could not address (or that we would refuse later) stays hidden.
            if check_component(&name).is_err() {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            found.push(Entry {
                name,
                stat: Stat::of(&meta),
            });
        }
        found.sort_by_key(|e| e.name.to_lowercase());
        out.extend(found);
        Ok(out)
    }

    fn query_directory(
        &mut self,
        io: DeviceIoRequest,
        level: FileInformationClassLevel,
        initial: bool,
        pattern_path: &str,
    ) -> RdpdrPdu {
        let attrs_dir = FileAttributes::FILE_ATTRIBUTE_DIRECTORY;
        let attrs_file = self.file_attrs(false);
        let result = (|| -> Result<Option<FileInformationClass>, Fail> {
            let path = {
                let h = self.handles.get(&io.file_id).ok_or(Fail::BadHandle)?;
                if !matches!(h.kind, Kind::Dir { .. }) {
                    return Err(Fail::NotADirectory);
                }
                h.path.clone()
            };
            // The pattern is the last component of the path the server sends.
            let pattern = pattern_path
                .rsplit(['\\', '/'])
                .next()
                .unwrap_or("")
                .to_string();
            let need = initial
                || matches!(
                    self.handles.get(&io.file_id).map(|h| &h.kind),
                    Some(Kind::Dir { entries: None, .. })
                );
            if need {
                if !initial {
                    return Ok(None);
                }
                self.share.revalidate(&path)?;
                let mut list = self.snapshot(&path)?;
                list.retain(|e| wild_match(&pattern, &e.name));
                if let Some(h) = self.handles.get_mut(&io.file_id) {
                    h.kind = Kind::Dir {
                        entries: Some(list),
                        pos: 0,
                    };
                }
            }
            let h = self.handles.get_mut(&io.file_id).ok_or(Fail::BadHandle)?;
            let Kind::Dir {
                entries: Some(list),
                pos,
            } = &mut h.kind
            else {
                return Ok(None);
            };
            let Some(e) = list.get(*pos) else {
                return Ok(None);
            };
            *pos += 1;
            let attrs = if e.stat.is_dir {
                attrs_dir.clone()
            } else {
                attrs_file.clone()
            };
            let name = e.name.clone();
            let s = &e.stat;
            let size = s.size as i64;
            Ok(Some(match level {
                FileInformationClassLevel::FILE_DIRECTORY_INFORMATION => {
                    FileInformationClass::Directory(FileDirectoryInformation::new(
                        s.created, s.accessed, s.modified, s.modified, size, attrs, name,
                    ))
                }
                FileInformationClassLevel::FILE_FULL_DIRECTORY_INFORMATION => {
                    FileInformationClass::FullDirectory(FileFullDirectoryInformation::new(
                        s.created, s.accessed, s.modified, s.modified, size, attrs, name,
                    ))
                }
                FileInformationClassLevel::FILE_BOTH_DIRECTORY_INFORMATION => {
                    FileInformationClass::BothDirectory(FileBothDirectoryInformation::new(
                        s.created, s.accessed, s.modified, s.modified, size, attrs, name,
                    ))
                }
                FileInformationClassLevel::FILE_NAMES_INFORMATION => {
                    FileInformationClass::Names(FileNamesInformation::new(name))
                }
                _ => return Err(Fail::NotSupported),
            }))
        })();
        match result {
            Ok(Some(buf)) => {
                RdpdrPdu::ClientDriveQueryDirectoryResponse(ClientDriveQueryDirectoryResponse {
                    device_io_reply: DeviceIoResponse::new(io, NtStatus::SUCCESS),
                    buffer: Some(buf),
                })
            }
            Ok(None) => {
                RdpdrPdu::ClientDriveQueryDirectoryResponse(ClientDriveQueryDirectoryResponse {
                    device_io_reply: DeviceIoResponse::new(io, NtStatus::from(nt::NO_MORE_FILES)),
                    buffer: None,
                })
            }
            Err(f) => {
                RdpdrPdu::ClientDriveQueryDirectoryResponse(ClientDriveQueryDirectoryResponse {
                    device_io_reply: DeviceIoResponse::new(io, NtStatus::from(f.status())),
                    buffer: None,
                })
            }
        }
    }

    // -------------------------------------------------------------- volume

    fn query_volume(
        &mut self,
        io: DeviceIoRequest,
        level: FileSystemInformationClassLevel,
    ) -> RdpdrPdu {
        let ro = !self.share.writable;
        let (total, free) = disk_space(&self.share.root);
        const UNIT: u64 = 4096;
        let total_units = (total / UNIT) as i64;
        let free_units = (free / UNIT) as i64;
        let buf: Option<FileSystemInformationClass> = match level {
            FileSystemInformationClassLevel::FILE_FS_VOLUME_INFORMATION => Some(
                FileFsVolumeInformation {
                    volume_creation_time: FILETIME_UNIX_EPOCH,
                    volume_serial_number: serial_of(&self.share.root),
                    supports_objects: Boolean::False,
                    volume_label: self.name.clone(),
                }
                .into(),
            ),
            FileSystemInformationClassLevel::FILE_FS_SIZE_INFORMATION => Some(
                FileFsSizeInformation {
                    total_alloc_units: total_units,
                    available_alloc_units: free_units,
                    sectors_per_alloc_unit: 8,
                    bytes_per_sector: 512,
                }
                .into(),
            ),
            FileSystemInformationClassLevel::FILE_FS_ATTRIBUTE_INFORMATION => {
                let mut attrs = FileSystemAttributes::FILE_CASE_PRESERVED_NAMES
                    | FileSystemAttributes::FILE_UNICODE_ON_DISK;
                if ro {
                    attrs |= FileSystemAttributes::FILE_READ_ONLY_VOLUME;
                }
                Some(
                    FileFsAttributeInformation {
                        file_system_attributes: attrs,
                        max_component_name_len: MAX_COMPONENT_UTF16 as u32,
                        file_system_name: "FAT32".into(),
                    }
                    .into(),
                )
            }
            FileSystemInformationClassLevel::FILE_FS_FULL_SIZE_INFORMATION => Some(
                FileFsFullSizeInformation {
                    total_alloc_units: total_units,
                    caller_available_alloc_units: free_units,
                    actual_available_alloc_units: free_units,
                    sectors_per_alloc_unit: 8,
                    bytes_per_sector: 512,
                }
                .into(),
            ),
            FileSystemInformationClassLevel::FILE_FS_DEVICE_INFORMATION => {
                let mut ch =
                    Characteristics::FILE_REMOTE_DEVICE | Characteristics::FILE_DEVICE_IS_MOUNTED;
                if ro {
                    ch |= Characteristics::FILE_READ_ONLY_DEVICE;
                }
                Some(
                    FileFsDeviceInformation {
                        device_type: 0x07, // FILE_DEVICE_DISK
                        characteristics: ch,
                    }
                    .into(),
                )
            }
            _ => None,
        };
        let status = if buf.is_some() {
            NtStatus::SUCCESS
        } else {
            NtStatus::from(nt::NOT_SUPPORTED)
        };
        RdpdrPdu::ClientDriveQueryVolumeInformationResponse(
            ClientDriveQueryVolumeInformationResponse::new(io, status, buf),
        )
    }
}

fn close_pdu(io: DeviceIoRequest, status: u32) -> RdpdrPdu {
    RdpdrPdu::DeviceCloseResponse(DeviceCloseResponse {
        device_io_response: DeviceIoResponse::new(io, NtStatus::from(status)),
    })
}

/// Answer to any request while no drive is shared (the share was disconnected).
pub fn refuse(req: ServerDriveIoRequest, status: u32) -> Vec<RdpdrPdu> {
    let st = NtStatus::from(status);
    match req {
        ServerDriveIoRequest::ServerCreateDriveRequest(r) => {
            vec![RdpdrPdu::DeviceCreateResponse(DeviceCreateResponse {
                device_io_reply: DeviceIoResponse::new(r.device_io_request, st),
                file_id: 0,
                information: Information::FILE_SUPERSEDED,
            })]
        }
        ServerDriveIoRequest::ServerDriveQueryInformationRequest(r) => {
            vec![RdpdrPdu::ClientDriveQueryInformationResponse(
                ClientDriveQueryInformationResponse {
                    device_io_response: DeviceIoResponse::new(r.device_io_request, st),
                    buffer: None,
                },
            )]
        }
        ServerDriveIoRequest::DeviceCloseRequest(r) => vec![close_pdu(r.device_io_request, status)],
        ServerDriveIoRequest::ServerDriveQueryDirectoryRequest(r) => {
            vec![RdpdrPdu::ClientDriveQueryDirectoryResponse(
                ClientDriveQueryDirectoryResponse {
                    device_io_reply: DeviceIoResponse::new(r.device_io_request, st),
                    buffer: None,
                },
            )]
        }
        ServerDriveIoRequest::ServerDriveNotifyChangeDirectoryRequest(_) => Vec::new(),
        ServerDriveIoRequest::ServerDriveQueryVolumeInformationRequest(r) => {
            vec![RdpdrPdu::ClientDriveQueryVolumeInformationResponse(
                ClientDriveQueryVolumeInformationResponse::new(r.device_io_request, st, None),
            )]
        }
        ServerDriveIoRequest::DeviceControlRequest(r) => {
            vec![RdpdrPdu::DeviceControlResponse(DeviceControlResponse::new(
                r, st, None,
            ))]
        }
        ServerDriveIoRequest::DeviceReadRequest(r) => {
            vec![RdpdrPdu::DeviceReadResponse(DeviceReadResponse {
                device_io_reply: DeviceIoResponse::new(r.device_io_request, st),
                read_data: Vec::new(),
            })]
        }
        ServerDriveIoRequest::DeviceWriteRequest(r) => {
            vec![RdpdrPdu::DeviceWriteResponse(DeviceWriteResponse {
                device_io_reply: DeviceIoResponse::new(r.device_io_request, st),
                length: 0,
            })]
        }
        ServerDriveIoRequest::ServerDriveSetInformationRequest(r) => {
            match ClientDriveSetInformationResponse::new(&r, st) {
                Ok(p) => vec![RdpdrPdu::ClientDriveSetInformationResponse(p)],
                Err(_) => vec![close_pdu(r.device_io_request, status)],
            }
        }
        ServerDriveIoRequest::ServerDriveLockControlRequest(r) => {
            vec![close_pdu(r.device_io_request, status)]
        }
    }
}

// ------------------------------------------------------------ OS specifics

#[cfg(unix)]
fn verify_same_file(f: &File, path: &Path) -> Result<(), Fail> {
    use std::os::unix::fs::MetadataExt;
    // Closes the window between "resolved" and "opened": the opened file has to be the one
    // the path names now (and not a symlink swapped in meanwhile).
    let a = f.metadata().map_err(|e| io_fail(&e))?;
    let b = fs::symlink_metadata(path).map_err(|e| io_fail(&e))?;
    if b.file_type().is_symlink() || a.dev() != b.dev() || a.ino() != b.ino() {
        return Err(Fail::Escape);
    }
    Ok(())
}

#[cfg(not(unix))]
fn verify_same_file(_f: &File, path: &Path) -> Result<(), Fail> {
    let b = fs::symlink_metadata(path).map_err(|e| io_fail(&e))?;
    if b.file_type().is_symlink() {
        return Err(Fail::Escape);
    }
    Ok(())
}

fn create_new_file(path: &Path) -> std::io::Result<File> {
    let mut o = OpenOptions::new();
    o.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Never executable: whatever the server creates cannot be started from here.
        o.mode(0o644);
    }
    o.open(path)
}

fn make_dir(path: &Path) -> std::io::Result<()> {
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut b = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o755);
    }
    b.create(path)
}

fn serial_of(p: &Path) -> u32 {
    // FNV-1a over the path: stable, no meaning.
    let mut h: u32 = 0x811c_9dc5;
    for b in p.to_string_lossy().as_bytes() {
        h ^= u32::from(*b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

#[cfg(unix)]
fn disk_space(p: &Path) -> (u64, u64) {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(p.as_os_str().as_bytes()) else {
        return (1 << 40, 1 << 39);
    };
    // SAFETY: `c` is a valid NUL-terminated path and `st` is a plain struct statvfs fills.
    unsafe {
        let mut st: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c.as_ptr(), &mut st) == 0 {
            let frsize = st.f_frsize as u64;
            return (
                (st.f_blocks as u64).saturating_mul(frsize),
                (st.f_bavail as u64).saturating_mul(frsize),
            );
        }
    }
    (1 << 40, 1 << 39)
}

#[cfg(not(unix))]
fn disk_space(_p: &Path) -> (u64, u64) {
    // Plausible fixed values; Windows hosts do not report the real free space here.
    (1 << 40, 1 << 39)
}

// ------------------------------------------------------------------- backend

/// State shared between the RDPDR channel (inside the session) and the command thread.
pub type SharedDrive = Arc<Mutex<Option<DriveFs>>>;

/// Plugs [`DriveFs`] into IronRDP's RDPDR channel.
pub struct DriveBackend {
    slot: SharedDrive,
}

impl DriveBackend {
    pub fn new(slot: SharedDrive) -> Self {
        Self { slot }
    }
}

impl std::fmt::Debug for DriveBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DriveBackend")
    }
}

ironrdp::core::impl_as_any!(DriveBackend);

impl RdpdrBackend for DriveBackend {
    fn handle_server_device_announce_response(
        &mut self,
        pdu: ServerDeviceAnnounceResponse,
    ) -> PduResult<()> {
        let _ = pdu;
        Ok(())
    }

    fn handle_scard_call(
        &mut self,
        _req: DeviceControlRequest<ScardIoCtlCode>,
        _call: ScardCall,
    ) -> PduResult<()> {
        // Smart cards are never announced.
        Ok(())
    }

    fn handle_drive_io_request(&mut self, req: ServerDriveIoRequest) -> PduResult<Vec<SvcMessage>> {
        let pdus = match self.slot.lock() {
            Ok(mut g) => match g.as_mut() {
                Some(fs) => fs.handle(req),
                None => refuse(req, nt::NO_SUCH_DEVICE),
            },
            Err(_) => refuse(req, nt::UNSUCCESSFUL),
        };
        Ok(pdus.into_iter().map(SvcMessage::from).collect())
    }
}

/// Builds the channel for a connection. `initial` is announced right away.
pub fn make_channel(slot: &SharedDrive, computer: &str) -> Rdpdr {
    let initial = slot.lock().ok().and_then(|mut g| {
        g.as_mut().map(|fs| {
            fs.reset();
            vec![(fs.device_id(), fs.name().to_string())]
        })
    });
    Rdpdr::new(
        Box::new(DriveBackend::new(slot.clone())),
        computer.to_string(),
    )
    .with_drives(initial)
}

// -------------------------------------------------- runtime add / remove

fn stage_event(
    f: impl FnOnce(&mut Rdpdr) -> Vec<SvcMessage> + Send + 'static,
) -> ironrdp::client::rdp::RdpInputEvent {
    use ironrdp::client::rdp::{RdpInputEvent, StageHook};
    use ironrdp::svc::SvcProcessorMessages;
    RdpInputEvent::WithStage(StageHook(Box::new(move |stage| {
        let Some(rdpdr) = stage.get_svc_processor_mut::<Rdpdr>() else {
            return Ok(Vec::new());
        };
        let msgs = f(rdpdr);
        if msgs.is_empty() {
            return Ok(Vec::new());
        }
        stage.process_svc_processor_messages(SvcProcessorMessages::<Rdpdr>::new(msgs))
    })))
}

/// Announces another drive in a running session.
pub fn add_event(device_id: u32, name: String) -> ironrdp::client::rdp::RdpInputEvent {
    stage_event(move |r| {
        vec![SvcMessage::from(RdpdrPdu::ClientDeviceListAnnounce(
            r.add_drive(device_id, name),
        ))]
    })
}

/// Removes a drive from a running session.
pub fn remove_event(device_id: u32) -> ironrdp::client::rdp::RdpInputEvent {
    stage_event(move |r| {
        r.remove_device(device_id)
            .map(|p| vec![SvcMessage::from(RdpdrPdu::ClientDeviceListRemove(p))])
            .unwrap_or_default()
    })
}

#[cfg(test)]
#[path = "drive_tests.rs"]
mod tests;
