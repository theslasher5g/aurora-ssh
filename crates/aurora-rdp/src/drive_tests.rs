//! Tests for the drive share: path confinement, request handling and a wire-level exchange
//! between a fake RDPDR server and the real IronRDP channel with our backend.

use super::*;
use ironrdp::core::encode_vec;
use ironrdp::rdpdr::pdu::efs::{
    DesiredAccess, DeviceReadRequest, DeviceWriteRequest, FileAttributes, MajorFunction,
    MinorFunction, ServerDriveQueryDirectoryRequest, ServerDriveQueryInformationRequest,
    ServerDriveQueryVolumeInformationRequest, SharedAccess,
};
use ironrdp::svc::SvcProcessor;

fn tmp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("aurora-drive-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

// ------------------------------------------------------------ path syntax

#[test]
fn accepts_ordinary_paths() {
    assert_eq!(parse_remote_path("").unwrap(), Vec::<String>::new());
    assert_eq!(parse_remote_path("\\").unwrap(), Vec::<String>::new());
    assert_eq!(parse_remote_path("\\a.txt").unwrap(), s(&["a.txt"]));
    assert_eq!(
        parse_remote_path("a\\b\\c.txt").unwrap(),
        s(&["a", "b", "c.txt"])
    );
    assert_eq!(parse_remote_path("\\dir\\").unwrap(), s(&["dir"]));
    assert_eq!(
        parse_remote_path("\\Ünï cödé (1).txt").unwrap(),
        s(&["Ünï cödé (1).txt"])
    );
    assert_eq!(parse_remote_path("\\.hidden").unwrap(), s(&[".hidden"]));
    assert_eq!(parse_remote_path("\\a b").unwrap(), s(&["a b"]));
}

#[test]
fn rejects_traversal() {
    for p in [
        "..",
        "\\..",
        "\\..\\x",
        "\\a\\..\\..\\x",
        "a\\..",
        "\\a\\..\\b",
        "..\\..\\..\\etc\\passwd",
        "\\a\\.\\b",
        ".",
    ] {
        assert!(parse_remote_path(p).is_err(), "{p}");
    }
    assert_eq!(parse_remote_path("\\..").unwrap_err(), Fail::Traversal);
    // xrdp sends forward slashes: a separator like the backslash, never part of a name.
    for p in [
        "a/../b",
        "\\a/..\\b",
        "../x",
        "/../x",
        "a/b/../../../c",
        "a/./b",
    ] {
        assert!(parse_remote_path(p).is_err(), "{p}");
    }
    assert_eq!(
        parse_remote_path("/etc/passwd").unwrap(),
        s(&["etc", "passwd"])
    );
    assert_eq!(parse_remote_path("a/b\\c").unwrap(), s(&["a", "b", "c"]));
    assert_eq!(
        parse_remote_path("/Sub/deep.txt").unwrap(),
        s(&["Sub", "deep.txt"])
    );
}

#[test]
fn rejects_unc_and_absolute_forms() {
    for p in [
        "\\\\server\\share\\x",
        "\\\\?\\C:\\Windows",
        "\\\\.\\PhysicalDrive0",
        "//server/share",
        "C:\\Windows",
        "C:",
        "\\C:\\x",
        "\\a\\C:\\x",
        "d:foo",
    ] {
        assert!(parse_remote_path(p).is_err(), "{p}");
    }
    assert_eq!(parse_remote_path("\\\\srv\\x").unwrap_err(), Fail::Unc);
}

#[test]
fn rejects_device_names_in_any_form() {
    for n in [
        "CON",
        "con",
        "Prn",
        "AUX",
        "NUL",
        "COM1",
        "com9",
        "LPT1",
        "lpt0",
        "CON.txt",
        "nul.tar.gz",
        "Aux.",
        "COM1 ",
        "CONIN$",
        "CONOUT$",
        "COM\u{b9}",
        "LPT\u{b2}",
        "con .txt",
    ] {
        assert!(parse_remote_path(&format!("\\{n}")).is_err(), "{n}");
        assert!(
            parse_remote_path(&format!("\\dir\\{n}")).is_err(),
            "dir\\{n}"
        );
    }
    // Look-alikes are fine.
    for n in [
        "CONSOLE",
        "COM10",
        "COM",
        "LPT",
        "console.txt",
        "NULL",
        "AUXX",
        "xCON",
        "com1x",
        "C0N",
    ] {
        assert!(parse_remote_path(&format!("\\{n}")).is_ok(), "{n}");
    }
    assert!(is_reserved_device_name("Con.TXT"));
    assert!(!is_reserved_device_name("Contact"));
}

#[test]
fn rejects_trailing_dots_and_spaces() {
    for n in [
        "a.",
        "a ",
        "a. .",
        "... ",
        "...",
        "x...",
        "name.txt.",
        "name.txt ",
        " .",
    ] {
        assert!(parse_remote_path(&format!("\\{n}")).is_err(), "{n:?}");
    }
    assert_eq!(
        parse_remote_path("\\a.").unwrap_err(),
        Fail::TrailingDotSpace
    );
    assert!(parse_remote_path("\\ a").is_ok());
    assert!(parse_remote_path("\\a.b").is_ok());
}

#[test]
fn rejects_invalid_characters() {
    for n in [
        "a:b", "a:stream", "a::$DATA", "a<b", "a>b", "a\"b", "a|b", "a?b", "a*b", "a\0b", "a\nb",
        "a\tb", "a\x7fb",
    ] {
        assert!(parse_remote_path(&format!("\\{n}")).is_err(), "{n:?}");
    }
    assert!(parse_remote_path("a\\\\b").is_err(), "empty component");
    assert!(parse_remote_path("\\\\").is_err());
}

#[test]
fn bounds_length_and_depth() {
    let long = "a".repeat(256);
    assert_eq!(
        parse_remote_path(&format!("\\{long}")).unwrap_err(),
        Fail::TooLong
    );
    assert!(parse_remote_path(&format!("\\{}", "a".repeat(255))).is_ok());
    // 255 UTF-16 units: an astral character counts twice.
    assert!(parse_remote_path(&format!("\\{}", "😀".repeat(128))).is_err());
    let deep = (0..65).map(|_| "d").collect::<Vec<_>>().join("\\");
    assert_eq!(parse_remote_path(&deep).unwrap_err(), Fail::TooDeep);
    let ok = (0..64).map(|_| "d").collect::<Vec<_>>().join("\\");
    assert!(parse_remote_path(&ok).is_ok());
    let huge = format!("\\{}", "a\\".repeat(20_000));
    assert!(parse_remote_path(&huge).is_err());
}

#[test]
fn drive_names_are_sanitized() {
    assert_eq!(sanitize_drive_name("Dokumente", "X"), "Dokumente");
    assert_eq!(sanitize_drive_name("a/b\\c<d>", "X"), "abcd");
    assert_eq!(sanitize_drive_name("   ", "X"), "X");
    assert_eq!(sanitize_drive_name("\u{0}\u{1}", "Fallback"), "Fallback");
    assert_eq!(
        sanitize_drive_name(&"n".repeat(100), "X").chars().count(),
        MAX_NAME_CHARS
    );
}

#[test]
fn wildcards() {
    assert!(wild_match("*", "anything"));
    assert!(wild_match("*.*", "noext"));
    assert!(wild_match("", "x"));
    assert!(wild_match("*.TXT", "a.txt"));
    assert!(wild_match("a?c", "ABC"));
    assert!(!wild_match("a?c", "ac"));
    assert!(wild_match("a*b*c", "aXXbYYc"));
    assert!(!wild_match("a*b*c", "aXXbYY"));
    assert!(wild_match("name.txt", "NAME.TXT"));
    assert!(!wild_match("name.txt", "name.txt2"));
    // Pathological patterns do not hang.
    assert!(!wild_match(&"*a".repeat(40), &"a".repeat(40)));
}

// ------------------------------------------------------------ confinement

fn share_with_files(name: &str, writable: bool) -> (PathBuf, Share) {
    let root = tmp(name);
    fs::write(root.join("Readme.TXT"), b"hello").unwrap();
    fs::create_dir_all(root.join("Sub/Deeper")).unwrap();
    fs::write(root.join("Sub/Deeper/file.bin"), b"deep").unwrap();
    let share = Share::new(&root, writable).unwrap();
    (root, share)
}

#[test]
fn resolves_inside_the_root() {
    let (root, share) = share_with_files("resolve", false);
    let canon = fs::canonicalize(&root).unwrap();
    match share.resolve(&[]).unwrap() {
        Resolved::Existing(p) => assert_eq!(p, canon),
        r => panic!("{r:?}"),
    }
    match share.resolve(&s(&["Sub", "Deeper", "file.bin"])).unwrap() {
        Resolved::Existing(p) => assert_eq!(p, canon.join("Sub/Deeper/file.bin")),
        r => panic!("{r:?}"),
    }
    match share.resolve(&s(&["Sub", "new.txt"])).unwrap() {
        Resolved::Missing { parent, name } => {
            assert_eq!(parent, canon.join("Sub"));
            assert_eq!(name, "new.txt");
        }
        r => panic!("{r:?}"),
    }
    assert_eq!(
        share.resolve(&s(&["Nope", "x"])).unwrap_err(),
        Fail::PathNotFound
    );
    // A file in the middle of the path is not a directory.
    assert_eq!(
        share.resolve(&s(&["Readme.TXT", "x"])).unwrap_err(),
        Fail::PathNotFound
    );
}

#[test]
fn lookup_is_case_insensitive_like_windows() {
    let (root, share) = share_with_files("case", false);
    let canon = fs::canonicalize(&root).unwrap();
    for name in ["readme.txt", "README.TXT", "ReadMe.Txt"] {
        match share.resolve(&s(&[name])).unwrap() {
            Resolved::Existing(p) => assert_eq!(p, canon.join("Readme.TXT"), "{name}"),
            r => panic!("{r:?}"),
        }
    }
    match share.resolve(&s(&["SUB", "deeper", "FILE.BIN"])).unwrap() {
        Resolved::Existing(p) => assert_eq!(p, canon.join("Sub/Deeper/file.bin")),
        r => panic!("{r:?}"),
    }
}

#[test]
fn ambiguous_case_variants_are_refused() {
    let root = tmp("ambiguous");
    fs::write(root.join("Foo"), b"1").unwrap();
    // File systems that fold case cannot hold both; then there is nothing to refuse.
    if fs::write(root.join("FOO"), b"2").is_err() || fs::read_dir(&root).unwrap().count() < 2 {
        return;
    }
    let share = Share::new(&root, false).unwrap();
    assert!(matches!(
        share.resolve(&s(&["Foo"])).unwrap(),
        Resolved::Existing(_)
    ));
    assert_eq!(share.resolve(&s(&["foo"])).unwrap_err(), Fail::Ambiguous);
}

#[cfg(unix)]
#[test]
fn symlinks_never_resolve() {
    use std::os::unix::fs::symlink;
    let (root, share) = share_with_files("symlink", true);
    let outside = tmp("symlink-outside");
    fs::write(outside.join("secret.txt"), b"secret").unwrap();
    symlink(outside.join("secret.txt"), root.join("link-file")).unwrap();
    symlink(&outside, root.join("link-dir")).unwrap();
    symlink("/etc/passwd", root.join("link-abs")).unwrap();
    symlink("../../../../etc", root.join("link-rel")).unwrap();
    // Symlinks that stay inside are refused too: one rule, nothing to get wrong.
    symlink(root.join("Readme.TXT"), root.join("link-inside")).unwrap();
    symlink(".", root.join("link-self")).unwrap();
    // A link chain through a subfolder.
    symlink(&outside, root.join("Sub/escape")).unwrap();

    for comps in [
        s(&["link-file"]),
        s(&["link-dir"]),
        s(&["link-dir", "secret.txt"]),
        s(&["link-abs"]),
        s(&["link-rel"]),
        s(&["link-rel", "passwd"]),
        s(&["link-inside"]),
        s(&["link-self", "Readme.TXT"]),
        s(&["Sub", "escape"]),
        s(&["Sub", "escape", "secret.txt"]),
        s(&["LINK-FILE"]),
    ] {
        let r = share.resolve(&comps);
        assert_eq!(r.unwrap_err(), Fail::Escape, "{comps:?}");
    }
    // The listing does not even show them.
    let fsys = DriveFs::new(share.clone(), "T".into(), 1);
    let names: Vec<String> = fsys
        .snapshot(share.root())
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert!(names.contains(&"Readme.TXT".to_string()));
    assert!(!names.iter().any(|n| n.starts_with("link")), "{names:?}");
}

#[cfg(unix)]
#[test]
fn a_symlinked_share_root_is_followed_once_then_pinned() {
    use std::os::unix::fs::symlink;
    let real = tmp("rootlink-real");
    fs::write(real.join("a.txt"), b"a").unwrap();
    let parent = tmp("rootlink-parent");
    symlink(&real, parent.join("shortcut")).unwrap();
    // The user typed the shortcut: the canonical target becomes the root.
    let share = Share::new(&parent.join("shortcut"), false).unwrap();
    assert_eq!(share.root(), fs::canonicalize(&real).unwrap());
    assert!(matches!(
        share.resolve(&s(&["a.txt"])).unwrap(),
        Resolved::Existing(_)
    ));
}

#[test]
fn root_must_be_a_real_folder() {
    let root = tmp("rootcheck");
    fs::write(root.join("file"), b"x").unwrap();
    assert!(Share::new(&root.join("file"), false).is_err());
    assert!(Share::new(&root.join("missing"), false).is_err());
    assert!(Share::new(Path::new("/"), false).is_err());
    assert!(Share::new(&root, false).is_ok());
}

#[test]
fn filetime_conversion() {
    assert_eq!(filetime(UNIX_EPOCH), 116_444_736_000_000_000);
    assert_eq!(
        filetime(UNIX_EPOCH + std::time::Duration::from_secs(1)),
        116_444_736_000_000_000 + 10_000_000
    );
}

// ------------------------------------------------------------ requests

fn io(major: MajorFunction, file_id: u32, id: u32) -> DeviceIoRequest {
    DeviceIoRequest {
        device_id: 1,
        file_id,
        completion_id: id,
        major_function: major,
        minor_function: MinorFunction::from(0),
    }
}

fn create_req(
    path: &str,
    access: u32,
    disp: CreateDisposition,
    options: CreateOptions,
) -> ServerDriveIoRequest {
    ServerDriveIoRequest::ServerCreateDriveRequest(DeviceCreateRequest {
        device_io_request: io(MajorFunction::Create, 0, 7),
        desired_access: DesiredAccess::from_bits_retain(access),
        allocation_size: 0,
        file_attributes: FileAttributes::empty(),
        shared_access: SharedAccess::from_bits_retain(7),
        create_disposition: disp,
        create_options: options,
        path: path.to_string(),
    })
}

const READ: u32 = 0x8000_0000 | 0x0012_0089; // GENERIC_READ | SYNCHRONIZE | READ_CONTROL | attrs | read
const WRITE: u32 = 0x0012_0116 | 0x2; // typical write mask incl. FILE_WRITE_DATA
const NONE: CreateOptions = CreateOptions::empty();

fn status_of(p: &RdpdrPdu) -> u32 {
    match p {
        RdpdrPdu::DeviceCreateResponse(r) => r.device_io_reply.io_status.into(),
        RdpdrPdu::DeviceCloseResponse(r) => r.device_io_response.io_status.into(),
        RdpdrPdu::DeviceReadResponse(r) => r.device_io_reply.io_status.into(),
        RdpdrPdu::DeviceWriteResponse(r) => r.device_io_reply.io_status.into(),
        RdpdrPdu::ClientDriveQueryInformationResponse(r) => r.device_io_response.io_status.into(),
        RdpdrPdu::ClientDriveQueryDirectoryResponse(r) => r.device_io_reply.io_status.into(),
        RdpdrPdu::ClientDriveQueryVolumeInformationResponse(r) => {
            r.device_io_reply.io_status.into()
        }
        RdpdrPdu::ClientDriveSetInformationResponse(_) => {
            let bytes = encode_vec(p).unwrap();
            u32::from_le_bytes(bytes[12..16].try_into().unwrap())
        }
        other => panic!("unexpected {other:?}"),
    }
}

fn open(
    fsys: &mut DriveFs,
    path: &str,
    access: u32,
    disp: CreateDisposition,
    opts: CreateOptions,
) -> (u32, u32) {
    let r = fsys.handle(create_req(path, access, disp, opts));
    assert_eq!(r.len(), 1);
    let RdpdrPdu::DeviceCreateResponse(c) = &r[0] else {
        panic!()
    };
    (c.device_io_reply.io_status.into(), c.file_id)
}

fn read(fsys: &mut DriveFs, id: u32, len: u32, off: u64) -> (u32, Vec<u8>) {
    let r = fsys.handle(ServerDriveIoRequest::DeviceReadRequest(DeviceReadRequest {
        device_io_request: io(MajorFunction::Read, id, 8),
        length: len,
        offset: off,
    }));
    let RdpdrPdu::DeviceReadResponse(r) = &r[0] else {
        panic!()
    };
    (r.device_io_reply.io_status.into(), r.read_data.clone())
}

fn write(fsys: &mut DriveFs, id: u32, off: u64, data: &[u8]) -> (u32, u32) {
    let r = fsys.handle(ServerDriveIoRequest::DeviceWriteRequest(
        DeviceWriteRequest {
            device_io_request: io(MajorFunction::Write, id, 9),
            offset: off,
            write_data: data.to_vec(),
        },
    ));
    let RdpdrPdu::DeviceWriteResponse(r) = &r[0] else {
        panic!()
    };
    (r.device_io_reply.io_status.into(), r.length)
}

fn close(fsys: &mut DriveFs, id: u32) -> u32 {
    let r = fsys.handle(ServerDriveIoRequest::DeviceCloseRequest(
        ironrdp::rdpdr::pdu::efs::DeviceCloseRequest::decode(io(MajorFunction::Close, id, 10)),
    ));
    status_of(&r[0])
}

fn fsys(name: &str, writable: bool) -> (PathBuf, DriveFs) {
    let (root, share) = share_with_files(name, writable);
    (root, DriveFs::new(share, "Test".into(), 1))
}

#[test]
fn reads_files_and_directories() {
    let (_root, mut d) = fsys("read", false);
    let (st, id) = open(
        &mut d,
        "\\readme.txt",
        READ,
        CreateDisposition::FILE_OPEN,
        NONE,
    );
    assert_eq!(st, 0);
    assert_ne!(id, 0);
    assert_eq!(read(&mut d, id, 100, 0), (0, b"hello".to_vec()));
    assert_eq!(read(&mut d, id, 3, 1), (0, b"ell".to_vec()));
    assert_eq!(read(&mut d, id, 10, 5), (0, Vec::new()), "EOF");
    assert_eq!(close(&mut d, id), 0);
    assert_eq!(d.open_handles(), 0);
    // Unknown handle.
    assert_eq!(read(&mut d, 999, 1, 0).0, nt::INVALID_HANDLE);
    assert_eq!(close(&mut d, 999), nt::INVALID_HANDLE);
    // Directories open as directories and refuse reads.
    let (st, dir) = open(
        &mut d,
        "\\Sub",
        READ,
        CreateDisposition::FILE_OPEN,
        CreateOptions::FILE_DIRECTORY_FILE,
    );
    assert_eq!(st, 0);
    assert_eq!(read(&mut d, dir, 1, 0).0, nt::FILE_IS_A_DIRECTORY);
    // Directory flag mismatches.
    assert_eq!(
        open(
            &mut d,
            "\\Sub",
            READ,
            CreateDisposition::FILE_OPEN,
            CreateOptions::FILE_NON_DIRECTORY_FILE
        )
        .0,
        nt::FILE_IS_A_DIRECTORY
    );
    assert_eq!(
        open(
            &mut d,
            "\\Readme.TXT",
            READ,
            CreateDisposition::FILE_OPEN,
            CreateOptions::FILE_DIRECTORY_FILE
        )
        .0,
        nt::NOT_A_DIRECTORY
    );
    assert_eq!(
        open(
            &mut d,
            "\\missing",
            READ,
            CreateDisposition::FILE_OPEN,
            NONE
        )
        .0,
        nt::OBJECT_NAME_NOT_FOUND
    );
    assert_eq!(
        open(
            &mut d,
            "\\no\\such",
            READ,
            CreateDisposition::FILE_OPEN,
            NONE
        )
        .0,
        nt::OBJECT_PATH_NOT_FOUND
    );
}

#[test]
fn read_length_is_capped() {
    let (root, mut d) = fsys("bigread", false);
    fs::write(root.join("big.bin"), vec![7u8; MAX_READ + 5000]).unwrap();
    let (_, id) = open(
        &mut d,
        "\\big.bin",
        READ,
        CreateDisposition::FILE_OPEN,
        NONE,
    );
    let (st, data) = read(&mut d, id, u32::MAX, 0);
    assert_eq!(st, 0);
    assert_eq!(data.len(), MAX_READ);
    let (_, tail) = read(&mut d, id, u32::MAX, MAX_READ as u64);
    assert_eq!(tail.len(), 5000);
}

#[test]
fn read_only_share_refuses_every_write_path() {
    let (root, mut d) = fsys("ro", false);
    // Write access, create, overwrite, supersede, delete-on-close: all refused.
    assert_eq!(
        open(
            &mut d,
            "\\Readme.TXT",
            WRITE,
            CreateDisposition::FILE_OPEN,
            NONE
        )
        .0,
        nt::ACCESS_DENIED
    );
    assert_eq!(
        open(
            &mut d,
            "\\Readme.TXT",
            0x4000_0000,
            CreateDisposition::FILE_OPEN,
            NONE
        )
        .0,
        nt::ACCESS_DENIED
    );
    assert_eq!(
        open(
            &mut d,
            "\\Readme.TXT",
            0x1000_0000,
            CreateDisposition::FILE_OPEN,
            NONE
        )
        .0,
        nt::ACCESS_DENIED
    );
    assert_eq!(
        open(
            &mut d,
            "\\Readme.TXT",
            0x1_0000,
            CreateDisposition::FILE_OPEN,
            NONE
        )
        .0,
        nt::ACCESS_DENIED
    );
    assert_eq!(
        open(
            &mut d,
            "\\Readme.TXT",
            READ,
            CreateDisposition::FILE_OVERWRITE,
            NONE
        )
        .0,
        nt::ACCESS_DENIED
    );
    assert_eq!(
        open(
            &mut d,
            "\\Readme.TXT",
            READ,
            CreateDisposition::FILE_OVERWRITE_IF,
            NONE
        )
        .0,
        nt::ACCESS_DENIED
    );
    assert_eq!(
        open(
            &mut d,
            "\\Readme.TXT",
            READ,
            CreateDisposition::FILE_SUPERSEDE,
            NONE
        )
        .0,
        nt::ACCESS_DENIED
    );
    assert_eq!(
        open(
            &mut d,
            "\\Readme.TXT",
            READ,
            CreateDisposition::FILE_CREATE,
            NONE
        )
        .0,
        nt::ACCESS_DENIED
    );
    assert_eq!(
        open(
            &mut d,
            "\\Readme.TXT",
            READ,
            CreateDisposition::FILE_OPEN,
            CreateOptions::FILE_DELETE_ON_CLOSE
        )
        .0,
        nt::ACCESS_DENIED
    );
    assert_eq!(
        open(
            &mut d,
            "\\new.txt",
            READ,
            CreateDisposition::FILE_OPEN_IF,
            NONE
        )
        .0,
        nt::ACCESS_DENIED
    );
    assert_eq!(
        open(
            &mut d,
            "\\new.txt",
            WRITE,
            CreateDisposition::FILE_CREATE,
            NONE
        )
        .0,
        nt::ACCESS_DENIED
    );
    assert_eq!(
        open(
            &mut d,
            "\\newdir",
            READ,
            CreateDisposition::FILE_CREATE,
            CreateOptions::FILE_DIRECTORY_FILE
        )
        .0,
        nt::ACCESS_DENIED
    );
    assert!(!root.join("new.txt").exists() && !root.join("newdir").exists());
    // A handle opened for reading cannot write, truncate, delete or rename.
    let (_, id) = open(
        &mut d,
        "\\Readme.TXT",
        READ,
        CreateDisposition::FILE_OPEN,
        NONE,
    );
    assert_eq!(write(&mut d, id, 0, b"x"), (nt::ACCESS_DENIED, 0));
    for info in [
        FileInformationClass::EndOfFile(eof(0)),
        FileInformationClass::Disposition(disposition(1)),
        FileInformationClass::Rename(rename("\\other.txt", false)),
    ] {
        let r = d.handle(ServerDriveIoRequest::ServerDriveSetInformationRequest(
            ServerDriveSetInformationRequest {
                device_io_request: io(MajorFunction::SetInformation, id, 11),
                set_buffer: info,
            },
        ));
        assert_eq!(status_of(&r[0]), nt::ACCESS_DENIED);
    }
    assert_eq!(close(&mut d, id), 0);
    assert_eq!(fs::read(root.join("Readme.TXT")).unwrap(), b"hello");
    assert!(root.join("Readme.TXT").exists());
    // These attempts show up as denied.
    assert!(d.activity.denied >= 10);
}

fn eof(n: i64) -> ironrdp::rdpdr::pdu::efs::FileEndOfFileInformation {
    // The type has no public constructor: decode it from its wire form.
    let bytes = n.to_le_bytes();
    let mut src = ironrdp::core::ReadCursor::new(&bytes);
    match FileInformationClass::decode(
        FileInformationClassLevel::FILE_END_OF_FILE_INFORMATION,
        8,
        &mut src,
    )
    .unwrap()
    {
        FileInformationClass::EndOfFile(e) => e,
        _ => unreachable!(),
    }
}

fn disposition(v: u8) -> ironrdp::rdpdr::pdu::efs::FileDispositionInformation {
    let bytes = [v];
    let mut src = ironrdp::core::ReadCursor::new(&bytes);
    match FileInformationClass::decode(
        FileInformationClassLevel::FILE_DISPOSITION_INFORMATION,
        1,
        &mut src,
    )
    .unwrap()
    {
        FileInformationClass::Disposition(e) => e,
        _ => unreachable!(),
    }
}

fn rename(to: &str, replace: bool) -> ironrdp::rdpdr::pdu::efs::FileRenameInformation {
    let mut bytes = vec![replace as u8, 0];
    let mut name: Vec<u8> = to.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    name.extend_from_slice(&[0, 0]);
    bytes.extend_from_slice(&(name.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&name);
    let mut src = ironrdp::core::ReadCursor::new(&bytes);
    match FileInformationClass::decode(
        FileInformationClassLevel::FILE_RENAME_INFORMATION,
        bytes.len(),
        &mut src,
    )
    .unwrap()
    {
        FileInformationClass::Rename(e) => e,
        _ => unreachable!(),
    }
}

fn set_info(d: &mut DriveFs, id: u32, info: FileInformationClass) -> u32 {
    let r = d.handle(ServerDriveIoRequest::ServerDriveSetInformationRequest(
        ServerDriveSetInformationRequest {
            device_io_request: io(MajorFunction::SetInformation, id, 12),
            set_buffer: info,
        },
    ));
    status_of(&r[0])
}

#[test]
fn hostile_paths_are_refused_with_access_denied_or_invalid_name() {
    let (root, mut d) = fsys("hostile", true);
    let outside = root
        .parent()
        .unwrap()
        .join(format!("aurora-outside-{}", std::process::id()));
    fs::write(&outside, b"outside").unwrap();
    for p in [
        "\\..\\x",
        "..\\..\\etc\\passwd",
        "\\Sub\\..\\..\\x",
        "\\\\server\\share",
        "\\\\?\\C:\\x",
        "C:\\Windows\\win.ini",
        "//etc/passwd",
        "\\CON",
        "\\Sub\\NUL.txt",
        "\\file.",
        "\\file ",
        "\\a:b",
        "\\x\0y",
    ] {
        for disp in [
            CreateDisposition::FILE_OPEN,
            CreateDisposition::FILE_CREATE,
            CreateDisposition::FILE_OVERWRITE_IF,
        ] {
            let (st, id) = open(&mut d, p, WRITE, disp, NONE);
            assert!(
                st == nt::ACCESS_DENIED || st == nt::OBJECT_NAME_INVALID,
                "{p:?} {disp:?} -> {st:#x}"
            );
            assert_eq!(id, 0);
        }
    }
    assert_eq!(fs::read(&outside).unwrap(), b"outside");
    assert_eq!(d.open_handles(), 0);
    let _ = fs::remove_file(&outside);
    // Nothing was created anywhere in the share.
    let names: Vec<_> = fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");
}

#[cfg(unix)]
#[test]
fn symlinks_cannot_be_opened_read_or_written() {
    use std::os::unix::fs::symlink;
    let (root, mut d) = fsys("symlink-io", true);
    let outside = tmp("symlink-io-outside");
    fs::write(outside.join("secret.txt"), b"secret").unwrap();
    symlink(outside.join("secret.txt"), root.join("evil")).unwrap();
    symlink(&outside, root.join("evildir")).unwrap();
    for (p, access, disp) in [
        ("\\evil", READ, CreateDisposition::FILE_OPEN),
        ("\\evil", WRITE, CreateDisposition::FILE_OPEN_IF),
        ("\\evil", WRITE, CreateDisposition::FILE_OVERWRITE_IF),
        ("\\evildir\\secret.txt", READ, CreateDisposition::FILE_OPEN),
        ("\\evildir\\new.txt", WRITE, CreateDisposition::FILE_CREATE),
    ] {
        let (st, id) = open(&mut d, p, access, disp, NONE);
        assert_eq!(st, nt::ACCESS_DENIED, "{p}");
        assert_eq!(id, 0);
    }
    assert_eq!(fs::read(outside.join("secret.txt")).unwrap(), b"secret");
    assert!(!outside.join("new.txt").exists());
    // Listing the root hides the links and a recursive listing cannot reach through them.
    let (_, dir) = open(
        &mut d,
        "\\",
        READ,
        CreateDisposition::FILE_OPEN,
        CreateOptions::FILE_DIRECTORY_FILE,
    );
    let names = list(&mut d, dir, "*");
    assert!(names.iter().all(|n| !n.starts_with("evil")), "{names:?}");
}

#[test]
fn writable_share_creates_writes_renames_deletes() {
    let (root, mut d) = fsys("rw", true);
    let (st, id) = open(
        &mut d,
        "\\New File.txt",
        WRITE,
        CreateDisposition::FILE_CREATE,
        NONE,
    );
    assert_eq!(st, 0);
    assert_eq!(write(&mut d, id, 0, b"abc"), (0, 3));
    assert_eq!(write(&mut d, id, 3, b"def"), (0, 3));
    assert_eq!(close(&mut d, id), 0);
    assert_eq!(fs::read(root.join("New File.txt")).unwrap(), b"abcdef");
    // Creating again collides.
    assert_eq!(
        open(
            &mut d,
            "\\new file.TXT",
            WRITE,
            CreateDisposition::FILE_CREATE,
            NONE
        )
        .0,
        nt::OBJECT_NAME_COLLISION
    );
    // New files are never executable.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(root.join("New File.txt"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0, "{mode:o}");
    }
    // Overwrite truncates.
    let (st, id) = open(
        &mut d,
        "\\New File.txt",
        WRITE,
        CreateDisposition::FILE_OVERWRITE_IF,
        NONE,
    );
    assert_eq!(st, 0);
    assert_eq!(fs::read(root.join("New File.txt")).unwrap(), b"");
    assert_eq!(write(&mut d, id, 0, b"xyz"), (0, 3));
    // Truncate via end-of-file.
    assert_eq!(
        set_info(&mut d, id, FileInformationClass::EndOfFile(eof(1))),
        0
    );
    assert_eq!(close(&mut d, id), 0);
    assert_eq!(fs::read(root.join("New File.txt")).unwrap(), b"x");
    // Rename inside the share.
    let (_, id) = open(
        &mut d,
        "\\New File.txt",
        0x1_0000 | 0x80,
        CreateDisposition::FILE_OPEN,
        NONE,
    );
    assert_eq!(
        set_info(
            &mut d,
            id,
            FileInformationClass::Rename(rename("\\Sub\\moved.txt", false))
        ),
        0
    );
    assert!(root.join("Sub/moved.txt").exists() && !root.join("New File.txt").exists());
    // Rename out of the share, onto an existing file without the flag, or to a bad name: refused.
    for to in [
        "\\..\\escaped.txt",
        "\\\\srv\\x",
        "\\Readme.TXT",
        "\\CON",
        "\\x:y",
        "",
    ] {
        let st = set_info(&mut d, id, FileInformationClass::Rename(rename(to, false)));
        assert_ne!(st, 0, "{to}");
    }
    assert!(root.join("Sub/moved.txt").exists());
    assert!(!root.parent().unwrap().join("escaped.txt").exists());
    // Delete on close.
    assert_eq!(
        set_info(
            &mut d,
            id,
            FileInformationClass::Disposition(disposition(1))
        ),
        0
    );
    assert_eq!(close(&mut d, id), 0);
    assert!(!root.join("Sub/moved.txt").exists());
    // Directories: create, refuse to delete non-empty, delete empty.
    let (st, dir) = open(
        &mut d,
        "\\Made",
        WRITE,
        CreateDisposition::FILE_CREATE,
        CreateOptions::FILE_DIRECTORY_FILE,
    );
    assert_eq!(st, 0);
    assert!(root.join("Made").is_dir());
    assert_eq!(close(&mut d, dir), 0);
    let (_, sub) = open(
        &mut d,
        "\\Sub",
        0x1_0000 | 0x80,
        CreateDisposition::FILE_OPEN,
        CreateOptions::FILE_DIRECTORY_FILE,
    );
    assert_eq!(
        set_info(
            &mut d,
            sub,
            FileInformationClass::Disposition(disposition(1))
        ),
        0
    );
    assert_eq!(close(&mut d, sub), nt::DIRECTORY_NOT_EMPTY);
    assert!(root.join("Sub").is_dir());
    let (_, made) = open(
        &mut d,
        "\\Made",
        0x1_0000 | 0x80,
        CreateDisposition::FILE_OPEN,
        CreateOptions::FILE_DIRECTORY_FILE,
    );
    assert_eq!(
        set_info(
            &mut d,
            made,
            FileInformationClass::Disposition(disposition(1))
        ),
        0
    );
    assert_eq!(close(&mut d, made), 0);
    assert!(!root.join("Made").exists());
    // The root itself is never deleted or renamed.
    assert_eq!(
        open(
            &mut d,
            "\\",
            0x1_0000,
            CreateDisposition::FILE_OPEN,
            CreateOptions::FILE_DIRECTORY_FILE
        )
        .0,
        nt::ACCESS_DENIED
    );
    let (_, r) = open(
        &mut d,
        "\\",
        READ,
        CreateDisposition::FILE_OPEN,
        CreateOptions::FILE_DIRECTORY_FILE,
    );
    assert_eq!(
        set_info(&mut d, r, FileInformationClass::Disposition(disposition(1))),
        nt::ACCESS_DENIED
    );
    assert_eq!(
        set_info(
            &mut d,
            r,
            FileInformationClass::Rename(rename("\\gone", false))
        ),
        nt::ACCESS_DENIED
    );
    assert!(root.exists());
}

#[test]
fn write_caps() {
    let (_root, mut d) = fsys("caps", true);
    let (_, id) = open(
        &mut d,
        "\\cap.bin",
        WRITE,
        CreateDisposition::FILE_CREATE,
        NONE,
    );
    assert_eq!(write(&mut d, id, MAX_FILE_SIZE, b"x").0, nt::DISK_FULL);
    assert_eq!(
        write(&mut d, id, u64::MAX, b"x").0,
        nt::DISK_FULL,
        "offset overflow"
    );
    assert_eq!(
        write(&mut d, id, 0, &vec![0u8; MAX_WRITE + 1]).0,
        nt::DISK_FULL
    );
    assert_eq!(
        set_info(
            &mut d,
            id,
            FileInformationClass::EndOfFile(eof(MAX_FILE_SIZE as i64 + 1))
        ),
        nt::DISK_FULL
    );
    assert_eq!(
        set_info(&mut d, id, FileInformationClass::EndOfFile(eof(-1))),
        nt::INVALID_PARAMETER
    );
    d.written_total = MAX_SESSION_WRITE - 1;
    assert_eq!(write(&mut d, id, 0, b"ab").0, nt::DISK_FULL);
    assert_eq!(write(&mut d, id, 0, b"a").0, 0);
}

#[test]
fn handle_count_is_capped() {
    let (_root, mut d) = fsys("handles", false);
    let mut ids = Vec::new();
    for _ in 0..MAX_HANDLES {
        let (st, id) = open(
            &mut d,
            "\\Readme.TXT",
            READ,
            CreateDisposition::FILE_OPEN,
            NONE,
        );
        assert_eq!(st, 0);
        ids.push(id);
    }
    assert_eq!(
        open(
            &mut d,
            "\\Readme.TXT",
            READ,
            CreateDisposition::FILE_OPEN,
            NONE
        )
        .0,
        nt::TOO_MANY_OPENED_FILES
    );
    assert_eq!(close(&mut d, ids[0]), 0);
    assert_eq!(
        open(
            &mut d,
            "\\Readme.TXT",
            READ,
            CreateDisposition::FILE_OPEN,
            NONE
        )
        .0,
        0
    );
}

fn list(d: &mut DriveFs, dir: u32, pattern: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut first = true;
    loop {
        let r = d.handle(ServerDriveIoRequest::ServerDriveQueryDirectoryRequest(
            ServerDriveQueryDirectoryRequest {
                device_io_request: DeviceIoRequest {
                    minor_function: MinorFunction::IRP_MN_QUERY_DIRECTORY,
                    ..io(MajorFunction::DirectoryControl, dir, 13)
                },
                file_info_class_lvl: FileInformationClassLevel::FILE_BOTH_DIRECTORY_INFORMATION,
                initial_query: first as u8,
                path: pattern.to_string(),
            },
        ));
        first = false;
        let RdpdrPdu::ClientDriveQueryDirectoryResponse(r) = &r[0] else {
            panic!()
        };
        match &r.buffer {
            Some(FileInformationClass::BothDirectory(b)) => names.push(b.file_name.clone()),
            None => {
                assert_eq!(u32::from(r.device_io_reply.io_status), nt::NO_MORE_FILES);
                return names;
            }
            _ => panic!(),
        }
        assert!(names.len() < 1000);
    }
}

#[test]
fn lists_directories_with_wildcards() {
    let (root, mut d) = fsys("list", false);
    fs::write(root.join("b.txt"), b"1").unwrap();
    fs::write(root.join("bad<name"), b"1").unwrap_or(());
    fs::write(root.join("trailing."), b"1").unwrap_or(());
    fs::write(root.join("CON"), b"1").unwrap_or(());
    let (_, dir) = open(
        &mut d,
        "\\",
        READ,
        CreateDisposition::FILE_OPEN,
        CreateOptions::FILE_DIRECTORY_FILE,
    );
    let all = list(&mut d, dir, "*");
    assert_eq!(all, vec![".", "..", "b.txt", "Readme.TXT", "Sub"]);
    // Names Windows could not address stay out of the listing.
    assert!(
        !all.iter()
            .any(|n| n.contains('<') || n == "CON" || n.ends_with('.') && n != "." && n != "..")
    );
    assert_eq!(list(&mut d, dir, "\\*.txt"), vec!["b.txt", "Readme.TXT"]);
    assert_eq!(list(&mut d, dir, "readme.txt"), vec!["Readme.TXT"]);
    assert_eq!(list(&mut d, dir, "nomatch*"), Vec::<String>::new());
    // Continuing a handle that never started a query ends the listing.
    let (_, dir2) = open(
        &mut d,
        "\\Sub",
        READ,
        CreateDisposition::FILE_OPEN,
        CreateOptions::FILE_DIRECTORY_FILE,
    );
    let r = d.handle(ServerDriveIoRequest::ServerDriveQueryDirectoryRequest(
        ServerDriveQueryDirectoryRequest {
            device_io_request: io(MajorFunction::DirectoryControl, dir2, 14),
            file_info_class_lvl: FileInformationClassLevel::FILE_NAMES_INFORMATION,
            initial_query: 0,
            path: String::new(),
        },
    ));
    assert_eq!(status_of(&r[0]), nt::NO_MORE_FILES);
    // Listing a file handle fails.
    let (_, f) = open(&mut d, "\\b.txt", READ, CreateDisposition::FILE_OPEN, NONE);
    let r = d.handle(ServerDriveIoRequest::ServerDriveQueryDirectoryRequest(
        ServerDriveQueryDirectoryRequest {
            device_io_request: io(MajorFunction::DirectoryControl, f, 15),
            file_info_class_lvl: FileInformationClassLevel::FILE_NAMES_INFORMATION,
            initial_query: 1,
            path: "*".into(),
        },
    ));
    assert_eq!(status_of(&r[0]), nt::NOT_A_DIRECTORY);
}

#[test]
fn answers_information_and_volume_queries() {
    let (_root, mut d) = fsys("info", false);
    let (_, f) = open(
        &mut d,
        "\\Readme.TXT",
        READ,
        CreateDisposition::FILE_OPEN,
        NONE,
    );
    let q = |d: &mut DriveFs, lvl: FileInformationClassLevel| {
        d.handle(ServerDriveIoRequest::ServerDriveQueryInformationRequest(
            ServerDriveQueryInformationRequest {
                device_io_request: io(MajorFunction::QueryInformation, f, 16),
                file_info_class_lvl: lvl,
            },
        ))
    };
    let r = q(&mut d, FileInformationClassLevel::FILE_STANDARD_INFORMATION);
    let RdpdrPdu::ClientDriveQueryInformationResponse(r) = &r[0] else {
        panic!()
    };
    let Some(FileInformationClass::Standard(st)) = &r.buffer else {
        panic!()
    };
    assert_eq!(st.end_of_file, 5);
    assert_eq!(st.directory, Boolean::False);
    let r = q(&mut d, FileInformationClassLevel::FILE_BASIC_INFORMATION);
    let RdpdrPdu::ClientDriveQueryInformationResponse(r) = &r[0] else {
        panic!()
    };
    let Some(FileInformationClass::Basic(b)) = &r.buffer else {
        panic!()
    };
    // Files of a read-only share carry the read-only attribute.
    assert!(
        b.file_attributes
            .contains(FileAttributes::FILE_ATTRIBUTE_READONLY)
    );
    assert!(b.last_write_time > FILETIME_UNIX_EPOCH);
    assert_eq!(
        status_of(&q(&mut d, FileInformationClassLevel::from(99))[0]),
        nt::NOT_SUPPORTED
    );
    for lvl in [
        FileSystemInformationClassLevel::FILE_FS_VOLUME_INFORMATION,
        FileSystemInformationClassLevel::FILE_FS_SIZE_INFORMATION,
        FileSystemInformationClassLevel::FILE_FS_ATTRIBUTE_INFORMATION,
        FileSystemInformationClassLevel::FILE_FS_FULL_SIZE_INFORMATION,
        FileSystemInformationClassLevel::FILE_FS_DEVICE_INFORMATION,
    ] {
        let r = d.handle(
            ServerDriveIoRequest::ServerDriveQueryVolumeInformationRequest(
                ServerDriveQueryVolumeInformationRequest {
                    device_io_request: io(MajorFunction::QueryVolumeInformation, f, 17),
                    fs_info_class_lvl: lvl,
                },
            ),
        );
        assert_eq!(status_of(&r[0]), 0);
        // Everything we answer can be encoded.
        let pdu = r.into_iter().next().unwrap();
        encode_vec(&pdu).unwrap();
    }
}

#[test]
fn refuse_answers_every_request_kind() {
    let (_root, mut d) = fsys("refuse", true);
    let reqs = vec![
        create_req("\\x", READ, CreateDisposition::FILE_OPEN, NONE),
        ServerDriveIoRequest::DeviceCloseRequest(
            ironrdp::rdpdr::pdu::efs::DeviceCloseRequest::decode(io(MajorFunction::Close, 1, 1)),
        ),
        ServerDriveIoRequest::DeviceReadRequest(DeviceReadRequest {
            device_io_request: io(MajorFunction::Read, 1, 2),
            length: 1,
            offset: 0,
        }),
        ServerDriveIoRequest::DeviceWriteRequest(DeviceWriteRequest {
            device_io_request: io(MajorFunction::Write, 1, 3),
            offset: 0,
            write_data: vec![1],
        }),
    ];
    for r in reqs {
        let out = refuse(r, nt::NO_SUCH_DEVICE);
        assert_eq!(out.len(), 1);
        assert_eq!(status_of(&out[0]), nt::NO_SUCH_DEVICE);
        encode_vec(&out[0]).unwrap();
    }
    // Still working when requests run against a live share.
    let _ = &mut d;
}

// ------------------------------------------------------- wire-level exchange

fn utf16z(s: &str) -> Vec<u8> {
    let mut v: Vec<u8> = s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    v.extend_from_slice(&[0, 0]);
    v
}

/// The server side of the channel: builds the PDUs a Windows RDPDR server would send.
struct FakeServer {
    comp: u32,
}

impl FakeServer {
    fn irp(&mut self, device: u32, file: u32, major: u32, minor: u32, body: &[u8]) -> Vec<u8> {
        self.comp += 1;
        let mut v = Vec::new();
        v.extend_from_slice(&0x4472u16.to_le_bytes());
        v.extend_from_slice(&0x4952u16.to_le_bytes());
        for x in [device, file, self.comp, major, minor] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        v.extend_from_slice(body);
        v
    }

    fn create(&mut self, path: &str, access: u32, disp: u32, options: u32) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&access.to_le_bytes());
        b.extend_from_slice(&0u64.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&7u32.to_le_bytes());
        b.extend_from_slice(&disp.to_le_bytes());
        b.extend_from_slice(&options.to_le_bytes());
        let p = utf16z(path);
        b.extend_from_slice(&(p.len() as u32).to_le_bytes());
        b.extend_from_slice(&p);
        self.irp(1, 0, 0, 0, &b)
    }
}

