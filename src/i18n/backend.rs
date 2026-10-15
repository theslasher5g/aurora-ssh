//! English texts for the German source strings (see `super::tr`).

pub const T: &[(&str, &str)] = &[
    // rdp
    (
        "Kein RDP-Client gefunden. Installiere FreeRDP (xfreerdp) oder Remmina.",
        "No RDP client found. Install FreeRDP (xfreerdp) or Remmina.",
    ),
    (
        "RDP-Komponente (aurora-rdp) nicht gefunden",
        "RDP component (aurora-rdp) not found",
    ),
    (
        "RDP-Komponente unerwartet beendet",
        "RDP component exited unexpectedly",
    ),
    // tunnel
    (
        "Port {} ist schon belegt ({})",
        "Port {} is already in use ({})",
    ),
    ("SSH-Verbindung getrennt", "SSH connection lost"),
    // serial
    ("Öffne {} @ {} baud…", "Opening {} @ {} baud…"),
    (
        "Dateien hochladen geht nur bei SSH-Sessions",
        "File upload only works in SSH sessions",
    ),
    // ssh
    (
        "Zeitüberschreitung – Host nicht erreichbar",
        "Timed out – host unreachable",
    ),
    ("Jump-Host {}: {}", "Jump host {}: {}"),
    (
        "Jump-Host {} erreicht {}:{} nicht ({})",
        "Jump host {} cannot reach {}:{} ({})",
    ),
    (
        "Zeitüberschreitung über den Jump-Host",
        "Timed out via the jump host",
    ),
    (
        "SSH-Key konnte nicht gelesen werden – stimmt die Passphrase? ({})",
        "Could not read the SSH key – is the passphrase correct? ({})",
    ),
    (
        "{} erlaubt keine Anmeldung per Passwort – nur per SSH-Key. Hinterlege einen Key in der Identität des Hosts.",
        "{} does not allow password login – SSH key only. Add a key to the host's identity.",
    ),
    (
        "Der SSH-Key wurde von {} für „{}“ nicht angenommen – ist er in ~/.ssh/authorized_keys eingetragen? (Rechtsklick → SSH-Key einrichten)",
        "{} did not accept the SSH key for \"{}\" – is it in ~/.ssh/authorized_keys? (Right-click → Set up SSH key)",
    ),
    (
        "{} erwartet ein Passwort für „{}“",
        "{} expects a password for \"{}\"",
    ),
    (
        "Anmeldung als „{}“ an {} abgelehnt – Benutzername oder Passwort falsch? (root-Login ist auf vielen Servern gesperrt)",
        "Login as \"{}\" to {} rejected – wrong username or password? (root login is disabled on many servers)",
    ),
    ("Server-Key wurde abgelehnt", "Server key was rejected"),
    (
        "Verbindung abgelehnt – läuft der SSH-Dienst auf diesem Port?",
        "Connection refused – is the SSH service running on this port?",
    ),
    (
        "Hostname nicht gefunden – Tippfehler oder fehlt ein DNS-Eintrag?",
        "Hostname not found – typo or missing DNS entry?",
    ),
    (
        "Netzwerk nicht erreichbar – bist du im richtigen Netz / VPN?",
        "Network unreachable – are you on the right network / VPN?",
    ),
    (
        "Verbindung vom Server zurückgesetzt – evtl. Firewall oder fail2ban",
        "Connection reset by the server – possibly a firewall or fail2ban",
    ),
    (
        "Zeitüberschreitung – Host antwortet nicht (aus, Firewall oder falscher Port?)",
        "Timed out – host is not responding (off, firewall or wrong port?)",
    ),
    (
        "Server und Aurora haben keine gemeinsamen Verfahren – sehr alter SSH-Server?",
        "Server and Aurora share no common algorithms – very old SSH server?",
    ),
    (
        "Zu viele Anmeldeversuche – der Server hat die Verbindung getrennt",
        "Too many login attempts – the server closed the connection",
    ),
    (
        "Datei oder Ordner existiert nicht",
        "File or folder does not exist",
    ),
    ("Keine Berechtigung", "Permission denied"),
    ("Verbinde mit {} über {}…", "Connecting to {} via {}…"),
    ("Verbinde mit {}…", "Connecting to {}…"),
    ("Öffne Shell…", "Opening shell…"),
    ("Zeitüberschreitung", "Timed out"),
    ("Ungültiger öffentlicher Schlüssel", "Invalid public key"),
    (
        "Server hat den Schlüssel nicht übernommen (nur Linux/Unix-Server, Exit {})",
        "Server did not accept the key (Linux/Unix servers only, exit {})",
    ),
    (
        "Schlüssel eingetragen, Anmeldung damit klappt aber nicht: {}",
        "Key installed, but logging in with it fails: {}",
    ),
    // sftp
    ("unsicherer Dateiname: {}", "unsafe file name: {}"),
    (
        "{} Einträge mit unsicheren Namen übersprungen",
        "Skipped {} entries with unsafe names",
    ),
    ("abgebrochen", "cancelled"),
    ("ungültiger Pfad", "invalid path"),
    // scan
    ("Ungültige Adresse", "Invalid address"),
    ("Ungültige Maske", "Invalid mask"),
    (
        "Bitte höchstens ein /22-Netz (1024 Adressen)",
        "At most a /22 network, please (1024 addresses)",
    ),
    ("Ungültiges Ende", "Invalid end"),
    ("Ende liegt vor dem Anfang", "End is before the start"),
    (
        "Bitte höchstens 1024 Adressen",
        "At most 1024 addresses, please",
    ),
    ("Ungültiger Port: {}", "Invalid port: {}"),
    ("1 bis 8 Ports angeben", "Enter 1 to 8 ports"),
    // update
    ("GitHub nicht erreichbar", "GitHub unreachable"),
    (
        "GitHub antwortet mit HTTP {}",
        "GitHub responded with HTTP {}",
    ),
    (
        "unerwartete Antwort von GitHub",
        "unexpected response from GitHub",
    ),
    (
        "Portable Version: bitte neu herunterladen",
        "Portable version: please download again",
    ),
    (
        "Release enthält keine Datei *{}",
        "Release contains no *{} file",
    ),
    (
        "Release enthält keine Prüfsummen",
        "Release contains no checksums",
    ),
    ("keine Prüfsumme für {}", "no checksum for {}"),
    (
        "Prüfsumme stimmt nicht – Download verworfen",
        "Checksum mismatch – download discarded",
    ),
    (
        "Installer konnte nicht gestartet werden",
        "Could not start the installer",
    ),
    (
        "pkexec nicht gefunden – bitte manuell installieren",
        "pkexec not found – please install manually",
    ),
    (
        "Installation abgebrochen ({})",
        "Installation cancelled ({})",
    ),
    ("Entpacken fehlgeschlagen", "Extraction failed"),
    ("Archiv ist leer", "Archive is empty"),
    ("{} nicht beschreibbar", "{} is not writable"),
    ("Download fehlgeschlagen", "Download failed"),
    ("Weiterleitung ohne Ziel", "Redirect without target"),
    ("unsichere Weiterleitung", "unsafe redirect"),
    (
        "Download fehlgeschlagen: HTTP {}",
        "Download failed: HTTP {}",
    ),
    ("zu viele Weiterleitungen", "too many redirects"),
    // store
    ("ungültiger Salt", "invalid salt"),
    ("ungültige Nonce", "invalid nonce"),
    ("Falsches Master-Passwort", "Wrong master password"),
    ("Verschlüsselung fehlgeschlagen", "Encryption failed"),
    ("Keine Aurora-Exportdatei", "Not an Aurora export file"),
    (
        "Falsches Passwort oder beschädigte Datei",
        "Wrong password or corrupted file",
    ),
    // model: dangerous commands
    ("löscht rekursiv", "deletes recursively"),
    ("fährt den Server herunter", "shuts the server down"),
    ("startet den Server neu", "reboots the server"),
    (
        "schreibt direkt auf ein Gerät",
        "writes directly to a device",
    ),
    ("ändert den Runlevel", "changes the runlevel"),
    ("beendet Prozesse hart", "kills processes forcefully"),
    ("leert Dateien", "empties files"),
    ("ändert Rechte rekursiv", "changes permissions recursively"),
    ("formatiert ein Dateisystem", "formats a file system"),
    ("stoppt oder startet Dienste", "stops or restarts services"),
    ("löscht Datenbankinhalte", "deletes database contents"),
    ("überschreibt Systemdaten", "overwrites system data"),
];
