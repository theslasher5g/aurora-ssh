# Changelog

## 0.10.0

### Releases
- **Signed releases:** the release workflow signs `SHA256SUMS.txt` with an Ed25519 key (secret `RELEASE_SIGNING_KEY`, public key
  compiled into the app) and publishes `SHA256SUMS.txt.sig`; in addition a keyless Sigstore/cosign signature
  (`SHA256SUMS.txt.cosign.bundle`) proves the file was built by this repository's workflow. Without the secrets
  releases are unsigned as before. `packaging/keys/` has the key generator, a verify script and the README.
- **The updater checks the signature** before it trusts the checksums: a release with a signature must verify
  (otherwise the update is discarded); an unsigned release shows a notice and asks "Install anyway". New setting
  *Only install signed updates* (off by default for now).
- **macOS:** optional Developer ID signing (Hardened Runtime, helpers signed inside-out, entitlements file) and
  notarization with stapling of app and .dmg, switched on by repository secrets; ad-hoc signing otherwise
  (`packaging/macos/SIGNING.md`).
- **Windows:** timestamp fallback server, signature verification after signing, documentation of the free options
  SignPath Foundation and Azure Trusted Signing (`packaging/WINDOWS-SIGNING.md`).
- **Linux:** optional detached GPG signature of the .deb (`.deb.asc`) and published public key.
### Web interfaces
- **The page now opens inside the Aurora tab** (like Royal TS / Devolutions RDM) instead of a
  separate window. The `aurora-web` helper creates its web view as a **child window** of the
  Aurora window (X11 window on Linux, `HWND` on Windows) and Aurora tells it the pixel rectangle of
  the tab content whenever it changes: window resize, pane dividers of the split view (2-4 panes,
  next to terminals), HiDPI scaling. Keyboard and clipboard work in the page (click into it;
  it also takes the focus when the tab appears); a click anywhere in Aurora gives the keyboard back.
- **Never on top of Aurora:** a native child window cannot be covered by egui, so the page is hidden
  while another tab or pane is shown, while a menu, popup, dialog or the command palette overlaps
  it, while the vault is locked or the window is minimized, and in popped-out windows. Closing
  the tab closes the page. Profile isolation, auto-login, certificate pinning, origin rules and
  downloads are unchanged (same helper protocol, same checks in the helper).
- **Embedded or own window:** *Settings -> Web interfaces -> Display* (default: in the tab). Every web
  tab has *Open in own window* / *Embed in the tab* (reopens the page), and *Details* shows the
  address, certificate and pin. Where embedding is impossible - Wayland and macOS (no
  cross-process child windows there) - or fails, the helper automatically uses its own window as before.
### Servers
- **Run on many hosts:** run a command or a saved snippet (with `{{placeholders}}`, asked once) on a
  selection, a group or the result of a search/filter – in parallel (default 8 at a time, per-host
  time limit, cancel). The result is a table (host, status ok / failed / timeout / unreachable /
  skipped, exit code, duration, first lines of output); click a row for the full output; filter by
  status and search in the results; *Run the failed ones again*; export as Markdown, CSV, JSON or a
  printable HTML page. Entry points: *Run command…* in the selection bar and the host menu, the
  group menu, the command palette. Safety: a confirmation lists the number of hosts and the command;
  **PROD hosts need an extra confirmation, a dangerous command on PROD needs the word typed**; hosts in
  a maintenance window are skipped unless you opt in; secret-looking placeholders are masked in
  results and exports; optional `sudo -n`; output is capped.
- **Live host statistics:** *Live-Statistik…* in the host menu opens CPU, RAM/swap, load, network
  and disk graphs (10 minutes of history, sampled every 3 s over one SSH channel that closes with the
  sheet). Optional tiny CPU/RAM graphs in the host list rows (Settings → *Mini-Graphen*, off by
  default). Hosts without `/proc` (BSD, Windows OpenSSH) show "not supported".
- **Windows without SSH through WinRM:** host form section *WinRM* for RDP hosts (HTTPS 5986 or HTTP
  5985, NTLM or Basic over HTTPS, credentials from the host's identity, certificate pinned on first
  use). *Update facts* in the inventory (and *Fakten per WinRM aktualisieren* in the host menu) collects
  OS, uptime, disks, CPU, RAM, addresses, pending updates, services, listening ports and firewall
  rules into the inventory; info snippets run as PowerShell; *Run on many hosts* works with PowerShell on
  WinRM hosts. Implemented in Aurora itself (NTLMv2, WS-Management); **tested against a mock server
  and the MS-NLMP test vectors, not against a real Windows machine yet**.

## 0.9.2

### Look
- **Two logos, one per theme:** the new Aurora logo comes in a dark and a light version. The taskbar and
  window icon follow the theme (dark logo on dark themes, light logo on light themes); the installed app
  icon is the dark one.

