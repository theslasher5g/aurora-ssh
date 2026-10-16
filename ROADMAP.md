# Roadmap

Planned work that is not implemented yet. Released changes are in [CHANGELOG.md](CHANGELOG.md).

## RDP file transfer – next steps

Dropping files onto an RDP session and saving files copied on the host work through the
clipboard channel (see CHANGELOG). Still open:

- Drive redirection (`rdpdr`): show a local folder as a drive in the Windows Explorer. Needs a
  newer IronRDP with `ironrdp-rdpdr` file-system support.
- Manual tests on Windows 10/11 and Windows Server 2019/2022: files > 4 GB, many small files,
  cancel in the middle. xrdp only takes plain files and needs its FUSE mount for pastes.

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
