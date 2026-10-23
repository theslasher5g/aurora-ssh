<div align="center">

# 🗺 Roadmap

What is planned, what is being looked at and what is only an idea.
Released work is in the [CHANGELOG](CHANGELOG.md).

**Status:** 🟢 next · 🟡 planned · 🔵 idea · ⚪ needs a decision

</div>

## 🟢 Next

| Item | What it gives you | Effort |
|---|---|---|
| **Web page inside the Aurora window** | Show a web interface entry in the tab itself (embedded view) instead of a separate window – the missing half of the Royal TS / RDM experience. The tab is already prepared for it | large |
| **Test on Windows & macOS** | The web helper, the WSL Docker fallback, key drag & drop and imports were only compiled there; real runs on Windows 10/11, Windows Server and macOS (Intel + Apple Silicon) | medium |
| **Certificate pinning on macOS** | Pin self‑signed certificates in the system web view as on Windows and Linux (needs a native hook) | medium |
| **Real exports for the importers** | Check the Royal TS / RDM / mRemoteNG parsers against real exports, read `.rtsz` archives, native file dialog | medium |
| **Auto‑login: spend the submit on the click** | Count the single automatic submit when the form is really submitted, not when the script arms itself | small |

## 🟡 Planned

### Large inventories
- Cache the inventory page for 20 000+ hosts (today it rebuilds its rows every frame) and scope the Proxmox / keychain pages to the customer focus.
- Keyboard navigation in the table view; manual ordering of groups.
- **Dynamic folders**: hosts from Proxmox, the inventory or a CSV/URL appear automatically as entries.

### Servers
- **Run on many hosts** – run a command or snippet on a whole group and see a result table (exit code, output per host).
- **Live host stats** – small CPU/RAM/disk graphs per host and a detail panel.
- **Facts for Windows without SSH** – WinRM / PowerShell remoting for inventory, services and firewall rules.

### Proxmox
- File browser for PBS archives (`.pxar`) with single‑file restore.
- Prune / verify on the Proxmox Backup Server (today read‑only with a prune preview).
- Migrate a whole selection at once; cloud‑init beyond `ipconfig0`.

### RDP
- **Drive redirection** (`rdpdr`): show a local folder as a drive in Windows Explorer – needs a newer IronRDP with file‑system support.
- Manual tests with files > 4 GB, many small files and cancelling; xrdp needs its FUSE mount for pastes.

## 🔵 Ideas

| Idea | What it gives you | Effort |
|---|---|---|
| **Hardware keys & agent** | Use the system SSH agent and FIDO2 keys (YubiKey) | medium |
| **Encrypted sync** | Keep hosts, snippets and keys in sync between PCs via WebDAV, S3 or a Git repository (end‑to‑end encrypted, like the export) | large |
| **Teams** | Shared entries and a shared vault with roles | large |
| **Log alerts** | Notify when a followed log prints an error pattern while the session is in the background | small |
| **Config diff** | Compare a config file between two hosts or two points in time | medium |

## ⚪ Open questions

- Should web pages open **only** in the tab once embedding exists, or keep the separate window as an option?

## ✅ Recently done

See [CHANGELOG.md](CHANGELOG.md): web interfaces, importers, large‑inventory navigation, SSH key drag & drop,
inventory detail view and documentation export, command blocks, transfer queue, built‑in editor,
password managers, reconnect, custom themes.