### Split view
- **Arrange the panes freely:** besides the automatic layout (side by side → 1 + 2 → 2 × 2) the split
  menu now offers *Side by side*, *Stacked* (two or more panes **below each other**), *One large on
  the left*, *One large on top* and *Grid*. The choice is remembered.
- **Resize the panes:** drag the divider between two panes (double-click resets it); the positions are
  remembered per layout.
- **Move panes by dragging their header** onto the slot where they should go (a preview shows the
  resulting layout); the same drop zones work for sessions dragged out of the rail.

### Diagnostics
- **sudo without a password:** leave the sudo password empty to run the collection with `sudo -n` – for
  hosts you log in to with a key where sudo needs no password (`NOPASSWD`). Before, the field had to be
  filled. If the host does ask for a password, Aurora says so.
- **HTML report in the diagnostics bundle:** next to `summary.txt` and the text files the bundle now
  contains `report.html` – a readable page with the host details, key facts (uptime, fullest disk,
  memory, failed services), a table of contents with status pills and one collapsible section per
  command (errors and warnings highlighted, long output shortened with a pointer to the text file).
  Light and dark mode, printable, no scripts, all remote data escaped, redaction applies.

### License
- Aurora is now released under the MIT License (`LICENSE`).

## 0.9.1

### Fixes
- **Security: paste protection.** Text pasted into a terminal with bracketed paste could rebuild
  the end-of-paste marker (`ESC[2` + `ESC[201~` + `01~`) and have the rest run as typed
  input. All control characters except tab and line breaks are now removed from pasted text.
- Session recordings are created readable by the owner only (mode 0600 on Linux/macOS).
- mRemoteNG exports (`<mrng:Connections …>`) are recognised; the Markdown/HTML export masks
  `10.1.2.3:8080` and `10.1.2.3.`; log commands also work on fish/csh login shells; the
  inventory refresh timeout grows with the number of info snippets; unparseable lines are no
  longer dropped when the session history is cleaned; deleting a single host also removes its
  inventory details; "Show in list" leaves a customer scope when the target is outside it;
  resizing a terminal no longer re-rings old bells.

## 0.9.0

### Keychain
- **Add existing SSH keys by drag & drop:** drop key files (or a whole `~/.ssh` folder) onto the
  Keys page. Aurora reads the private key, names it after the file, skips duplicates (same
  fingerprint) and opens the entry when it is a single key so you can set the user name and
  the passphrase of an encrypted key. *From ~/.ssh* lists the keys that are not in the
  keychain yet. `.pub` and PuTTY `.ppk` files are rejected with a hint.

### Web interfaces
- **New host type "Web interface"** (like the web entries of Devolutions Remote Desktop
  Manager): keep the web UIs of firewalls, switches, NAS boxes, Proxmox, iDRAC/iLO … next to
  your SSH and RDP hosts and open them from Aurora. *New -> Web interface*, or pick the
  *Web* filter in the host list. Groups, tags, favorites, colors, environments, notes,
  search/palette, reachability LED, certificate radar, Wake-on-LAN and the inventory
  ("Web only - no facts") work like for any other host.
- **Own window, own profile per entry:** the page opens in the new helper `aurora-web`
  (WebView2 on Windows, WKWebView on macOS, WebKitGTK on Linux). Cookies and logins of every
  entry are isolated in `web-profiles/<id>` in the Aurora data folder; the window title is
  the entry name. The Aurora tab shows the status, page title, address and the pinned
  certificate, with *Bring window to front*, *Reload* and *Close*. Closing the tab closes the
  window and the other way round; opening an entry again raises its window. (Embedding the
  page inside the Aurora window is planned for a later version.)
- **Vendor templates** in the form (pfSense, OPNsense, FortiGate, Sophos, UniFi, MikroTik
  WebFig, Synology DSM, TrueNAS, Proxmox VE :8006, Proxmox Backup :8007, iDRAC, iLO,
  Ubiquiti, generic): fill port, path, login field hints, tag and TLS mode.
- **TLS certificate errors, three ways:** *Strict* (system trust only), *Self-signed (pin)*
  - on first open Aurora shows the SHA-256 fingerprint and asks; afterwards exactly that
  certificate is accepted, and a changed certificate opens a loud warning that needs the
  host name typed to accept - and *Ignore all* (explicit per-entry opt-in with a warning).
  The check runs in the helper itself, not only in Aurora.
- **Auto-login:** fills the login form with the entry's identity or password-manager
  credentials (user and password selectors configurable, default: first visible password
  field and the text field before it) and optionally presses the login button - at most once
  per open, so a wrong password cannot lock the account. Secrets travel over a stdin pipe to
  the helper only: never in argv, URLs, logs or on disk outside the vault. The script is built
  with escaped string literals (unit-tested against quote and `</script>` injection) and the
  page checks its own origin before anything is filled.
