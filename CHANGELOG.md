# Changelog

## Unreleased

### New
- **Password managers:** take passwords (and SSH keys for identities) from Bitwarden (`bw`),
  1Password (`op`), KeePassXC (`keepassxc-cli`) or Proton Pass (`pass-cli`) instead of the
  Aurora vault. Settings -> Security -> Password managers; hosts and identities pick an item
  (only a reference is stored, never the secret). Secrets are fetched right before connecting
  (SSH, RDP, VNC, Telnet, SFTP, tunnels, jump hosts), live in memory only and are wiped.
- **Password prompt:** *Choose from keychain* - pick a stored identity or a password-manager
  item (search, arrow keys, Enter / double-click), optionally remember it for the host.

## 0.7.1

### New
- **Local terminal:** your own shell on this PC in a tab – `Ctrl+Shift+T`, *New → Local
  terminal* or the command palette (bash/zsh on Linux and macOS, PowerShell or cmd on Windows).
- **Proxmox VM console that works:** a host already saved for the VM is used with its user and
  key; otherwise a small sheet asks for the login (cloud-init user prefilled, key picker,
  *save as host*). VMs without an address open the serial console over the API (no SSH host
  needed) or noVNC in the browser; containers open over the API when no node SSH host is set.
- **Small windows:** headers, filter rows and list rows reflow instead of overlapping, hover
  buttons no longer cover names, columns and buttons hide step by step, and the window has a
  minimum size.

### Fixes
- The *Update available* button opens *Data & updates* instead of *Appearance*.
- Proxmox *Reboot* asks for confirmation like *Stop* and *Shut down*.

## 0.7.0

### Proxmox
- **Live graphs:** CPU sparkline per running guest; a *Statistics* tab per guest (CPU, RAM,
  network, disk – hour/day/week) and node statistics (CPU with IO wait, RAM, network, load).
- **Clone & new from template:** full or linked clone with the next free VMID, target node
  and storage; cloud-init user, IP and an SSH key straight from the keychain; *Connect*
  afterwards saves the guest as a host and opens a session.
- **Migration by drag & drop:** drag a guest onto another node heading (live for running VMs,
  restart mode for containers), also from the row menu.
- **Hardware sheet:** cores, sockets, RAM, balloon, swap, disk growth, bridge/VLAN per NIC –
  with a live diff and a note for changes that need a reboot.
- **Batch actions:** select guests (Ctrl/Shift or by tag) and start, shut down, reboot,
  snapshot or back them up together, with a result list.
- **Tasks:** all cluster tasks with filter, live log and cancel.
- **Overview:** quorum, nodes, storage usage, ZFS, HA and Ceph per server with warnings.
- **Alerts:** a background watcher reports guests that went down unexpectedly, offline nodes,
  failed tasks and full storage (toast + bell with a list); switchable per server.
- **Node shell without SSH** via `termproxy` in an Aurora terminal tab.
- **Node updates:** pending packages per node, refresh the package lists, upgrade over SSH.
- **Proxmox Backup Server:** datastores, snapshots (verify state, protected, size), prune
  preview and viewing/saving `.blob` files.
- Proxmox and backup servers are part of the encrypted export.

### Terminal
- **Command memory:** commands per host are remembered (encrypted, secrets and
  space-prefixed commands skipped). `Ctrl+R` searches the host's history, `Ctrl+K` shows it in
  the palette.
- **Snippet suggestions:** a command run three times with different values is offered as a
  snippet with `{{variables}}`.
- **Secret guard:** pasting private keys, cloud/API tokens, JWTs or passwords asks first
  (masked); on PROD also multi-line pastes. Middle click and a context-menu *Paste* were added.
- **Time travel:** scroll back through the last 30 minutes of a terminal like a video –
  timeline with activity, play/pause, copy the past screen; also for recordings.

### SSH & network
- **Short-lived SSH certificates:** Aurora can act as a small CA (key in the vault). Hosts
  can log in with a fresh certificate valid for 1/8/24 h instead of a stored key; one click
  types the server setup line. Falls back to the normal identity if the server refuses.
- **Host key changes double-checked:** a new or changed server key is compared with SSHFP in
  DNS (DNSSEC noted) and with `ssh-keyscan` from the jump host – green, amber or red verdict;
  on red the host name must be typed before trusting. Key history per host.
- **Jump chains & route finder:** up to four jump hosts in a row; if a host can't be reached
  directly, Aurora searches a working route through your saved hosts and offers it.
- **Certificate radar:** checks the TLS certificates of your hosts (open ports from the
  port scan, common TLS ports, SMTP STARTTLS, Proxmox) and warns before they expire – daily in
  the background.
- **Wake-on-LAN:** MAC per host (can be looked up), *Wake up* and *Wake up and connect* –
  also through a jump host.

