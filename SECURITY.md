# Security

## Model

| Area | Protection |
|---|---|
| Keychain | `vault.json`, Argon2id (64 MiB, t=3) → XChaCha20-Poly1305, fresh nonce on every save, file mode `0600` |
| SSH / SFTP | Server keys trust-on-first-use (SHA256), clear warning when a key changes |
| RDP | The TLS server key is pinned as well (TOFU), **before** any credentials are sent |
| Processes | Passwords reach `aurora-rdp` / FreeRDP via stdin pipes, never via argv or environment variables |
| Updates | HTTPS only (system certificate store); the download is verified against the release's `SHA256SUMS.txt` or discarded. Redirects only to `https://`. Downloaded into a private, randomly named directory (0700) so nobody can swap the verified file before it is installed |
| Server ⇄ server | Data flows through Aurora (no credentials on the servers); file names with `..`/separators are skipped |
| Reachability | Plain TCP connect to the host port, no data is sent; can be turned off in Settings |
| Auto-lock | The vault locks after a configurable idle time (default 15 min); keys are dropped from memory, open sessions keep running |
| PROD guard | Dangerous commands (`rm -r`, `reboot`, `mkfs`, `DROP TABLE` …) must be confirmed on PROD hosts – a safety net, not a security boundary |
| Server facts | After login only read-only commands run on a separate channel (`/etc/os-release`, `hostname`, `/proc/uptime`, `df`, `apt-get -s upgrade`); output is truncated and stripped of control characters |
| Network scan | Only TCP connects (+ reading the SSH banner), at most a /22, nothing logs in |
| SSH key setup | The public key is checked against an allow-list of characters before it goes into the shell command; afterwards a key-only login is verified |
| Tunnels | Listen on `127.0.0.1` only, never on all interfaces |
| Jump host | The target's server key is verified exactly like a direct connection (own fingerprint per `host:port`) |
| Editing remote files | Downloaded into a private, randomly named temp directory (0700); executable types (`.exe`, `.cmd`, `.hta`, `.js`, `.desktop` …) are not opened without an explicitly configured editor; on Windows always Notepad instead of the default app |
| Export | Own passphrase, Argon2id + XChaCha20-Poly1305, file mode `0600` |
| Containers | Container/pod names and options are shell-quoted before they go into the `docker`/`kubectl` command on an SSH host; on this PC the CLI is started directly (no shell) and only looked up in `PATH` – never in the current directory |
| Log viewer | Read-only: `tail -F <quoted path>` or the command you entered, on its own channel; escape sequences are stripped from the output, at most 20,000 lines are kept |
| Telnet | Unencrypted by design (only for devices that can't do SSH). Auto-login types the stored password only once and only after a password prompt |
| VNC | VNC auth (DES challenge) or none; RFB traffic itself is unencrypted – over the internet use a tunnel (SSH host → tunnel to port 5900). All server data is bounds-checked (rectangles clipped, cut text capped at 1 MiB) |
| Web interfaces | Only `http`/`https` addresses without embedded credentials (the same check runs in Aurora and in the `aurora-web` helper). Each entry has a private profile directory (0700). Certificate errors: strict, **pinned SHA-256 fingerprint** (confirmed by the user, enforced in the helper, loud warning on change) or ignored by explicit opt-in. Credentials only travel over the helper's stdin pipe and are only filled on the entry's own origin, at most one automatic submit per open. Navigation is limited to the entry's origin (plus listed hosts); other links open in the system browser after re-validation; no second windows, no dev tools in release builds; downloads get a sanitized name in `Downloads/Aurora`. Protocol lines are bounded to 64 KiB |
| Fonts | Font files are parsed and checked before they are handed to the renderer; broken files fall back to IBM Plex Mono |

## Security review 0.1 (fixed)

| # | Severity | Finding | Fix |
|---|---|---|---|
| 1 | High | **SFTP path traversal**: file names from the server (e.g. `../../.bashrc`) were appended to the local target path unchecked during (recursive) downloads. | `safe_local_name` rejects `..`, separators, drive prefixes and control characters; such entries are skipped and reported. |
| 2 | High | **RDP without certificate check**: `ironrdp-tls` accepts any certificate; on servers without NLA the password would go to a MITM. | Hook in `vendor/ironrdp-client`: the server key fingerprint is checked before CredSSP/ClientInfo; unknown/changed → dialog in the app. |
| 3 | Medium | **Paste-jacking**: pasted text could contain `ESC[201~`, end bracketed-paste mode and run commands directly. | Start/end markers are stripped from pasted text. |
| 4 | Medium | **Windows `cmdkey`**: the RDP password was stored permanently in the Credential Manager and briefly appeared on a command line. | Removed; `mstsc` asks itself (the embedded viewer is the default anyway). |
| 5 | Medium | **Binary planting**: release builds also looked for `aurora-rdp` relative to the working directory – a foreign binary would have received the RDP password. | Relative search paths only in debug builds; release builds only look next to their own executable. |
| 6 | Low | **Clipboard sync** always ran automatically, even when "clipboard" was disabled for the host. | Auto-sync follows the host setting; the buttons still work manually. |

## Security review 0.5 (fixed)

| # | Severity | Finding | Fix |
|---|---|---|---|
| 1 | Medium | **Opening remote files**: "edit" handed a downloaded file to the OS default handler – on Windows a server-named `.cmd`/`.hta`/`.js` would have been executed. | Windows always uses Notepad; executable types are refused without a configured editor. |
| 2 | Medium | **Predictable temp directories** (Linux, multi-user): another local user could pre-create `/tmp/aurora-update-<version>` and swap the verified `.deb` before `pkexec apt-get` read it. | Private, randomly named directories created with mode 0700 (`store::private_temp_dir`) for updates and edited files. |

## Known limitations

* The unlocked vault lives in memory (no zeroize); open sessions keep their credentials until closed.
* The checksums come from the same GitHub release as the packages (protects against broken/truncated downloads and foreign redirects, not against a compromised GitHub account). Windows builds are only signed when a certificate is configured; the macOS app is ad-hoc signed, not notarized.
* Terminal OSC 52 (clipboard writes by the server) is deliberately ignored.
* Keyboard-interactive auth answers every prompt with the stored password (OTP prompts fail).
* Jump hosts need stored credentials (identity); chains of several jump hosts are not supported.
* Telnet and VNC are unencrypted protocols; Apple Remote Desktop / VeNCrypt / TLS security types are not supported (macOS: enable "VNC viewers may control screen with password").