- **Navigation limits:** the window stays on the entry's origin (plus hosts you list for SSO);
  links to other origins open in the system browser, `javascript:`, `file:` and `data:`
  addresses are refused, `window.open` never creates a second window, developer tools are off
  in release builds. Downloads are saved to `Downloads/Aurora` with a sanitized file name.
- Packages: the helper ships in the Windows installer and zip, the .deb (new dependency
  `libwebkit2gtk-4.1-0`), the tar.gz and the macOS app. The Windows installer points to the
  WebView2 runtime if it is missing (it is part of Windows 11 and current Windows 10).
- macOS: certificate pinning is not available yet (WKWebView offers no hook through the web
  view library); only certificates the system trusts load there.
### Import
- **Migration importer** (Settings -> General -> *Other connection manager*, or the empty host
  list): open or drop an export of **mRemoteNG** (`confCons.xml`), **Royal TS / TSX** (Royal
  JSON), **Remote Desktop Manager** (JSON, `.rdm` XML) or a **CSV** table. The preview shows the
  folder tree (collapsed), counts per protocol, conflicts with existing hosts (skip / overwrite
  / keep both), warnings, a checkbox per folder and entry, a search box, a target group (new
  group named after source and date, an existing group, or top level) and an adjustable protocol
  mapping. Folders become nested groups (at most 12 levels). A report lists what was imported
  and why entries were skipped.
- Passwords: encrypted ones (mRemoteNG, Royal, RDM) are never decrypted or imported and are
  reported. Clear-text CSV passwords are stored in the vault (keychain) only after an explicit
  switch, never in the host list.
- Limits: 50 MB per file, 200,000 entries, XML without DTD/entity expansion, `.rtsz` archives
  are not read (export "Royal JSON" instead).
### Large inventories
- **Group tree for 20 000+ hosts:** customer -> site/type -> host as a collapsible tree with a
  count per group ("Acme · 214"), a status roll-up on every group (red = unreachable or
  critical, amber = warnings from the collected facts), a letter badge in the group colour,
  pinned groups on top and groups sorted by name (numbers sort naturally). Expand/collapse
  all, expansion is remembered, and only the visible rows are drawn (virtual scrolling), so
  the list stays smooth and opens fast; the flattened rows are cached.
- **Keyboard:** arrows move, Enter opens a host or toggles a group, Right/Left expand/collapse
  (Left also jumps to the parent), Space selects, Home/End/PageUp/PageDown; typing starts the
  search. A breadcrumb above the list shows where you are.
- **Only this customer (focus):** right-click a group -> "Show only this group" scopes the
  list, the palette (Ctrl+K), the inventory page and the bulk actions to it; a chip with an
  X in the page header and in the sidebar ends it.
- **Search language:** `customer/site/name`, `type:switch`, `tag:x` / `#x`, `env:prod`,
  `is:down|up|unknown|fav|warn|maint|live`, `ip:10.1.` (or just `10.1.`), `os:ubuntu`,
  `group:x`, `-` negates, `"quotes"` keep spaces. Free words still match fuzzily.
- **Saved views** (search + protocol filter + sort + focus + list/table) in the sidebar and
  the "Views" menu; recent hosts and pinned groups in the sidebar and as chips above the list.
- **Table view** next to the list (remembered per focus): name, address, type, tags, status,
  last seen, environment, group; sortable, resizable and hideable columns (right-click the
  header), automatic column dropping in narrow windows, Ctrl/Shift multi-select and the host
  context menu.
- **Group actions:** pin, badge letters, set the environment or add/remove a tag on all hosts
  of a group (with confirmation and count); "Open all" shows how many hosts it connects.
- Reachability probes run at most 128 at a time, hosts in view first, and results show up as
  they arrive; big libraries are written to disk compactly; `AURORA_PERF=1` prints frame times.

## 0.8.1

### Servers
- **Docker right after `usermod -aG docker`:** a "permission denied" on the local Docker
  socket is retried through `sg docker`, so it works without logging out first.
- **Containers in WSL:** on a Windows PC the Container page also finds Docker, Podman and
  kubectl that live inside WSL (run through `wsl.exe -e`, default distribution).
- **Inventory detail view:** click a host in the inventory table for a sheet with the tabs
  *System* (OS, hardware, disks with usage bars, addresses, users, warnings), *Services*
  (search, filter running/stopped/failed, failed ones highlighted), *Firewall* (ufw,
  firewalld, nft or iptables; `sudo -n` is tried, "needs root" is shown if it is denied -
  Aurora never asks for a password), *Ports* (`ss -tulpn`, fallback `netstat`) and *Info*.
  Collected on *Refresh facts* and stored in the library (at most 500 services, 200 rules,
  500 ports per host). Optional column *Services with errors* (off by default).
