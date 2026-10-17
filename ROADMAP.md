# Roadmap

Planned work that is not implemented yet. Released changes are in [CHANGELOG.md](CHANGELOG.md).

## RDP file transfer – next steps

Dropping files onto an RDP session and saving files copied on the host work through the
clipboard channel (see CHANGELOG). Still open:

- Drive redirection (`rdpdr`): show a local folder as a drive in the Windows Explorer. Needs a
  newer IronRDP with `ironrdp-rdpdr` file-system support.
- Manual tests on Windows 10/11 and Windows Server 2019/2022: files > 4 GB, many small files,
  cancel in the middle. xrdp only takes plain files and needs its FUSE mount for pastes.

## Proxmox – next ideas

| Idea | What it gives you | Effort |
|---|---|---|
| **Live graphs** | CPU/RAM/network/disk sparkline per guest and node from `rrddata`, bigger chart in a detail panel | medium |
| **Clone & new from template** | Full or linked clone with the next free VMID; cloud-init user, IP and an SSH key straight from the Aurora keychain; afterwards "connect" in one click | medium |
| **Migrate by drag & drop** | Drag a guest onto another node heading → (live) migration with task progress | medium |
| **Quick hardware edit** | Cores, RAM, disk resize, NIC/VLAN in a small sheet | medium |
| **Task log** | Running and recent cluster tasks (backups, migrations, failed jobs) with their log | small |
| **Cluster & storage overview** | Storage usage, ZFS/Ceph health, HA state and quorum at a glance | medium |
| **Batch actions** | Select several guests (or a tag like `web`) and start, shut down, snapshot or back them up together | small |
| **Node shell without SSH** | `termproxy` over the API in an Aurora terminal tab – works even without SSH keys on the node | medium |
| **Alerts** | Toast/notification when a guest goes down or a backup fails | small |
| **Proxmox Backup Server** | Browse PBS snapshots and restore single files | large |
| **Node updates** | Pending `apt` updates per node with "update" via SSH, one node after the other | small |
| **Export/import** | Proxmox servers in the encrypted export | small |

## Ideas

| Idea | What it gives you | Effort |
|---|---|---|
| **Run on many hosts** | Run a command or snippet on a whole group in the background and see a result table (exit code, output per host) | medium |
| **Wake-on-LAN** | "Wake up" button for hosts that are offline (MAC address in the host) | small |
| **Live host stats** | Small CPU/RAM/disk graph per host in the list and a detail panel, from the facts Aurora already collects | medium |
| **Hardware keys & agent** | Use the system SSH agent, FIDO2 keys (YubiKey) and SSH certificates | medium |
| **Encrypted sync** | Keep hosts, snippets and keys in sync between PCs via WebDAV, S3 or a Git repository (end-to-end encrypted, like the export) | large |
| **Log alerts** | Notify when a followed log prints an error pattern while the session is in the background | small |
