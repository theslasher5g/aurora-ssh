//! English texts for the German source strings (see `super::tr`): 0.6.3 – short-lived
//! SSH certificates (Aurora CA) and host key change verification.

pub const T: &[(&str, &str)] = &[
    (
        "Angemeldet mit Kurzzeit-Zertifikat (gültig bis {})",
        "Logged in with a short-lived certificate (valid until {})",
    ),
    (
        "Anmeldung mit Kurzzeit-Zertifikat",
        "Log in with short-lived certificate",
    ),
    ("Auf Server einrichten…", "Set up on server…"),
    (
        "Auf dem Server den öffentlichen Schlüssel nach {} legen und in /etc/ssh/sshd_config „TrustedUserCAKeys {}“ eintragen, dann sshd neu laden. Danach im Host-Editor „Anmeldung mit Kurzzeit-Zertifikat“ einschalten.",
        "On the server, put the public key into {} and add “TrustedUserCAKeys {}” to /etc/ssh/sshd_config, then reload sshd. Afterwards switch on “Log in with short-lived certificate” in the host editor.",
    ),
    (
        "Auf der Server-Konsole ausführen (z.B. Proxmox noVNC, iLO/IPMI, Cloud-Konsole) und mit dem Fingerprint oben vergleichen:",
        "Run this on the server console (e.g. Proxmox noVNC, iLO/IPMI, cloud console) and compare with the fingerprint above:",
    ),
    (
        "Aurora kann als kleine Zertifizierungsstelle arbeiten: pro Verbindung entsteht ein frischer Schlüssel, den die CA für 1–24 Stunden signiert. Server vertrauen der CA einmalig (TrustedUserCAKeys) – keine authorized_keys-Pflege, abgelaufene Zertifikate sind wertlos.",
        "Aurora can act as a small certificate authority: every connection gets a fresh key that the CA signs for 1–24 hours. Servers trust the CA once (TrustedUserCAKeys) – no authorized_keys upkeep, and expired certificates are worthless.",
    ),
    ("Aurora-CA", "Aurora CA"),
    ("Befehle anzeigen", "Show commands"),
    ("Befehle ausblenden", "Hide commands"),
    ("Befehle kopieren", "Copy commands"),
    ("Befehle kopiert", "Commands copied"),
    (
        "Benutzernamen, für die das Zertifikat gilt (Komma-getrennt)",
        "User names the certificate is valid for (comma-separated)",
    ),
    ("Bestätigt durch {}", "Confirmed by {}"),
    ("CA gelöscht", "CA deleted"),
    ("CA löschen…", "Delete CA…"),
    (
        "CA wirklich löschen? Hosts mit Kurzzeit-Zertifikat melden sich dann wieder mit ihrer Identität an.",
        "Really delete the CA? Hosts with short-lived certificates then log in with their identity again.",
    ),
    ("CA-Schlüssel erzeugen", "Create CA key"),
    ("CA-Schlüssel erzeugt", "CA key created"),
    ("DNS (SSHFP)", "DNS (SSHFP)"),
    ("DNS-Abfrage fehlgeschlagen: {}", "DNS query failed: {}"),
    ("DNS-Fehler {}", "DNS error {}"),
    ("DNS-Server antwortet nicht", "DNS server does not answer"),
    ("DNSSEC-validiert", "DNSSEC validated"),
    (
        "Der Befehl steht im Terminal – prüfen und mit Enter ausführen",
        "The command is typed into the terminal – review it and press Enter to run it",
    ),
    (
        "Eine zweite Quelle widerspricht. Wenn du trotzdem verbinden willst, tippe den Hostnamen ein: {}",
        "A second source disagrees. To connect anyway, type the host name: {}",
    ),
    (
        "Erstellt am {} · {} Hosts melden sich mit Kurzzeit-Zertifikat an",
        "Created {} · {} hosts log in with a short-lived certificate",
    ),
    ("Fingerabdruck vom Server", "Fingerprint from the server"),
    ("Gültigkeit", "Validity"),
    (
        "IP-Adresse – kein DNS-Name für SSHFP",
        "IP address – no DNS name for SSHFP",
    ),
    ("Jump-Host {}", "Jump host {}"),
    ("Keine zweite Quelle", "No second source"),
    ("Kopieren", "Copy"),
    ("Kurzzeit-Zertifikat", "Short-lived certificate"),
    (
        "Kurzzeit-Zertifikat nicht angenommen ({}) – Anmeldung mit der normalen Identität",
        "Short-lived certificate not accepted ({}) – logging in with the regular identity",
    ),
    (
        "Kurzzeit-Zertifikat von {} abgelehnt und keine andere Anmeldung hinterlegt. Ist die Aurora-CA auf dem Server eingerichtet (TrustedUserCAKeys) und erlaubt das Zertifikat den Benutzer „{}“ (Principals: {})?",
        "{} rejected the short-lived certificate and no other login is stored. Is the Aurora CA set up on the server (TrustedUserCAKeys) and does the certificate allow the user “{}” (principals: {})?",
    ),
    (
        "Kurzzeit-Zertifikate statt verteilter Schlüssel",
        "Short-lived certificates instead of scattered keys",
    ),
    (
        "Name existiert nicht (NXDOMAIN)",
        "Name does not exist (NXDOMAIN)",
    ),
    (
        "Noch keine CA – lege sie unter Schlüssel → Zertifikats-Aussteller (CA) an.",
        "No CA yet – create it under Keys → Certificate authority (CA).",
    ),
    ("Principals", "Principals"),
    (
        "Pro Verbindung erzeugt Aurora einen neuen Ed25519-Schlüssel (nie gespeichert) und signiert ihn mit der CA aus dem Schlüsselbund. Klappt das nicht, meldet sich Aurora mit der Identität an.",
        "For every connection Aurora creates a new Ed25519 key (never stored) and signs it with the CA from the keychain. If that fails, Aurora logs in with the identity.",
    ),
    (
        "Proxmox-Gast – keine zweite Quelle automatisch; vergleiche über die Konsole",
        "Proxmox guest – no automatic second source; compare via the console",
    ),
    ("Prüfe zweite Quellen…", "Checking second sources…"),
    ("SSHFP {} {} passt", "SSHFP {} {} matches"),
    (
        "SSHFP-Einträge nur für andere Schlüsseltypen, nicht für {}",
        "SSHFP records only for other key types, not for {}",
    ),
    (
        "Schlüsseltyp {} hat keine SSHFP-Nummer",
        "Key type {} has no SSHFP number",
    ),
    ("Server einrichten", "Set up servers"),
    ("Server-Schlüssel", "Server keys"),
    ("Widerspricht: {}", "Contradicted by: {}"),
    (
        "Zertifikat ohne Principal (Benutzer) – nicht ausgestellt",
        "Certificate without principal (user) – not issued",
    ),
    ("Zertifikats-Aussteller (CA)", "Certificate authority (CA)"),
    ("aktiv", "active"),
    ("aktuell", "current"),
    ("alt", "old"),
    ("ersetzt am {}", "replaced {}"),
    (
        "in /etc/ssh/sshd_config eintragen (vor dem ersten Match-Block):",
        "add to /etc/ssh/sshd_config (before the first Match block):",
    ),
    ("kein DNS-Server gefunden", "no DNS server found"),
    (
        "kein voller DNS-Name (FQDN) – SSHFP übersprungen",
        "not a fully qualified DNS name – SSHFP skipped",
    ),
    ("keine SSHFP-Einträge im DNS", "no SSHFP records in DNS"),
    ("leer = Login-Benutzer", "empty = login user"),
    ("leer = {}", "empty = {}"),
    ("oder: systemctl reload sshd", "or: systemctl reload sshd"),
    (
        "ohne DNSSEC – nur so sicher wie dein DNS",
        "without DNSSEC – only as safe as your DNS",
    ),
    (
        "ssh-keyscan auf {} fehlgeschlagen: {}",
        "ssh-keyscan on {} failed: {}",
    ),
    ("ungültige DNS-Antwort", "invalid DNS answer"),
    ("ungültiger DNS-Name", "invalid DNS name"),
    ("ungültiger Hostname", "invalid host name"),
    ("vertraut seit unbekannt", "trusted since: unknown"),
    ("vertraut seit {}", "trusted since {}"),
    (
        "vom Server abgelehnt – TrustedUserCAKeys eingerichtet, Principal passt?",
        "rejected by the server – is TrustedUserCAKeys set up and does the principal match?",
    ),
    ("wird geprüft…", "checking…"),
    (
        "{} SSHFP-Einträge für {}, keiner passt zu diesem Schlüssel",
        "{} SSHFP records for {}, none matches this key",
    ),
    ("{} h", "{} h"),
    (
        "{} liefert keine Schlüssel (ssh-keyscan fehlt oder Ziel nicht erreichbar)",
        "{} returns no keys (ssh-keyscan missing or target unreachable)",
    ),
    ("{} sieht denselben Schlüssel", "{} sees the same key"),
    (
        "{} sieht einen anderen Schlüssel: {}",
        "{} sees a different key: {}",
    ),
    ("{} sieht keinen {}-Schlüssel", "{} sees no {} key"),
    (
        "Öffentlichen Schlüssel der CA kopieren",
        "Copy the CA's public key",
    ),
    ("Öffentlicher CA-Schlüssel kopiert", "CA public key copied"),
    (
        "Öffnet ein Terminal zum Host und tippt die Befehle (mit sudo) vor – nichts wird ausgeführt, bevor du Enter drückst.",
        "Opens a terminal to the host and types the commands (with sudo) – nothing runs until you press Enter.",
    ),
    ("— SSH-Host wählen —", "— choose SSH host —"),
];