/// Splits a response into (device, completion, status, rest after the 12 byte IoCompletion).
fn completion(bytes: &[u8]) -> (u32, u32, u32, Vec<u8>) {
    assert_eq!(u16::from_le_bytes(bytes[0..2].try_into().unwrap()), 0x4472);
    assert_eq!(
        u16::from_le_bytes(bytes[2..4].try_into().unwrap()),
        0x4943,
        "IoCompletion"
    );
    let u = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
    (u(4), u(8), u(12), bytes[16..].to_vec())
}

fn run(ch: &mut Rdpdr, pdu: &[u8]) -> Vec<Vec<u8>> {
    ch.process(pdu)
        .unwrap()
        .iter()
        .map(|m| m.encode_unframed_pdu().unwrap())
        .collect()
}

#[test]
fn full_exchange_with_a_fake_server() {
    let root = tmp("wire");
    fs::write(root.join("hello.txt"), b"Hello from Aurora").unwrap();
    fs::create_dir_all(root.join("docs")).unwrap();
    let slot: SharedDrive = Arc::new(Mutex::new(Some(DriveFs::new(
        Share::new(&root, false).unwrap(),
        "Shared Docs".into(),
        1,
    ))));
    let mut ch = make_channel(&slot, "AURORA");
    let mut srv = FakeServer { comp: 0 };

    // 1. Server announce -> client announce reply + client name.
    let mut announce = vec![0x72, 0x44, 0x6E, 0x49];
    announce.extend_from_slice(&1u16.to_le_bytes()); // major
    announce.extend_from_slice(&12u16.to_le_bytes()); // minor
    announce.extend_from_slice(&42u32.to_le_bytes()); // client id
    let out = run(&mut ch, &announce);
    assert_eq!(out.len(), 2);
    assert_eq!(&out[0][2..4], &0x4343u16.to_le_bytes(), "CLIENTID_CONFIRM");
    assert_eq!(&out[1][2..4], &0x434Eu16.to_le_bytes(), "CLIENT_NAME");

    // 2. Client id confirm from the server, then logon: the drive is announced.
    let mut confirm = vec![0x72, 0x44, 0x43, 0x43];
    confirm.extend_from_slice(&1u16.to_le_bytes());
    confirm.extend_from_slice(&12u16.to_le_bytes());
    confirm.extend_from_slice(&42u32.to_le_bytes());
    assert!(run(&mut ch, &confirm).is_empty());
    let out = run(&mut ch, &[0x72, 0x44, 0x4C, 0x55]);
    assert_eq!(out.len(), 1);
    assert_eq!(
        &out[0][2..4],
        &0x4441u16.to_le_bytes(),
        "DEVICELIST_ANNOUNCE"
    );
    let announced = &out[0];
    let device_type = u32::from_le_bytes(announced[8..12].try_into().unwrap());
    let device_id = u32::from_le_bytes(announced[12..16].try_into().unwrap());
    assert_eq!((device_type, device_id), (8, 1), "filesystem device 1");
    let name = b"Shared Docs\0";
    assert!(
        announced.windows(name.len()).any(|w| w == name),
        "name is announced"
    );
    assert!(
        !announced
            .windows(root.to_string_lossy().len())
            .any(|w| w == root.to_string_lossy().as_bytes()),
        "the local path is never sent"
    );

    // 3. Server accepts the device.
    let mut reply = vec![0x72, 0x44, 0x72, 0x64];
    reply.extend_from_slice(&1u32.to_le_bytes());
    reply.extend_from_slice(&0u32.to_le_bytes());
    assert!(run(&mut ch, &reply).is_empty());

    // 4. Open, read, close a file.
    let out = run(&mut ch, &srv.create("\\hello.txt", READ, 1, 0x40));
    let (dev, _comp, status, rest) = completion(&out[0]);
    assert_eq!((dev, status), (1, 0));
    let file_id = u32::from_le_bytes(rest[0..4].try_into().unwrap());
    assert_ne!(file_id, 0);

    let mut body = Vec::new();
    body.extend_from_slice(&64u32.to_le_bytes());
    body.extend_from_slice(&6u64.to_le_bytes());
    body.extend_from_slice(&[0u8; 20]);
    let out = run(&mut ch, &srv.irp(1, file_id, 3, 0, &body));
    let (_, _, status, rest) = completion(&out[0]);
    assert_eq!(status, 0);
    let len = u32::from_le_bytes(rest[0..4].try_into().unwrap()) as usize;
    assert_eq!(&rest[4..4 + len], b"from Aurora");

    let out = run(&mut ch, &srv.irp(1, file_id, 2, 0, &[0u8; 32]));
    assert_eq!(completion(&out[0]).2, 0);

    // 5. List the root.
    let out = run(&mut ch, &srv.create("\\", READ, 1, 0x01));
    let dir_id = u32::from_le_bytes(completion(&out[0]).3[0..4].try_into().unwrap());
    let mut q = Vec::new();
    q.extend_from_slice(&3u32.to_le_bytes()); // FileBothDirectoryInformation
    q.push(1); // initial query
    let pat = utf16z("*");
    q.extend_from_slice(&(pat.len() as u32).to_le_bytes());
    q.extend_from_slice(&[0u8; 23]);
    q.extend_from_slice(&pat);
    let mut seen = Vec::new();
    for _ in 0..10 {
        let out = run(&mut ch, &srv.irp(1, dir_id, 0x0c, 1, &q));
        let (_, _, status, rest) = completion(&out[0]);
        q[4] = 0;
        if status != 0 {
            assert_eq!(status, nt::NO_MORE_FILES);
            break;
        }
        // BothDirectoryInformation: name length at offset 60, name at 93.
        let buf = &rest[4..];
        let nlen = u32::from_le_bytes(buf[60..64].try_into().unwrap()) as usize;
        let name: Vec<u16> = buf[93..93 + nlen]
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        seen.push(String::from_utf16(&name).unwrap());
    }
    assert_eq!(seen, vec![".", "..", "docs", "hello.txt"]);

    // 6. A traversal attempt and a write on the read-only share.
    let out = run(&mut ch, &srv.create("\\..\\..\\etc\\passwd", READ, 1, 0x40));
    assert_eq!(completion(&out[0]).2, nt::ACCESS_DENIED);
    let out = run(&mut ch, &srv.create("\\evil.txt", WRITE, 2, 0x40));
    assert_eq!(completion(&out[0]).2, nt::ACCESS_DENIED);
    assert!(!root.join("evil.txt").exists());
    assert!(slot.lock().unwrap().as_ref().unwrap().activity.denied >= 2);

    // 7. Disconnecting the share: requests are answered as "no such device", files closed.
    let id_before = slot.lock().unwrap().as_ref().unwrap().device_id();
    *slot.lock().unwrap() = None;
    let out = run(&mut ch, &srv.create("\\hello.txt", READ, 1, 0x40));
    assert_eq!(completion(&out[0]).2, nt::NO_SUCH_DEVICE);
    assert_eq!(id_before, 1);

    // 8. Requests for devices we never announced are ignored.
    let mut s2 = FakeServer { comp: 100 };
    let mut pdu = s2.create("\\hello.txt", READ, 1, 0x40);
    pdu[4..8].copy_from_slice(&77u32.to_le_bytes());
    assert!(run(&mut ch, &pdu).is_empty());

    // Garbage never panics.
    for junk in [
        &[][..],
        &[1, 2, 3][..],
        &[0x72, 0x44, 0x52, 0x49, 0, 0][..],
        &[0xff; 64][..],
    ] {
        let _ = ch.process(junk);
    }
}

