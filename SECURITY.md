<div align="center">

# 🔒 Security

How Aurora protects your secrets, what it checks before it trusts something, and what it
deliberately does **not** do.

[Reporting](#-reporting-a-vulnerability) · [Protection model](#-protection-model) · [Review history](#-review-history) · [Known limitations](#-known-limitations)

</div>

## 📣 Reporting a vulnerability

Please report security problems **privately**, not as a public issue:

1. Open the repository's **Security** tab → **Report a vulnerability** (GitHub private advisory).
2. Describe what you found, the affected version and, if possible, how to reproduce it.

You will get an answer as soon as possible. Fixes go into the next release; the
[CHANGELOG](CHANGELOG.md) names security fixes. Only the **latest release** is supported.

## 🎯 What Aurora defends against

| Attacker | Examples |
|---|---|
| **A malicious or compromised server** | Escape sequences in output, file names like `../../.bashrc`, crafted container names, fake prompts, a changed host key |
| **The network** | Man‑in‑the‑middle on SSH / RDP / HTTPS, a swapped update download |
| **A malicious file** you open, drop or import | Key files, import files (mRemoteNG / Royal TS / RDM / CSV), themes, session recordings, `.aurora` exports |
| **A malicious web page** in a web interface entry | Credential phishing through redirects, downloads, pop‑ups, form fields |
| **Another local user** on the same machine | Predictable temp files, readable recordings, planted binaries |

Out of scope: someone who already controls your unlocked session or your operating‑system account.

## 🛡 Protection model

### Secrets and the vault

| Area | Protection |
|---|---|
| Keychain | `vault.json`: Argon2id (64 MiB, t=3) → XChaCha20‑Poly1305, fresh nonce on every save, atomic write, file mode `0600`. KDF parameters stored in the file are ignored (no downgrade) |
| Auto‑lock | The vault locks after a configurable idle time (default 15 min); keys are dropped from memory, open sessions keep running |
| Password managers | Bitwarden, 1Password, KeePassXC, Proton Pass: only a **reference** is stored. The CLI runs without a shell, with a cleared environment (allow‑list), `--` before references, secrets via stdin/env, stderr never shown |
| Processes | Passwords reach `aurora-rdp`, `aurora-web` and FreeRDP via stdin pipes, never via argv or environment variables |
| Export | Own passphrase, Argon2id + XChaCha20‑Poly1305, file mode `0600` |
| SSH key import | Dropped key files: size cap (64 KB), regular files only, nothing logged; the key goes straight into the vault |
| Import of other tools | Encrypted passwords are never decrypted. Clear‑text CSV passwords go to the vault only after an explicit opt‑in |

### Trust and verification

| Area | Protection |
|---|---|
| SSH / SFTP | Server keys trust‑on‑first‑use (SHA256), clear warning when a key changes; a contradicting second source (SSHFP, jump host) requires typing the host name |
| Jump host | The target's key is verified exactly like a direct connection (own fingerprint per `host:port`) |
| RDP | The TLS server key is pinned (TOFU) **before** any credentials are sent |
| Proxmox | Pinned certificate fingerprint; API token or ticket in headers, never in URLs |
| WinRM | HTTPS certificate pinned by SHA‑256 fingerprint (TOFU, same dialog as SSH/RDP, loud warning on change); NTLMv2 with MIC, no password on the wire; Basic only over HTTPS; plain HTTP (5985) only for NTLM and shown as unencrypted. Batch runs never open a dialog – an unconfirmed certificate is a failure |
| SSH certificates | Aurora as a small CA: principals are required, the key is ephemeral, the signature is bound to the session |
| Updates | HTTPS only; the download is verified against the release's `SHA256SUMS.txt` or discarded. Redirects only to `https://`. Downloaded into a private, randomly named directory (0700) |

### Content from remote systems is untrusted

| Area | Protection |
|---|---|
| Terminal | Paste protection: all control characters (including a rebuilt end‑of‑paste marker) are removed from pasted text; OSC 52 clipboard writes by the server are ignored; replies to terminal queries are fixed strings |
| Server ⇄ server / SFTP | File names with `..`, separators, drive prefixes or control characters are skipped; remote symlinks are not followed in recursive copies |
| Editing remote files | Private, randomly named temp directory (0700); executable types are not opened without an explicitly configured editor; Windows always uses Notepad |
| Containers | Names and options are shell‑quoted before they go into `docker`/`kubectl` on an SSH host; on this PC the CLI starts directly (no shell) and is only looked up in `PATH` |
| Log viewer | Read‑only: `tail -F <quoted path>` or your own command, on its own channel; escape sequences are stripped; at most 20 000 lines are kept |
| Server facts and info snippets | Read‑only commands on a separate channel; output is truncated, control characters stripped, shown as plain text only |
| Run on many hosts | Confirmation with host count and command; hosts in a maintenance window are skipped unless you opt in; PROD hosts need an extra confirmation, a dangerous command (`rm -r`, `reboot`, `systemctl stop` … also inside chains, `sudo`, `xargs`, `sh -c`) on PROD needs the word typed. Placeholder values must be single‑line; values of secret‑looking placeholders (`password`, `token`, `key` …) are masked in the displayed command, the exports and every result. Output is capped (64 KB per host). `sudo` only as `sudo -n`; no password is ever stored or sent. Only stored credentials and already confirmed server keys/certificates are used |
| Live host statistics | One persistent exec channel per watched host with a constant read‑only shell loop (`/proc`, `df`); only while the host is visible or its sheet is open (at most 10 hosts), the interval is a clamped integer; at most 64 KB of unexpected output is buffered |
| Exports | Markdown and HTML escape all remote data (HTML carries a CSP, no scripts); CSV neutralises formulas; IP addresses can be masked |
| Fonts | Font files are parsed and checked before they reach the renderer |

### Web interfaces (`aurora-web` helper)

| Area | Protection |
|---|---|
| Addresses | Only `http`/`https` without embedded credentials – the same check runs in Aurora and in the helper |
| Isolation | Own profile directory per entry (0700) – cookies and logins are never shared |
| Certificates | Strict, **pinned SHA‑256 fingerprint** (confirmed by you, enforced in the helper, loud warning on change) or ignored by explicit opt‑in |
| Auto‑login | Credentials only travel over the helper's stdin pipe; the page script re‑checks the origin and escapes every value; at most one automatic submit per opening |
| Navigation | Limited to the entry's origin (plus listed hosts); other links open in the system browser after re‑validation; no second windows; no developer tools in release builds |
| Downloads | Sanitised file name in `Downloads/Aurora`, unique name, never executed |
| Import | An import that would change a web entry's origin drops its stored login |

### Network and local system

| Area | Protection |
|---|---|
| Tunnels | Listen on `127.0.0.1` only |
| Reachability / scans | Plain TCP connects (and the SSH banner); nothing logs in; can be turned off |
| SSH key setup | The public key is checked against an allow‑list of characters before it goes into the shell command; afterwards a key‑only login is verified |
| Recordings and history | Recordings are created owner‑only (`0600` on Linux/macOS); deleting uses plain `*.cast` file names only |
| Helpers | Release builds look for `aurora-rdp` / `aurora-web` only next to their own executable |
| PROD guard | Dangerous commands must be confirmed on PROD hosts – a **safety net, not a security boundary** |
| Telnet / VNC | Unencrypted by design (see limitations); RFB data is bounds‑checked and cut text is capped at 1 MiB |

## 🔍 Review history

Every release since 0.1 gets a security review; the last ones were done with a second, independent
pass. Findings that were actually exploitable are listed with their fix.

| Version | # | Severity | Finding | Fix |
|---|---|---|---|---|
| 0.1 | 1 | High | **SFTP path traversal** – server file names like `../../.bashrc` were appended to the local path | `safe_local_name` rejects `..`, separators, drive prefixes, control characters |
| 0.1 | 2 | High | **RDP without certificate check** – a MITM would have received the password on servers without NLA | Fingerprint is checked before CredSSP; unknown/changed → dialog |
| 0.1 | 3 | Medium | **Paste‑jacking** – pasted `ESC[201~` ended bracketed paste | Markers stripped *(bypass found and fixed later, see 0.9.1)* |
| 0.1 | 4 | Medium | **Windows `cmdkey`** stored the RDP password permanently | Removed |
| 0.1 | 5 | Medium | **Binary planting** – `aurora-rdp` was searched in the working directory | Release builds only look next to the executable |
| 0.1 | 6 | Low | **Clipboard sync** ignored the host setting | Auto‑sync follows the host setting |
| 0.5 | 1 | Medium | **Opening remote files** – a server‑named `.cmd`/`.hta`/`.js` could have been executed | Notepad only; executable types refused |
| 0.5 | 2 | Medium | **Predictable temp directories** – a local user could swap a verified update | Private, random directories with mode 0700 |
| 0.9.0 | 1 | Medium | **Import "overwrite" could redirect a web entry with auto‑login** to an attacker's URL | A different origin drops auto‑login and the stored identity |
| 0.9.1 | 1 | Medium | **Paste‑jacking bypass** – `ESC[2` + `ESC[201~` + `01~` rebuilt the end marker after stripping | All control characters (except tab/line breaks) are removed |
| 0.9.1 | 2 | Low | **Readable recordings** – raw terminal output created with the default umask | Created `0600` |

Releases 0.6 – 0.8 were reviewed without exploitable findings; hardening from those reviews:
recordings are only opened/deleted by plain file names, Markdown export titles are escaped.

## ⚠ Known limitations

* The unlocked vault lives in memory (no zeroize); open sessions keep their credentials until closed.
* **Supply chain:** the checksums come from the same GitHub release as the packages – they protect against
  broken downloads and foreign redirects, not against a compromised GitHub account. Windows builds are only
  signed when a certificate is configured; the macOS app is ad‑hoc signed, not notarized.
* **Web interfaces:** on macOS there is no certificate pinning (the system web view offers no hook) – use a
  trusted certificate or "ignore" explicitly. A web page can ask Aurora to open a link in the system browser or start
  a download without a click. The single automatic submit is spent when the script arms itself, not on the click.
* The server‑side `dangerous_command` check is easy to bypass (`ls; rm -rf /`) – it prevents accidents only.
* Keyboard‑interactive auth answers every prompt with the stored password (OTP prompts fail).
* Jump hosts need stored credentials (identity).
* Telnet and VNC are unencrypted protocols; Apple Remote Desktop / VeNCrypt / TLS security types are not supported.
* Importing a `.aurora` bundle can pre‑seed known server keys; only import bundles you trust (the bundle password is required).
* The session history file is cleaned without file locking; the info‑snippet and export output can contain user names.
