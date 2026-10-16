# Roadmap

Planned work that is not implemented yet. Released changes are in [CHANGELOG.md](CHANGELOG.md).

## Planned: drop files onto an RDP session

**Goal:** drag files (and folders) from the local desktop onto an embedded RDP session and
have them land on the Windows machine – like copy & paste between two Windows PCs.

### How

RDP moves files through the clipboard channel (`cliprdr`): the client announces a file list
(`FileGroupDescriptorW`), Windows asks for each file's size and then its content in chunks
(`FileContentsRequest` by range). `ironrdp-cliprdr` 0.7, which the RDP helper already uses for
text, supports this (`PackedFileList`, `FileContentsRequest`, locked lists by `clipDataId`).

1. **UI (`src/ui/rdp.rs`)** – while files hover over the RDP pane, show a drop overlay
   ("Loslassen zum Kopieren auf <host>"). On drop, send the paths to the helper together with
   the drop position.
2. **Protocol (helper stdio)** – new command `OfferFiles { paths }`. The helper walks folders
   (relative names, sizes, timestamps), rejects symlinks that leave the dropped tree, and
   announces the list as `FileGroupDescriptorW` (the text format stays available).
3. **Serving content (`crates/aurora-rdp`)** – answer `FileContentsRequest`: `SIZE` from the
   list, `RANGE` by reading the file at the requested offset (chunked, never the whole file in
   memory). Files are opened read-only when Windows asks, not at drop time.
4. **Paste** – after the announce, move the remote pointer to the drop position, click once
   and send `Ctrl+V`, so the files land in the Explorer window or desktop under the cursor.
   A toolbar button "Dateien einfügen" covers targets that ignore the click.
5. **Progress** – count served bytes per file and show a small progress badge in the RDP pane
   (same style as the terminal upload badge); errors as a toast.

### Limits and safety

- Only works when clipboard redirection is enabled on the server (it is by default) and the
  host setting "Zwischenablage" is on.
- Windows decides where the files go (the focused Explorer window); without a target window
  nothing is written.
- Paths are never sent to the server, only names relative to the dropped folder; `..` and
  absolute names are refused before announcing.
- Big transfers: RDP clipboard copies are slower than SFTP. The overlay hints at the file
  browser for Windows hosts with OpenSSH.

### Later

- The other direction: copy files on Windows, paste them into the Aurora file browser.
- Drive redirection (`rdpdr`): show a local folder as a drive in the Windows Explorer. Needs a
  newer IronRDP with `ironrdp-rdpdr` file-system support.

### Tests

- Unit: file list building (folders, names with umlauts, refused symlinks and `..`), range
  reads at file end and beyond.
- Manual: Windows 10/11 and Windows Server 2019/2022, xrdp (expected: no file paste), files
  > 4 GB, many small files, cancel in the middle.

## Ideas

| Idea | What it gives you | Effort |
|---|---|---|
| **Proxmox integration** | Add a Proxmox host once and see its VMs/containers: status, start/stop, console (SSH/noVNC), resources | large |
| **Run on many hosts** | Run a command or snippet on a whole group in the background and see a result table (exit code, output per host) | medium |
| **Wake-on-LAN** | "Wake up" button for hosts that are offline (MAC address in the host) | small |
| **Persistent sessions** | Optional `tmux`/`screen` per host: reconnect lands in the same shell with its running jobs | small |
| **Session recording** | Record a terminal session (asciinema format) and replay or export it, e.g. for documentation | medium |
| **Live host stats** | Small CPU/RAM/disk graph per host in the list and a detail panel, from the facts Aurora already collects | medium |
| **Hardware keys & agent** | Use the system SSH agent, FIDO2 keys (YubiKey) and SSH certificates | medium |
| **Encrypted sync** | Keep hosts, snippets and keys in sync between PCs via WebDAV, S3 or a Git repository (end-to-end encrypted, like the export) | large |
| **Open files at the terminal's folder** | One click in the terminal header opens the file browser in the current directory | small |