#[test]
fn truncated_irps_are_errors_not_panics() {
    let root = tmp("trunc");
    let slot: SharedDrive = Arc::new(Mutex::new(Some(DriveFs::new(
        Share::new(&root, true).unwrap(),
        "T".into(),
        1,
    ))));
    let mut ch = make_channel(&slot, "A");
    let mut srv = FakeServer { comp: 0 };
    let full = srv.create("\\a.txt", WRITE, 2, 0);
    for cut in 0..full.len() {
        let _ = ch.process(&full[..cut]);
    }
    // A path length field larger than the packet.
    let mut bad = full.clone();
    let off = 4 + 20 + 4 + 8 + 4 + 4 + 4 + 4;
    bad[off..off + 4].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
    assert!(ch.process(&bad).is_err());
    assert!(!root.join("a.txt").exists());
}

#[test]
fn announce_and_remove_messages_encode() {
    let slot: SharedDrive = Arc::new(Mutex::new(None));
    let mut ch = make_channel(&slot, "A");
    let a = ch.add_drive(5, "Zusatz".into());
    let bytes = encode_vec(&RdpdrPdu::ClientDeviceListAnnounce(a)).unwrap();
    assert_eq!(&bytes[2..4], &0x4441u16.to_le_bytes());
    let r = ch.remove_device(5).unwrap();
    let bytes = encode_vec(&RdpdrPdu::ClientDeviceListRemove(r)).unwrap();
    assert_eq!(&bytes[2..4], &0x444Du16.to_le_bytes());
    assert!(ch.remove_device(5).is_none());
}