### Operations
- **Runbooks:** step-by-step procedures (text, optional command with `{{variables}}`,
  confirmation per step) on the Snippets page; run them next to one or more terminals with
  *Run*/*Done*, notes and timing, and get a Markdown report with every step's output.
  Import/export as Markdown.
- **Maintenance windows:** mark a host or group as "in maintenance" for 30 min … 4 h or until
  a time – alerts, toasts and red LEDs are muted, the session shows an amber strip, and Aurora
  asks 5 minutes before the end whether to extend.
- **Diagnostics bundle:** collects system info, logs, services and containers of a host
  (optionally with sudo) into a `.tar.gz`, with an option to mask IPs, MACs, e-mails and host
  names.

## 0.6.2

### New features
- **Proxmox snapshots:** per guest a sheet with the snapshot tree (time, description, RAM),
  create (optionally with RAM for running VMs), roll back and delete – each with
  confirmation and live task status.
- **Proxmox backups:** all backups of a guest from every backup storage (date, size,
  storage, notes, protected), *Backup now* with storage and mode, and restore to a new VMID
  (confirmed by typing it; never overwrites the original).
- **Proxmox consoles:** noVNC in the browser for VMs and containers, SPICE via
  `remote-viewer` (virt-viewer) for VMs with a SPICE display.
- **Proxmox "save as host" with the right IP:** the address is looked up at click time –
  IPv4 first, the server's subnet preferred, docker/bridge/link-local addresses skipped,
  the static config (cloud-init / container net) for stopped guests, otherwise a short
  question. Saving again updates the existing host instead of adding a second one.
- **Port scan per host** (right-click → *Ports scannen…*): from this PC (well-known ports,
  1–1024, all 65535 or your own list like `22, 80, 8000-8100`, with banners and latency) or
  *listening on the server* over SSH (`ss`, `netstat` or `/proc/net/tcp`), which also shows
  services bound to localhost only. One click opens a web port in the browser or creates and
  starts an SSH tunnel to it.
- **Key groups and search:** keys can be sorted into coloured groups (collapsible, drag &
  drop, "move to group" menu); the search finds keys by name, user, type, comment,
  fingerprint or group. Key pickers in the host editor show the groups. Groups are stored in
  the encrypted vault and travel with the export.

### Fixes
- Proxmox: after start/stop the list refreshes 2 s and 6 s later; locking the vault while a
  request is running no longer brings the login ticket back.
- Persistent sessions: every tab to the same host gets its own tmux session; server facts
  are collected again; hosts without tmux/screen or with a non-POSIX login shell (fish,
  Windows) fall back to a plain shell instead of closing; leaving tmux is no longer reported
  as an error.
- Recordings that start in the same second no longer overwrite each other.
- The delete button of own log commands no longer flickers.
- RDP: with several RDP panes only one takes a dropped file (the one under the pointer, or
  the active one); a host that stops answering during a file transfer times out after 15 s
  instead of blocking further transfers; file reads and writes run on their own thread, so the
  picture no longer stutters during big transfers.

## 0.6.1

### New features
- **Proxmox VE:** own page in the sidebar with every VM and container of your Proxmox
  servers, grouped by server and node (status, VM/CT, tags, IP, uptime, vCPU, CPU, RAM).
  Search by name, VMID, tag or IP; filter VMs / containers / running; templates on demand;
  refreshes every 10 s. Start, shut down, reboot, stop (with confirmation) and resume;
  console (`pct enter` / `qm terminal` through the node's SSH host, or SSH to the VM's IP);
  "save as host". Login with an API token (recommended) or user + password; secrets stay in
  the encrypted vault, the server certificate is pinned on first contact. Guests are also
  in the command palette.
- **Logs next to the terminal:** the log viewer docks at the right of an SSH session
  (header button or `Ctrl+Shift+L`), uses the session's login (no second password prompt)
  and can be resized at its edge. Pick a preset, type a file or command, or one of your own.
  The followed source is reopened with the session on the next start.
- **Own log commands:** save a log source with a name from the panel, or manage them under
  Settings → Terminal (for all hosts or one host). They also appear under "View log".
- **Session recording:** record a terminal with the record button (red while recording) or
  every SSH session automatically. Recordings are asciinema files (`.cast`, only what the
  screen shows) in `Documents/Aurora/Recordings`; Settings → Terminal lists them with
  *Play* (replay in a tab, Space pauses) and *Delete*.
- **Persistent sessions:** host editor switch "Persistent session (tmux)": the shell runs in
  tmux (or screen) on the server, a reconnect lands in the same session with its jobs.
- **Open files at the terminal's folder:** a header button opens the file browser in the
  shell's current directory on the same login.
- **Files over RDP:** drop files and folders onto an embedded RDP session to copy them to the
  Windows machine (through the clipboard channel, no SFTP needed). Aurora clicks at the drop
  position and pastes; *Paste files* pastes again. Files the host copied can be saved with
  *Save files from host* into `Downloads/Aurora` (never overwrites). A badge shows progress.
  Follows the host's *Clipboard* setting.
- **PowerShell snippets:** snippets have a shell kind (*Shell* / *PowerShell*). The bolt button
  in an RDP session runs a PowerShell snippet on the Windows host (Win+R and an encoded
  command, placeholders are asked first). PowerShell snippets carry a *PS* tag.
- Terminal settings show a live preview of font, size and colours; the host editor's terminal
  profile has the same preview.
- Scrollback is picked from a list (1,000 to 100,000 lines); the font size has a slider with a
  value field.

## 0.6.0

### New design
- New look across the whole app: calm cards with rounded corners, **Inter** for the UI
  (IBM Plex Mono stays for addresses and the terminal), line icons instead of emoji, filled
  buttons and segmented controls.
- **Sidebar with labels** (Connections, Keys, Files, Snippets, Tunnels, Settings, Help) on the
  start pages; once sessions are open it becomes a slim rail with numbered session tiles
  (status dot, PROD/STAGING stripe, number = `Ctrl+N`).
- **Connections:** large title with count, search field, *Scan network* and *New* in the
  header, protocol filter as a segmented control. Groups are headings with a colour square and
  "x of y online"; their hosts sit in a card with two-line rows (name, PROD pill, favourite,
  address · OS · uptime · disk) and tags on the right (updates, protocol, tags, tunnels).
  *Edit / Files / Connect* appear on hover. Subnet hint and multi-selection bar restyled.
- **Sessions** are rounded panes with a 10 px gap; the active pane in a split has an accent
  frame. New two-line session header: number, name, address, status dot, actions, *split*,
  *full screen*, *own window*.
- **Files (SFTP):** both sides as cards with source picker, folder actions, path bar and
  *Copy →* on the right; rows with folder/file icons, size and date; transfers in their own card.
  The Files start page lists servers as cards.
- **Editors as sheets:** new host, group, key and snippet open as a sheet with grouped form
  rows (label left, field right), segmented protocol and environment pickers, colour dots and
  switches. All dialogs (password, host key, PROD warning, confirm, scan …) use the same frame.
- **Settings in tabs:** Appearance, Terminal, General, Security, Data & updates – each as cards
  with explanations and switches. Theme picker as preview tiles, split into dark and light.
- **Help page** instead of the shortcut popup: all shortcuts grouped in cards with key caps,
  searchable, opens with `F1` / `?`.
- Snippets with `$` preview and placeholder chips, keys with *PW/KEY* badges, command palette
  with type chips and key hints, toasts and status badges restyled.

### New features
- **14 more themes:** One Dark, Monokai, Everforest, Kanagawa, Ayu Mirage, Night Owl, Synthwave,
  Midnight, Matrix, Catppuccin Latte, Rosé Pine Dawn, One Light, Ayu Light, Nord Light
  (26 in total). Every theme is checked for readable contrast.
- **Full screen** (`F11` or the header button): only the session, with *Exit*, a session
  switcher and the session's actions in the top bar.
- **Open a session in its own window** (terminal, files, RDP, VNC, logs) and bring it back
  with the same button or by closing the window.
- **Tunnels in the sidebar:** a menu with every tunnel of every host, open/close in one click.

### Fixes
- Pages no longer share their scroll position (settings or files could open scrolled).
- Groups without matching hosts are hidden while a protocol filter is active.
- Row hover in compact mode no longer highlights two rows at the boundary.

## 0.5.2
- VNC (embedded RFB client), Telnet, Docker/Podman containers and Kubernetes pods as session
  types (on this PC or via an SSH host) with a container browser.
- Live log viewer (`tail -F`, `journalctl`, container logs) with filter, levels and pause.
- Terminal profile per host (colours, font, size) and a global terminal font.
- Split screen: the highlighted pane always gets the keyboard.

## 0.5.1
- macOS build (universal .app/.dmg), English README/SECURITY/packaging texts, UI language
  follows the system by default.

## 0.5.0
- Tunnels (port forwarding), jump host, session restore, edit remote files, multi-select,
  compact list, clearer errors, encrypted export/import.
- English UI, Windows code signing hook, security fixes (private temp dirs, never shell-open
  executable remote files).

## 0.4.0
- Host list with server facts (OS, uptime, disk, updates), rename, subnet groups, network scan,
  SSH key setup.

## 0.3.0
- Quick connect and save, open a group in split, terminal search and zoom, snippet from
  selection, PROD labels with dangerous-command guard, host notes, auto-lock, onboarding,
  shortcut overlay, empty states.

## 0.2.0
- Split screen (drag ports into place), file browser server ⇄ server, more themes, updater.

## 0.1.0
- First release: SSH, SFTP, RDP (embedded via IronRDP) and serial consoles, encrypted keychain,
  snippets, groups, themes; Windows installer/portable and Linux .deb/.tar.gz.
