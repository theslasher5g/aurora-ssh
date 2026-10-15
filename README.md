# Aurora

Moderner Client für **SSH, SFTP, RDP und serielle Konsolen** – in Rust, schlicht, schnell, mit
verschlüsselter Keychain, Snippets, Host-Gruppen und umschaltbaren Themes

![Hosts](docs/screenshots/hosts.png)

## Download

Unter [Releases](../../releases) liegen Windows und Linux im selben Release:

| System | Datei | Installation |
|---|---|---|
| **Windows** (x64) | `Aurora-<version>-windows-x64-setup.exe` | Installer ausführen |
| | `Aurora-<version>-windows-x64-portable.zip` | entpacken, `Aurora.exe` starten |
| **Linux** (x86_64) | `aurora_<version>_amd64.deb` | `sudo apt install ./aurora_<version>_amd64.deb` |
| | `aurora-<version>-linux-x86_64.tar.gz` | entpacken, `./install.sh` oder `./aurora` |

## Features

| Bereich | |
|---|---|
| **SSH** | Eigener Terminal-Emulator (256 Farben/Truecolor, Scrollback, Auswahl, Bracketed Paste, Resize), Key/Passwort/Keyboard-Interactive, Keepalive, Reconnect per Enter |
| **Dateien (SFTP)** | Eigener Bereich in der Leiste. Zwei Spalten, jede Seite **lokal oder ein beliebiger Server** – auch **Server ⇄ Server** (wird über Aurora gestreamt, die Server müssen sich nicht gegenseitig erreichen). Windows-Server mit OpenSSH (`C:\pfad` wird übersetzt, Laufwerksliste lokal), RDP-Hosts bekommen ihren SFTP-Port. Rekursive Transfers mit Fortschritt, Drag & Drop |
| **Splitscreen** | Bis zu **4 Sessions nebeneinander** (Terminal, Dateien, RDP gemischt) – über das ◫-Symbol im Session-Kopf oder Rechtsklick auf einen Port |
| **RDP** | Direkt im Tab (IronRDP): passt sich der Fenstergröße an, **Zwischenablage in beide Richtungen** (automatisch + Buttons „vom Host kopieren"/„an Host senden"), **Ctrl+Alt+Del**, Windows-Taste, Zwischenablage als Tastatureingabe tippen (für Login-Masken/Konsolen). Optional im System-Client (mstsc, FreeRDP, Remmina) |
| **Serial** | Port-Erkennung, Baudrate, Format, Flusskontrolle, Zeilenende, lokales Echo |
| **Keychain** | Passwörter & SSH-Keys, Ed25519-Generator, verschlüsselt mit Argon2id + XChaCha20-Poly1305 |
| **Snippets** | Platzhalter `{{variable}}` mit Live-Vorschau, Snippet-Leiste im Terminal, in allen Sessions ausführen, Startup-Snippet pro Host |
| **Gruppen** | Verschachtelt, Farbe & Icon, vererbte Standard-User/-Identität, Favoriten, Zuletzt verbunden, Tags |
| **Design** | Eigenes „Patchbay“-Design: Sessions als nummerierte Ports mit Status-LED in der Leiste, IBM Plex Sans/Mono. 12 Themes (Phosphor, Glacier, Nord, Dracula, Paper, Tokyo Night, Gruvbox, Catppuccin, Rosé Pine, Solarized Dark/Light, GitHub Light) – das Terminal übernimmt jeweils die passenden Farben. Befehlspalette `Ctrl/⌘+K`, Broadcast-Modus |
| **Komfort** | Erreichbarkeits-LED je Host (TCP-Check mit Latenz), **Update-Check** beim Start mit Ein-Klick-Update (SHA256-geprüft) |

| | |
|---|---|
| ![Splitscreen](docs/screenshots/split.png) | ![Server ⇄ Server](docs/screenshots/sftp.png) |
| ![Dateien](docs/screenshots/files.png) | ![Tokyo Night](docs/screenshots/tokyo.png) |
| ![Terminal](docs/screenshots/terminal.png) | ![GitHub Light](docs/screenshots/light.png) |
| ![RDP](docs/screenshots/rdp.png) | ![Paper-Theme](docs/screenshots/paper.png) |
| ![Befehlspalette](docs/screenshots/palette.png) | ![Snippets](docs/screenshots/snippets.png) |
| ![Einstellungen & Themes](docs/screenshots/themes.png) | ![Tresor](docs/screenshots/lock.png) |

## Tastenkürzel

| Kürzel | Aktion |
|---|---|
| `Ctrl/⌘ + K` | Befehlspalette (Hosts, Snippets, Themes, `user@host`) |
| `Ctrl + 1…9` | Zu Port/Session 1…9 springen, `Ctrl + 0` zurück zur Übersicht |
| `Ctrl + Tab` | Nächste Session |
| `Ctrl/⌘ + Shift + W` | Session schließen |
| `Ctrl + Shift + C` / Auswahl | Kopieren im Terminal |
| `Enter` in getrennter Session | Neu verbinden |

## Selbst bauen

Rust ≥ 1.88:

```bash
cargo build --release                                              # App
cargo build --release --manifest-path crates/aurora-rdp/Cargo.toml # RDP-Komponente
cargo test
```

Die RDP-Komponente ist ein eigenes Crate (`crates/aurora-rdp`), weil IronRDP Krypto-Vorabversionen
pinnt, die mit russh kollidieren. Sie läuft als Hilfsprozess neben `aurora` und streamt nur
geänderte Bildbereiche. Zwei kleine Fixes an IronRDP liegen unter `crates/aurora-rdp/vendor`
(markiert mit `AURORA PATCH`).

Releases: Workflow „Release“ unter *Actions* manuell starten (Version) oder Tag `v<version>` pushen –
GitHub Actions baut Windows und Linux und veröffentlicht beides in einem Release inkl. `SHA256SUMS.txt`
(die prüft das In-App-Update).

## Daten & Sicherheit

* `library.json` – Hosts, Gruppen, Snippets, Einstellungen, vertrauenswürdige Server (keine Geheimnisse)
* `vault.json` – Keychain, verschlüsselt
* Ort: `~/.local/share/AuroraTerm`, `%APPDATA%\Aurora\AuroraTerm\data` oder `AURORA_DATA_DIR`

Server-Keys (SSH) und TLS-Zertifikate (RDP) werden beim ersten Verbinden bestätigt und gepinnt.
Details und Review-Ergebnisse: [SECURITY.md](SECURITY.md).

## Architektur

```
src/
├── app.rs             Zustand, Tabs, Session-Lifecycle, Layout
├── terminal.rs        vt100 + Renderer + Tastatur→Escape-Sequenzen
├── theme.rs           Design-System „Patchbay“: Paletten, Fonts, Widgets
├── update.rs          Update-Check (GitHub Releases) und Selbst-Update
├── model.rs / store.rs  Daten, Persistenz, Vault
├── backend/           ssh, sftp (inkl. Server⇄Server), serial, rdp, rdp_embed, reach
└── ui/                Port-Leiste (chrome), Hosts, Keychain, Snippets, SFTP, RDP, Einstellungen, Dialoge, Palette
assets/fonts/          IBM Plex Sans/Mono (SIL OFL)
crates/aurora-rdp/     IronRDP-Hilfsprozess (stdio-Protokoll)
packaging/             .deb/.tar.gz-Skript, Inno-Setup-Installer
```
