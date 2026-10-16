# Changelog

## Unreleased
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
