<div align="center">

<img src="docs/assets/hero.svg" alt="Aurora – one calm, fast client for everything you administer" width="100%">

[![Release](https://img.shields.io/github/v/release/theslasher5g/ssh-client?style=flat-square&color=7c9cff)](../../releases/latest)
[![Downloads](https://img.shields.io/github/downloads/theslasher5g/ssh-client/total?style=flat-square&color=6cc9a5)](../../releases)
![Platforms](https://img.shields.io/badge/Windows%20·%20Linux%20·%20macOS-grey?style=flat-square)
![Rust](https://img.shields.io/badge/built%20with-Rust-dea584?style=flat-square&logo=rust&logoColor=white)
![Languages](https://img.shields.io/badge/UI-English%20·%20Deutsch-grey?style=flat-square)

[**Download**](#-download) · [Features](#-features) · [Demos](#-demos) · [Screenshots](#-screenshots) · [Themes](#-themes) · [Shortcuts](#-keyboard-shortcuts) · [Build](#-building) · [Security](#-data--security) · [Changelog](CHANGELOG.md)

<br>

![Aurora – host list](docs/screenshots/hosts.png)

</div>

## Why Aurora

Aurora is a native desktop app for people who manage many servers, firewalls and customers.
Everything lives in one window, one encrypted keychain and one searchable host list – from the
first `ssh` to a 20 000‑host inventory.

| | |
|---|---|
| 🖥️ **All protocols in one place** | SSH, SFTP, RDP, VNC, Telnet, serial, **web interfaces** (firewalls, switches, NAS, Proxmox …), Docker/Podman/Kubernetes – side by side in up to four split panes. |
| 🔐 **Secrets stay secrets** | Encrypted vault (Argon2id + XChaCha20‑Poly1305), SSH keys by drag & drop, Bitwarden / 1Password / KeePassXC / Proton Pass, SSH certificates, server keys and TLS certificates pinned on first use. |
| 🏢 **Built for MSP scale** | Customer → site → host tree, "only this customer" focus, a search language (`type:switch is:down env:prod`), saved views, virtual scrolling for 20 000+ hosts. |
| ⚡ **Fast and light** | Rust, GPU‑drawn UI, one small download, no browser engine in the main app. 26 themes, English and German. |
| 🧰 **Made for operations** | Command blocks, session recording and time travel, live logs next to the terminal, runbooks, transfer queue, server inventory with system documentation export. |
| 🚚 **Easy to switch** | Import from **mRemoteNG, Royal TS, Remote Desktop Manager, CSV and `~/.ssh/config`**. |

## ⬇ Download

Every [release](../../releases/latest) contains Windows, Linux and macOS:

| System | File | Install |
|---|---|---|
| **Windows** (x64) | `Aurora-<version>-windows-x64-setup.exe` | run the installer |
| | `Aurora-<version>-windows-x64-portable.zip` | unzip, start `Aurora.exe` |
| **Linux** (x86_64) | `aurora_<version>_amd64.deb` | `sudo apt install ./aurora_<version>_amd64.deb` |
| | `aurora-<version>-linux-x86_64.tar.gz` | unpack, `./install.sh` or `./aurora` |
| **macOS** 11+ (Apple Silicon & Intel) | `Aurora-<version>-macos-universal.dmg` | open, drag Aurora to Applications |

> **Good to know**
> - **Windows:** without a code‑signing certificate SmartScreen may ask once ("More info" → "Run anyway"). Web interface entries use the Microsoft Edge WebView2 runtime (part of Windows 11 and current Windows 10).
> - **macOS:** the app is not notarized – on first start right‑click Aurora → *Open* (or `xattr -cr /Applications/Aurora.app`).
> - **Linux:** needs glibc ≥ 2.35 and X11 or Wayland with OpenGL; web interface entries need WebKitGTK (`libwebkit2gtk-4.1-0`, installed with the `.deb`).
> - Aurora checks for new versions on start and can update itself (downloads are verified against `SHA256SUMS.txt`). Language: *Settings → Sprache / Language*.

## 🚀 Quick start

1. **Create the vault** – choose a password on first start; it encrypts your keychain.
2. **Add hosts** – *New* → SSH / RDP / VNC / Telnet / Serial / Web interface / Container, or import from
   `~/.ssh/config`, mRemoteNG, Royal TS, RDM or a CSV file (*Settings → General → Other connection manager*).
3. **Add keys** – drop key files (or your `~/.ssh` folder) on the *Keys* page, or generate an Ed25519 key.
4. **Connect** – double‑click a host or press `Ctrl/⌘ + K` and type a name, `user@host` or `ssh -p 2222 user@host`.
5. **Work side by side** – drag a session into the window to split it, press `Ctrl + Shift + L` for live logs.

## ✨ Features

### Connect

| | |
|---|---|
| **SSH** | Own terminal emulator (truecolor, scrollback, selection across screens, reflow on resize, bracketed paste), key / password / keyboard‑interactive login, keepalive, **automatic reconnect**, optional tmux/screen persistence |
| **SFTP & files** | Two columns, each side local or **any server** – including **server ⇄ server**. Transfer queue (concurrency, bandwidth limit, pause/resume, SHA‑256 check), built‑in editor, edit remote files and upload on save |
| **RDP** | In the tab (IronRDP): clipboard both ways, Ctrl+Alt+Del, type the clipboard as keystrokes, **drop files onto the session**, PowerShell snippets |
| **VNC · Telnet · Serial** | Own RFB client (ZRLE, clipboard, view‑only) · Telnet with auto‑login for switches and routers · serial consoles with baud rate, flow control and echo |
| **Web interfaces** | Firewall, switch, NAS, iDRAC/iLO, Proxmox … web UIs as host entries with **vendor templates**, own cookie profile per entry, **self‑signed certificates pinned by fingerprint** and optional **auto‑login** |
| **Containers** | Docker, Podman and Kubernetes on this PC (also inside WSL) or any SSH host: shell, logs, actions, compose groups, pod warnings |
| **Proxmox VE** | All VMs and containers by node, snapshots and backups, console (noVNC / SPICE / `pct enter`), live graphs, clone, migration by drag & drop, cluster overview, Proxmox Backup Server |
| **Tunnels & routes** | Port forwardings as switches, ProxyJump chains, automatic route finding, Wake‑on‑LAN |

### Organise

| | |
|---|---|
| **Host list** | Nested groups with colour and icon, tags, favourites, environment labels (PROD / STAGING / TEST / DEV), notes, reachability LEDs, OS / uptime / disk / updates at a glance |
| **Large inventories** | Customer → site → host tree with counts and status roll‑up, **focus on one customer**, search language, saved views, table view, keyboard navigation |
| **Server inventory** | Table with OS, kernel, uptime, disk, updates and warnings; per‑host detail view (services, firewall rules, listening ports, your own info snippets); **system documentation export** to Markdown, printable HTML, JSON and CSV |
| **Migration** | Import from mRemoteNG, Royal TS, Remote Desktop Manager, CSV and `~/.ssh/config` with preview, conflict handling and report |
| **Keychain** | Passwords and SSH keys in coloured groups, key import by **drag & drop**, Ed25519 generator, SSH certificates (Aurora as a small CA), password managers as credential source |

### Work faster

| | |
|---|---|
| **Command blocks** | Every command and its output as a block (exit code, copy, rerun, collapse, jump with `Ctrl+↑/↓`) |
| **Logs** | Follow files, `journalctl -f` or container logs next to the terminal, with filters, highlighting and **own log commands with `{{variables}}`** |
| **Recording & history** | Record sessions (asciinema `.cast`), time travel through the last 30 minutes, markers with notes, full‑text search over recordings |
| **Snippets & runbooks** | Shell / PowerShell snippets with placeholders, step‑by‑step runbooks with a Markdown report |
| **Safety** | PROD sessions get a red frame and ask before dangerous commands, secret guard for pastes, auto‑lock, maintenance windows |

<details>
<summary><b>Full feature reference</b> (every feature, one line each)</summary>

<br>

| Area | |
|---|---|
| **SSH** | Own terminal emulator (256 colours/truecolor, scrollback, selection, bracketed paste, resize), key/password/keyboard-interactive auth, keepalive, reconnect with Enter |
| **Files (SFTP)** | Own section in the rail. Two columns, each side **local or any server** – including **server ⇄ server** (streamed through Aurora, the servers don't need to reach each other). Windows servers with OpenSSH (`C:\path` is translated, local drive list), RDP hosts get their SFTP port. Recursive transfers with progress, drag & drop |
| **Edit remote files** | Double-click a server file to open it in your editor; every save is uploaded automatically (executable file types are never opened with the default app) |
| **Built-in editor** | Double-click opens server or local text files in an Aurora tab: syntax highlighting, find/replace, safe saving with conflict check, CRLF/LF kept; also `Open file…` from an SSH terminal |
| **Transfer queue** | All copies run through one queue: concurrency and bandwidth limit, pause/resume/retry, resume after reconnect or restart, optional SHA-256 check, overwrite policy |
| **Split screen** | Up to **4 sessions side by side** (terminal, files, RDP mixed) – drag a port from the rail into the window (a preview shows where it lands), the split menu or right-click. Double-click a header to maximize, "type into all panes", × ends a session |
| **Full screen & windows** | **Full screen** (`F11`) hides everything but the session, with a session switcher in the top bar · **open a session in its own window** (and bring it back with the same button) |
| **Tunnels & jump host** | Port forwardings per host as switches in the list (status LED, optionally open the browser); "connect via" another SSH host (ProxyJump) – used for terminal, files and tunnels |
| **RDP** | Inside the tab (IronRDP): follows the window size, **clipboard in both directions** (automatic + "copy from host"/"send to host" buttons), **Ctrl+Alt+Del**, Windows key, type the clipboard as keystrokes (for login screens/consoles). **Drop files onto the session** to copy them to Windows (clipboard channel, no SFTP needed) and **save files copied on the host**. **PowerShell snippets** run on the Windows host from the session header. Optionally in the system client (mstsc, FreeRDP, Remmina) |
| **VNC** | Inside the tab (own RFB client: ZRLE, CopyRect, Raw, resize, VNC password): fit or 1:1, Ctrl+Alt+Del, clipboard in both directions, type the clipboard as keystrokes, view-only mode |
| **Web interfaces** | Host type *Web interface*: firewall, switch, NAS, Proxmox, iDRAC/iLO web UIs next to your SSH/RDP hosts, in their own window (WebView2 / WKWebView / WebKitGTK) with an **own cookie profile per entry**. **Vendor templates** (pfSense, OPNsense, FortiGate, Sophos, UniFi, MikroTik, Synology, TrueNAS, Proxmox VE/PBS, iDRAC, iLO …), **self-signed certificates pinned by SHA-256 fingerprint** (asks on first use, warns when it changes), optional **auto-login** from the keychain or password manager, links to other sites open in your browser |
| **Telnet** | For switches, routers and old devices: option negotiation (echo, window size, terminal type) and optional auto-login with the stored user and password |
| **Serial** | Port detection, baud rate, format, flow control, line ending, local echo |
| **Proxmox VE** | Own page with all VMs and containers of your Proxmox servers by node: status, IP, CPU/RAM, tags; start/stop/shutdown/reboot, **snapshots and backups** (create, roll back, restore to a new VMID), console (`pct enter`, `qm terminal`, SSH, noVNC or SPICE), save as host with the right IP. API token or password (in the vault), pinned certificate. **Live graphs**, clone / new from template with cloud-init, **migration by drag & drop**, hardware edits, batch actions, cluster overview (storage, ZFS, Ceph, HA), task log, **alerts**, node shell via the API, node updates and **Proxmox Backup Server** browsing |
| **Docker & Kubernetes** | Containers and pods as their own session type – on this PC or on any SSH host. "Browse containers" lists `docker ps` / `kubectl get pods` (Podman and kube contexts supported); one click opens a shell (bash, else sh) or the logs, or saves the container to the host list |
| **Log viewer** | Follow a file (`tail -F`), `journalctl -f` or container logs live – in their own tab or **docked at the right of an SSH session** (`Ctrl+Shift+L`, same login, resizable) so terminal and logs sit side by side. **Own log commands** (for all hosts or one host) next to the presets. Filter (`nginx \| timeout`), only matches or highlighting, errors/warnings colored and filterable, pause with a counter of waiting lines, copy, wrap |
| **Session recording** | Record a terminal (asciinema `.cast`, only what the screen shows) with one click or for every SSH session; replay inside Aurora (Space pauses) or with `asciinema play` |
| **Persistent sessions** | Optional per host: the shell runs in `tmux` (or `screen`) on the server, a reconnect lands in the same session with its running jobs |
| **Terminal profile** | Per host: own color theme (e.g. PROD always in a different look), font and font size. Fonts: IBM Plex Mono, Hack, installed monospace fonts (Cascadia, Consolas, Menlo, DejaVu, JetBrains Mono …) or any .ttf/.otf file |
| **Host list** | Groups as cards with "x of y online". After the first login it shows **OS, uptime, disk usage and pending updates** (one click: "install updates"), latency and status (green = reachable, ring = no answer, red = last connection failed). Hosts without a name take the server's hostname, rename with `F2`. Multi-select (Ctrl/Shift+click) with bulk actions, drag hosts into groups, compact view, sorting. Groups show "x/y online"; hosts in the same subnet can be grouped with one click |
| **Large inventories** | Built for 100 customers and 20 000+ hosts: virtual scrolling, a customer → site → host tree with counts and status roll-up, keyboard navigation, breadcrumb, **"only this customer" focus** for the whole app, a search language (`customer/site/name`, `type:switch`, `is:down`, `env:prod`, `#tag`, `10.1.`), saved views, pinned/recent groups and hosts, and a sortable table view |
| **Terminal memory** | Commands per host remembered (encrypted) – `Ctrl+R` / `Ctrl+K` to find them again; commands you repeat are offered as snippets; **time travel** through the last 30 minutes of a terminal like a video; **secret guard** asks before pasting keys, tokens or passwords |
| **Runbooks** | Step-by-step procedures with commands and confirmations, run next to the terminal(s) with a Markdown report of every step and its output |
| **Maintenance & diagnostics** | Maintenance windows per host or group mute alerts and mark the sessions; one-click diagnostics bundle (`.tar.gz`, optional masking of personal data) |
| **SSH certificates** | Aurora as a small SSH CA: log in with a fresh certificate valid for 1–24 h instead of stored keys. Changed server keys are double-checked against SSHFP (DNS) and the jump host |
| **Routes & wake-up** | Chains of up to 4 jump hosts; if a host can't be reached directly Aurora finds a working route through your hosts · **Wake-on-LAN** with "wake up and connect" · **certificate radar** warns before TLS certificates of your hosts expire |
| **Container page & inventory** | Sidebar pages: **Docker / Podman / Kubernetes** on this PC and over SSH (state, CPU/RAM, compose groups, pod warnings, actions, shell, logs) and a **server inventory** table (OS, kernel, uptime, disk, updates, security updates, warnings with thresholds) with a per-host **detail view** (system, disks, services with failed ones highlighted, firewall rules, listening ports, output of your own **info snippets**) and a **system documentation export** (Markdown, printable HTML, JSON, CSV; optional IP masking). RDP-only hosts are shown honestly as "no facts" |
| **Command blocks & markers** | Every command and its output as a block (exit-code mark, copy, rerun, collapse, `Ctrl+↑/↓` to jump) via OSC 133 shell integration; **session markers** with notes, full-text search over all recordings, session history, Markdown export |
| **Reconnect** | After a connection loss or standby Aurora reconnects by itself (backoff, banner, tmux re-attach), switchable per host |
| **Password managers** | Bitwarden, 1Password, KeePassXC and Proton Pass as credential source (only a reference is stored; fetched right before connecting, memory only) · *Choose from keychain* in the password prompt |
| **Custom themes** | Import colour schemes (iTerm2, Windows Terminal, base16, Alacritty, Kitty/Ghostty) – the whole app follows them · a *What is new* sheet after every update |
| **Port scan** | Per host: TCP scan from this PC (well-known, 1–1024, all or a custom list) or the ports listening on the server (also localhost-only services); open a web port in the browser or tunnel to it with one click |
| **Network scan** | Finds SSH/RDP devices in the local network (with an OS hint from the SSH banner) and adds them |
| **SSH key setup** | Generates a key, installs it on the server, verifies the login – no more passwords |
| **Keychain** | Passwords & SSH keys in coloured groups with search, Ed25519 generator, encrypted with Argon2id + XChaCha20-Poly1305 |
| **Snippets** | Shell or PowerShell, `{{variable}}` placeholders with live preview, snippet panel in the terminal, run in all sessions, startup snippet per host, save selected terminal text as a snippet |
| **Groups** | Nested, colour & icon, inherited default user/identity, favourites, recently used, tags |
| **Productivity** | **Quick connect**: `user@host`, `host:port` or `ssh -p 2222 user@host` in `Ctrl+K`, then "save as host" · **open a whole group** side by side · **search in the terminal** (`Ctrl+Shift+F`, incl. scrollback) · **font size with `Ctrl` + mouse wheel** per pane · import from `~/.ssh/config` · sessions and split layout are **restored on the next start** |
| **Safety** | **Environment labels** PROD / STAGING / TEST / DEV per host or group – PROD sessions get a red frame and **ask before dangerous commands** (`rm -rf`, `reboot`, `DROP TABLE` …) · **notes per host** shown when connecting · **auto-lock** after inactivity (sessions keep running) · clear error messages with the next step |
| **Migration import** | Move hundreds of entries from other managers: **mRemoteNG** `confCons.xml`, **Royal TS/TSX** (Royal JSON), **Remote Desktop Manager** (JSON / `.rdm` XML) and generic **CSV** (`name, host, port, protocol, user, group, tags, description`; header detection, `;`/`,`/tab, BOM). Preview sheet with folder tree, per-entry checkboxes, protocol mapping, conflict handling (skip / overwrite / keep both), target group, and a result report. Encrypted passwords are never decrypted; clear-text CSV passwords go to the vault only after an explicit opt-in. Settings → General → *Other connection manager* |
| **Comfort** | Reachability LED per host (TCP check with latency), **update check** on start with one-click update (SHA256-verified) · drop files onto a terminal to upload them to the current directory · **open the file browser at the terminal's folder** · a port blinks when a long command finishes in the background · disconnected sessions marked with ⟳, click to reconnect · encrypted **export/import** of hosts, groups, snippets and keys |
| **Design** | Calm card layout with Inter and IBM Plex Mono, line icons and rounded sheets for all editors. The sidebar shows labels on the start pages and becomes a slim rail with numbered session tiles once sessions are open. **26 themes** – dark: Phosphor, Glacier, Nord, Dracula, Tokyo Night, Gruvbox, Catppuccin, Rosé Pine, Solarized Dark, One Dark, Monokai, Everforest, Kanagawa, Ayu Mirage, Night Owl, Synthwave, Midnight, Matrix; light: Paper, Solarized Light, GitHub Light, Catppuccin Latte, Rosé Pine Dawn, One Light, Ayu Light, Nord Light – the terminal follows the theme. Settings in tabs (appearance, terminal, general, security, data & updates), command palette `Ctrl/⌘+K`, broadcast mode, first-start tour, help page with all shortcuts (`F1` / `?`) |

</details>

## 🎬 Demos

| | |
|---|---|
| ![Connect](docs/demo/connect.gif)<br><sub><b>Connect</b> – command palette, SSH session, coloured output</sub> | ![Split screen](docs/demo/split.gif)<br><sub><b>Split screen</b> – four sessions side by side</sub> |
| ![Large inventories](docs/demo/inventory.gif)<br><sub><b>Large inventories</b> – customer → site → host tree, focus on one customer, search language</sub> | ![Logs](docs/demo/logs.gif)<br><sub><b>Live logs</b> – docked next to the terminal, with filters</sub> |

## 🖼 Screenshots

| | |
|---|---|
| ![Split screen – two panes](docs/screenshots/tokyo.png)<br><sub>Split screen – two panes</sub> | ![PROD sessions: red frame and warnings](docs/screenshots/prod.png)<br><sub>PROD sessions: red frame and warnings</sub> |
| ![Files: server ⇄ server](docs/screenshots/sftp.png)<br><sub>Files: server ⇄ server</sub> | ![Full screen with session switcher](docs/screenshots/fullscreen.png)<br><sub>Full screen with session switcher</sub> |
| ![Terminal](docs/screenshots/terminal.png)<br><sub>Terminal</sub> | ![New host](docs/screenshots/editor.png)<br><sub>New host</sub> |
| ![RDP in the tab](docs/screenshots/rdp.png)<br><sub>RDP in the tab</sub> | ![Command palette (Ctrl/⌘ + K)](docs/screenshots/palette.png)<br><sub>Command palette (Ctrl/⌘ + K)</sub> |
| ![Snippets](docs/screenshots/snippets.png)<br><sub>Snippets</sub> | ![Settings](docs/screenshots/themes.png)<br><sub>Settings</sub> |
| ![Shortcuts help (F1)](docs/screenshots/shortcuts.png)<br><sub>Shortcuts help (F1)</sub> | ![Files](docs/screenshots/files.png)<br><sub>Files</sub> |
| ![Encrypted vault](docs/screenshots/lock.png)<br><sub>Encrypted vault</sub> | ![Host list](docs/screenshots/hosts.png)<br><sub>Host list</sub> |

(Screenshots show the German or English UI.)

## 🎨 Themes

26 built-in themes – the whole app and the terminal follow the theme (*Settings → Appearance*,
or `Ctrl/⌘ + K` → theme). Import your own from iTerm2, Windows Terminal, base16, Alacritty or Kitty/Ghostty.

![Switching themes](docs/demo/themes.gif)

<details>
<summary><b>All 26 themes</b> – same scene in every theme</summary>

<br>

| | | |
|---|---|---|
| ![Phosphor](docs/themes/phosphor.png)<br><sub>Phosphor</sub> | ![Glacier](docs/themes/glacier.png)<br><sub>Glacier</sub> | ![Nord](docs/themes/nord.png)<br><sub>Nord</sub> |
| ![Dracula](docs/themes/dracula.png)<br><sub>Dracula</sub> | ![Tokyo Night](docs/themes/tokyo-night.png)<br><sub>Tokyo Night</sub> | ![Gruvbox](docs/themes/gruvbox.png)<br><sub>Gruvbox</sub> |
| ![Catppuccin](docs/themes/catppuccin.png)<br><sub>Catppuccin</sub> | ![Rosé Pine](docs/themes/rose-pine.png)<br><sub>Rosé Pine</sub> | ![Solarized Dark](docs/themes/solarized-dark.png)<br><sub>Solarized Dark</sub> |
| ![One Dark](docs/themes/one-dark.png)<br><sub>One Dark</sub> | ![Monokai](docs/themes/monokai.png)<br><sub>Monokai</sub> | ![Everforest](docs/themes/everforest.png)<br><sub>Everforest</sub> |
| ![Kanagawa](docs/themes/kanagawa.png)<br><sub>Kanagawa</sub> | ![Ayu Mirage](docs/themes/ayu-mirage.png)<br><sub>Ayu Mirage</sub> | ![Night Owl](docs/themes/night-owl.png)<br><sub>Night Owl</sub> |
| ![Synthwave](docs/themes/synthwave.png)<br><sub>Synthwave</sub> | ![Midnight](docs/themes/midnight.png)<br><sub>Midnight</sub> | ![Matrix](docs/themes/matrix.png)<br><sub>Matrix</sub> |
| ![Paper](docs/themes/paper.png)<br><sub>Paper</sub> | ![Solarized Light](docs/themes/solarized-light.png)<br><sub>Solarized Light</sub> | ![GitHub Light](docs/themes/github-light.png)<br><sub>GitHub Light</sub> |
| ![Catppuccin Latte](docs/themes/catppuccin-latte.png)<br><sub>Catppuccin Latte</sub> | ![Rosé Pine Dawn](docs/themes/rose-pine-dawn.png)<br><sub>Rosé Pine Dawn</sub> | ![One Light](docs/themes/one-light.png)<br><sub>One Light</sub> |
| ![Ayu Light](docs/themes/ayu-light.png)<br><sub>Ayu Light</sub> | ![Nord Light](docs/themes/nord-light.png)<br><sub>Nord Light</sub> |  |

</details>

## ⌨ Keyboard shortcuts

| Shortcut | Action |
|---|---|
| `Ctrl/⌘ + K` | Command palette (hosts, snippets, tunnels, themes, `user@host`) |
| `Ctrl + 1…9` | Jump to port/session 1…9, `Ctrl + 0` back to the overview |
| `Ctrl + Tab` | Next session |
| `Ctrl/⌘ + Shift + W` | Close session |
| `Ctrl + Shift + F` | Search in the terminal (Enter = older, Shift+Enter = newer match) |
| `Ctrl + Shift + L` | Logs next to the terminal |
| `Ctrl + Shift + T` | Local terminal (your shell on this PC) |
| `Ctrl + R` | Search the host's command history (at the shell prompt) |
| `Ctrl` + mouse wheel | Terminal font size |
| `F11` | Full screen on/off |
| `F1` / `?` | Help page with all shortcuts |
| `Ctrl + Shift + C` / selection | Copy in the terminal |
| `Enter` in a disconnected session | Reconnect |

## 🛠 Building

Rust ≥ 1.88:

```bash
cargo build --release                                              # app
cargo build --release --manifest-path crates/aurora-rdp/Cargo.toml # RDP component
cargo build --release --manifest-path crates/aurora-web/Cargo.toml # web interface window (Linux: libwebkit2gtk-4.1-dev, libgtk-3-dev)
cargo test
```

The RDP component is a separate crate (`crates/aurora-rdp`) because IronRDP pins crypto
pre-releases that clash with russh. It runs as a helper process next to `aurora` and streams only
changed screen regions. Two small fixes to IronRDP live in `crates/aurora-rdp/vendor`
(marked `AURORA PATCH`).

Releases: start the "Release" workflow under *Actions* (version input) or push a tag `v<version>` –
GitHub Actions builds Windows, Linux and macOS and publishes everything in one release including
`SHA256SUMS.txt` (used by the in-app updater). Windows builds are code-signed when the repository
secrets `WINDOWS_CERT_PFX` (base64 .pfx) and `WINDOWS_CERT_PASSWORD` are set.

Translations: German is the source language; English texts live in `src/i18n/*.rs`
(`t!("…")` in the code). A test fails if a text has no translation.

## 🔒 Data & security

* `library.json` – hosts, groups, snippets, settings, trusted servers (no secrets)
* `vault.json` – keychain, encrypted
* Location: `~/.local/share/AuroraTerm`, `%APPDATA%\Aurora\AuroraTerm\data`,
  `~/Library/Application Support/dev.Aurora.AuroraTerm` or `AURORA_DATA_DIR`

Server keys (SSH) and TLS certificates (RDP) are confirmed and pinned on first connect.
Details and review results: [SECURITY.md](SECURITY.md).

## 🧱 Architecture

```
src/
├── app.rs             state, tabs, session lifecycle, layout
├── terminal.rs        vt100 + renderer + keyboard → escape sequences, search, zoom
├── theme.rs           design system: 26 palettes, fonts, widgets (buttons, cards, forms, sheets)
├── update.rs          update check (GitHub releases) and self-update
├── i18n/              English texts for the German source strings
├── model.rs / store.rs  data, persistence, vault, export
├── backend/           ssh (incl. jump host), sftp (incl. server⇄server), tunnel, scan, serial, rdp, reach,
│                      vnc (RFB client), telnet, local (pseudo terminal), container, logs
└── ui/                sidebar/rail and session headers (chrome), line icons, hosts, keychain, snippets,
                       files, RDP, VNC, log viewer, containers, settings, help, editors, dialogs, palette
assets/fonts/          Inter (UI) and IBM Plex Mono (terminal), both SIL OFL
crates/aurora-rdp/     IronRDP helper process (stdio protocol)
crates/aurora-web/     web interface helper (wry/tao, JSON lines over stdio)
packaging/             .deb/.tar.gz script, Inno Setup installer, macOS .app/.dmg script
```