- **Info snippets** (Settings -> General -> Inventory): your own commands (name, command,
  optional host/group/tag filter, templates for installed packages, cron jobs, Docker
  containers, open ports) that run over SSH on the matching hosts when facts are refreshed.
  Text output only (64 KB, 30 s per command), shown in the *Info* tab and exported as one
  chapter per snippet. This is also the way to collect data from Windows hosts with the
  OpenSSH server: enter your own PowerShell commands.
- Hosts without a command channel (RDP, VNC, Telnet, Serial) now say "RDP only - no facts
  (needs SSH/OpenSSH)" instead of a row of dashes.
- **System documentation export** (new export dialog): Markdown with one chapter per host
  (overview, hardware, OS, network, disks, services, ports, firewall, info snippets, notes),
  self-contained printable HTML with table of contents (light theme, page break per host),
  JSON with all data, and the existing CSV. Header with date, host count and filter scope,
  summary (warnings, pending updates, failed services) and a "Notes / not collected"
  section. Choose what is included, drop IP addresses (replaced by a placeholder), notes are
  off by default. All text is escaped.

### Fixes
- The Container dialog also finds Docker, Podman and kubectl inside WSL on Windows.
- Hardening: recordings are only deleted/opened by a plain `*.cast` file name from the history, and the
  title/host in a Markdown session export are escaped.
- The Snippets button in the session bar shows its active state like Logs.

## 0.8.0

### Files
- **Built-in editor:** double-click a server (or local) file to edit it in an Aurora tab –
  line numbers, syntax highlighting (shell, Python, JSON, YAML, TOML/INI, nginx/Apache,
  Markdown, Rust, JavaScript, HTML, Dockerfile, SQL, systemd, logs), find/replace, go to line,
  LF/CRLF/BOM/final newline preserved, conflict check on save, unsaved marker, read-only
  preview for large or binary files. *Open file…* from any SSH terminal. The external editor
  stays available (Settings → *Standard editor*).
- **Transfer queue:** all copies go through one queue with a panel (speed, ETA, pause, resume,
  cancel, retry), parallel limit, bandwidth limit, optional SHA-256 check, overwrite policy,
  resume of interrupted transfers (`.aurora-part`, also after a restart).

### Terminal & sessions
- **Command blocks**, **session markers and history** (see below), **automatic reconnect**
  after connection loss or standby (backoff, banner, tmux re-attach, per-host override),
  **log commands with `{{variables}}`** (asked at start, recent values remembered).
- The terminal now answers device queries (device attributes, cursor position, version), so
  fish no longer waits ten seconds at start.

### Servers
- **Container page** (sidebar): Docker, Podman and Kubernetes on this PC and over SSH – state,
  CPU/RAM, ports, compose groups, pod warnings, start/stop/restart/remove, shell, logs,
  inspect, rollout restart.
- **Server inventory** (sidebar): table of all hosts with OS, kernel, uptime, disk, updates,
  security updates, CPUs, RAM; warning rules with thresholds, end-of-life table, background
  refresh of facts, export as CSV/Markdown/JSON.

### Quality of life
- **Password managers** and the *Choose from keychain* picker in the password prompt (below).
- **What is new** sheet after an update (also in Help and the palette).
- **Custom themes:** import iTerm2, Windows Terminal, base16, Alacritty and Kitty/Ghostty
  colour schemes; the whole app follows the scheme.

### Details
- **Password managers:** take passwords (and SSH keys for identities) from Bitwarden (`bw`),
  1Password (`op`), KeePassXC (`keepassxc-cli`) or Proton Pass (`pass-cli`) instead of the
  Aurora vault. Settings -> Security -> Password managers; hosts and identities pick an item
  (only a reference is stored, never the secret). Secrets are fetched right before connecting
  (SSH, RDP, VNC, Telnet, SFTP, tunnels, jump hosts), live in memory only and are wiped.
- **Password prompt:** *Choose from keychain* - pick a stored identity or a password-manager
  item (search, arrow keys, Enter / double-click), optionally remember it for the host.
- **Command blocks:** every command and its output get a gutter mark (green exit 0, red
  failed, grey unknown) and a hover toolbar (copy command/output/both, rerun, collapse,
  duration). `Ctrl+↑/↓` jumps between commands, `Ctrl+Shift+O` copies the last output, the
  context menu jumps to the last error. Uses OSC 133 shell integration (snippets for bash,
  zsh, fish in Settings; optional per-host automatic setup after login), falls back to
  guessing at the prompt.
- **Session markers and history:** `Ctrl+Shift+M` sets a marker with a note (also written to
  recordings as asciicast `m` events; replays keep notes in `<name>.cast.notes.json`), flags on
  the time-travel timeline, full-text search over all recordings, session history per host,
  export of recordings as Markdown or text.

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
