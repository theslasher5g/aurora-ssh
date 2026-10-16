# Roadmap

Planned work that is not implemented yet. Released changes are in [CHANGELOG.md](CHANGELOG.md).

## RDP file transfer – next steps

Dropping files onto an RDP session and saving files copied on the host work through the
clipboard channel (see CHANGELOG). Still open:

- Drive redirection (`rdpdr`): show a local folder as a drive in the Windows Explorer. Needs a
  newer IronRDP with `ironrdp-rdpdr` file-system support.
- Manual tests on Windows 10/11 and Windows Server 2019/2022: files > 4 GB, many small files,
  cancel in the middle. xrdp only takes plain files and needs its FUSE mount for pastes.

## Proxmox – next steps

- noVNC/SPICE console for VMs without SSH, Windows VMs as RDP targets.
- Proxmox servers in the encrypted export/import.

## Ideas

| Idea | What it gives you | Effort |
|---|---|---|
| **Run on many hosts** | Run a command or snippet on a whole group in the background and see a result table (exit code, output per host) | medium |
| **Wake-on-LAN** | "Wake up" button for hosts that are offline (MAC address in the host) | small |
| **Live host stats** | Small CPU/RAM/disk graph per host in the list and a detail panel, from the facts Aurora already collects | medium |
| **Hardware keys & agent** | Use the system SSH agent, FIDO2 keys (YubiKey) and SSH certificates | medium |
| **Encrypted sync** | Keep hosts, snippets and keys in sync between PCs via WebDAV, S3 or a Git repository (end-to-end encrypted, like the export) | large |
| **Log alerts** | Notify when a followed log prints an error pattern while the session is in the background | small |
