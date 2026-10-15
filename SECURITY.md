# Sicherheit

## Modell

| Bereich | Schutz |
|---|---|
| Keychain | `vault.json`, Argon2id (64 MiB, t=3) → XChaCha20-Poly1305, frische Nonce pro Speichern, Dateirechte `0600` |
| SSH / SFTP | Server-Keys per Trust-on-First-Use (SHA256), deutliche Warnung bei geändertem Key |
| RDP | TLS-Server-Key wird ebenfalls gepinnt (TOFU), **bevor** Zugangsdaten gesendet werden |
| Prozesse | Passwörter gehen über stdin-Pipes an `aurora-rdp` / FreeRDP, nie über argv oder Umgebungsvariablen |

| Updates | Nur HTTPS (System-Zertifikatsspeicher); Download wird gegen `SHA256SUMS.txt` des Releases geprüft, sonst verworfen. Weiterleitungen nur auf `https://` |
| Server ⇄ Server | Daten laufen durch Aurora (keine Zugangsdaten auf den Servern nötig); Dateinamen mit `..`/Trennern werden übersprungen |
| Erreichbarkeit | Reiner TCP-Verbindungsaufbau zum Host-Port, es werden keine Daten gesendet; abschaltbar in den Einstellungen |
| Sperren | Tresor sperrt nach einstellbarer Inaktivität (Standard 15 min); Schlüssel werden aus dem Speicher genommen, offene Sessions laufen weiter |
| PROD-Schutz | Gefährliche Befehle (`rm -r`, `reboot`, `mkfs`, `DROP TABLE` …) werden auf PROD-Hosts vor dem Absenden bestätigt – eine Komfort-Bremse, keine Sicherheitsgrenze |

## Security-Review 0.1 (behobene Funde)

| # | Schwere | Fund | Fix |
|---|---|---|---|
| 1 | Hoch | **SFTP-Path-Traversal**: Dateinamen vom Server (z. B. `../../.bashrc`) wurden beim (rekursiven) Download ungeprüft an den lokalen Zielpfad gehängt. | `safe_local_name` lehnt `..`, Trenner, Laufwerks-Präfixe und Steuerzeichen ab; betroffene Einträge werden übersprungen und gemeldet. |
| 2 | Hoch | **RDP ohne Zertifikatsprüfung**: `ironrdp-tls` akzeptiert jedes Zertifikat; bei Servern ohne NLA ginge das Passwort an einen MITM. | Hook in `vendor/ironrdp-client`: Fingerprint des Server-Keys wird vor CredSSP/ClientInfo geprüft, unbekannt/geändert → Dialog in der App. |
| 3 | Mittel | **Paste-Jacking**: eingefügter Text konnte `ESC[201~` enthalten, den Bracketed-Paste-Modus beenden und Befehle direkt ausführen. | Start/Ende-Marker werden aus eingefügtem Text entfernt. |
| 4 | Mittel | **Windows `cmdkey`**: Das RDP-Passwort wurde dauerhaft im Credential Manager abgelegt und stand kurz in der Kommandozeile. | Entfernt; `mstsc` fragt selbst (Standard ist ohnehin der eingebettete Viewer). |
| 5 | Mittel | **Binary Planting**: Release-Builds suchten `aurora-rdp` auch relativ zum Arbeitsverzeichnis – ein fremdes Binary hätte das RDP-Passwort erhalten. | Relative Suchpfade nur noch in Debug-Builds; Release sucht nur neben der eigenen EXE. |
| 6 | Niedrig | **Clipboard-Sync** lief immer automatisch, auch wenn „Zwischenablage“ am Host deaktiviert war. | Auto-Sync folgt der Host-Einstellung; die Buttons funktionieren weiterhin manuell. |

## Bekannte Grenzen

* Der entsperrte Vault liegt im Speicher (kein Zeroize); offene Sessions halten ihre Zugangsdaten bis zum Schließen.
* Die Prüfsummen stammen aus demselben GitHub-Release wie die Pakete (schützt vor defekten/abgebrochenen Downloads und fremden Weiterleitungen, nicht vor einem kompromittierten GitHub-Konto). Pakete sind nicht signiert.
* Terminal-OSC-52 (Clipboard-Schreiben durch den Server) wird bewusst ignoriert.
* Keyboard-Interactive-Auth beantwortet alle Prompts mit dem gespeicherten Passwort (OTP-Prompts schlagen dann fehl).
