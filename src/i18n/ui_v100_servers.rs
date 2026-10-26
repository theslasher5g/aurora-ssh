//! English texts for the German source strings: servers (run on many hosts, live stats, WinRM).

pub const T: &[(&str, &str)] = &[
    (
        "Keine gültige NTLM-Antwort des Servers",
        "Not a valid NTLM answer from the server",
    ),
    (
        "Unerwarteter NTLM-Nachrichtentyp",
        "Unexpected NTLM message type",
    ),
    (
        "Zertifikat noch nicht bestätigt: {}",
        "Certificate not confirmed yet: {}",
    ),
    (
        "Das Zertifikat hat sich geändert: {}",
        "The certificate has changed: {}",
    ),
    ("Host {} nicht gefunden", "Host {} not found"),
    (
        "TLS-Verbindung fehlgeschlagen: {}",
        "TLS connection failed: {}",
    ),
    (
        "Die Antwort des Servers ist ungültig (Kopfzeilen zu lang)",
        "The server's answer is invalid (headers too long)",
    ),
    (
        "Der Server hat die Verbindung beendet",
        "The server closed the connection",
    ),
    (
        "Die Antwort des Servers ist kein HTTP",
        "The server's answer is not HTTP",
    ),
    (
        "Die Antwort des Servers ist zu groß",
        "The server's answer is too large",
    ),
    (
        "Die Antwort des Servers ist ungültig (Chunk-Kopf)",
        "The server's answer is invalid (chunk header)",
    ),
    (
        "Die Antwort des Servers ist ungültig (Chunk-Größe)",
        "The server's answer is invalid (chunk size)",
    ),
    (
        "Basic-Anmeldung wird nur über HTTPS angeboten",
        "Basic authentication is only offered over HTTPS",
    ),
    (
        "Anmeldung abgelehnt (HTTP 401). Prüfe Benutzer, Passwort und Domäne; angeboten: {}",
        "Login refused (HTTP 401). Check user, password and domain; offered: {}",
    ),
    (
        "Unerwartete Antwort des Servers (HTTP {})",
        "Unexpected answer from the server (HTTP {})",
    ),
    (
        "Der Server bietet kein NTLM an (angeboten: {})",
        "The server does not offer NTLM (offered: {})",
    ),
    (
        "Der Server beendet die Verbindung während der Anmeldung",
        "The server closes the connection during login",
    ),
    (
        "Auf diesem Port läuft kein WinRM (HTTP 404)",
        "No WinRM on this port (HTTP 404)",
    ),
    (
        "Unerwartete Antwort des Servers (HTTP {}): {}",
        "Unexpected answer from the server (HTTP {}): {}",
    ),
    (
        "Kein /proc auf diesem Host (nur Linux)",
        "No /proc on this host (Linux only)",
    ),
    (
        "Der Host liefert keine Messwerte (keine POSIX-Shell?)",
        "The host returns no readings (no POSIX shell?)",
    ),
    ("Verbindung beendet", "Connection closed"),
    ("eingehend", "inbound"),
    ("ausgehend", "outbound"),
    (
        "Der Server hat keine Windows-Fakten geliefert",
        "The server returned no Windows facts",
    ),
    ("nicht erreichbar", "unreachable"),
    ("übersprungen", "skipped"),
    (
        "Kein Befehlskanal (SSH oder WinRM) eingerichtet",
        "No command channel (SSH or WinRM) set up",
    ),
    (
        "Zeitüberschreitung beim Verbinden",
        "Timeout while connecting",
    ),
    ("Befehl nach {} s abgebrochen", "Command stopped after {} s"),
    (
        "Kein Exit-Code (Verbindung beendet)",
        "No exit code (connection closed)",
    ),
    (
        "Der Befehl ist zu lang für WinRM (höchstens etwa 10 000 Zeichen)",
        "The command is too long for WinRM (about 10,000 characters at most)",
    ),
    (
        "Die Antwort des Servers enthält keine Shell-ID",
        "The server's answer contains no shell ID",
    ),
    (
        "Die Antwort des Servers enthält keine Befehls-ID",
        "The server's answer contains no command ID",
    ),
    ("Zertifikat nicht bestätigt", "Certificate not confirmed"),
    (
        "Das Zertifikat konnte nicht bestätigt werden",
        "The certificate could not be confirmed",
    ),
    ("Mehrfachlauf", "Multi-host run"),
    ("Gestartet", "Started"),
    ("Ergebnis", "Result"),
    ("Hinweis", "Note"),
    (
        "Aurora – Befehl auf mehreren Hosts",
        "Aurora – command on many hosts",
    ),
    ("Ergebnisse", "Results"),
    (
        "Der Wert von „{}“ darf keinen Zeilenumbruch enthalten",
        "The value of “{}” must not contain a line break",
    ),
    (
        "fährt den Server herunter oder startet ihn neu",
        "shuts the server down or restarts it",
    ),
    (
        "formatiert oder löscht Datenträger",
        "formats or wipes disks",
    ),
    ("entfernt Objekte", "removes objects"),
    ("Snippet", "Snippet"),
    ("Noch kein Mehrfachlauf", "No multi-host run yet"),
    (
        "PowerShell-Befehl, aber der Host ist nicht als Windows bekannt (Fakten aktualisieren)",
        "PowerShell command, but the host is not known as Windows (update the facts)",
    ),
    (
        "Windows-Host: den Befehl als PowerShell ausführen",
        "Windows host: run the command as PowerShell",
    ),
    (
        "Kein Befehlskanal: SSH oder WinRM im Host einrichten",
        "No command channel: set up SSH or WinRM in the host",
    ),
    (
        "WinRM führt PowerShell aus – die Shell auf PowerShell stellen",
        "WinRM runs PowerShell – set the shell to PowerShell",
    ),
    (
        "Kein Befehlskanal (SSH oder WinRM)",
        "No command channel (SSH or WinRM)",
    ),
    (
        "im Wartungsfenster ({}) – übersprungen",
        "in a maintenance window ({}) – skipped",
    ),
    (
        "Mehrfachlauf fertig: {} ok, {} Probleme, {} übersprungen",
        "Multi-host run finished: {} ok, {} problems, {} skipped",
    ),
    (
        "Ergebnis: Befehlspalette → „Letzten Mehrfachlauf anzeigen“",
        "Result: command palette → “Show last multi-host run”",
    ),
    ("Auf mehreren Hosts ausführen", "Run on many hosts"),
    ("Hosts · {}", "Hosts · {}"),
    ("{} von {} gewählt", "{} of {} selected"),
    (
        "Hosts suchen: env:prod tag:web type:ssh …",
        "Find hosts: env:prod tag:web type:ssh …",
    ),
    ("Treffer übernehmen", "Use the matches"),
    (
        "Ersetzt die Liste durch die Hosts, die die Suche findet",
        "Replaces the list with the hosts the search finds",
    ),
    (
        "Keine Hosts – suche oben und übernimm die Treffer.",
        "No hosts – search above and use the matches.",
    ),
    ("Eigener Befehl", "Own command"),
    ("Snippet wählen…", "Pick a snippet…"),
    (
        "z. B. uptime  ·  systemctl status {{dienst}}",
        "e.g. uptime  ·  systemctl status {{service}}",
    ),
    ("Shell:", "Shell:"),
    ("Platzhalter", "Placeholders"),
    ("Mit sudo (ohne Passwort)", "With sudo (no password)"),
    ("Parallel", "In parallel"),
    ("Hosts gleichzeitig", "hosts at a time"),
    ("Zeitlimit", "Time limit"),
    ("je Host", "per host"),
    ("Weiter…", "Next…"),
    (
        "Nicht alle Platzhalter sind ausgefüllt",
        "Not all placeholders are filled in",
    ),
    ("Wähle ein Snippet", "Pick a snippet"),
    (
        "Befehl auf {} Hosts ausführen?",
        "Run the command on {} hosts?",
    ),
    (
        "{} parallel, {} s Zeitlimit",
        "{} in parallel, {} s time limit",
    ),
    ("{} Hosts werden übersprungen", "{} hosts are skipped"),
    (
        "Auch {} Hosts im Wartungsfenster einbeziehen (sonst übersprungen)",
        "Include {} hosts in a maintenance window too (otherwise skipped)",
    ),
    ("Gefährlicher Befehl: {}", "Dangerous command: {}"),
    ("{} PROD-Hosts dabei: {}{}", "{} PROD hosts included: {}{}"),
    (
        "Gefährlicher Befehl auf PROD: tippe {} zur Bestätigung.",
        "Dangerous command on PROD: type {} to confirm.",
    ),
    (
        "Ja, auch auf den PROD-Hosts ausführen",
        "Yes, run on the PROD hosts too",
    ),
    ("Auf {} Hosts ausführen", "Run on {} hosts"),
    ("Alle {}", "All {}"),
    ("ok {}", "ok {}"),
    ("Probleme {}", "Problems {}"),
    ("übersprungen {}", "skipped {}"),
    ("läuft {}", "running {}"),
    (
        "Ergebnisse durchsuchen (Host, Status, Ausgabe)",
        "Search results (host, status, output)",
    ),
    (
        "Keine Ergebnisse für diesen Filter",
        "No results for this filter",
    ),
    ("HTML zum Drucken (.html)", "Printable HTML (.html)"),
    ("Als Markdown kopieren", "Copy as Markdown"),
    (
        "Fehlgeschlagene wiederholen ({})",
        "Run the failed ones again ({})",
    ),
    (
        "WinRM ist für diesen Host nicht eingerichtet",
        "WinRM is not set up for this host",
    ),
    (
        "Kein Passwort gespeichert – im Host eine Identität mit Passwort hinterlegen",
        "No password stored – give the host an identity with a password",
    ),
    (
        "Kein Benutzername für diesen Host",
        "No user name for this host",
    ),
    (
        "Zertifikat noch nicht bestätigt – einmal „Fakten per WinRM aktualisieren“ ausführen",
        "Certificate not confirmed yet – run “Update facts via WinRM” once",
    ),
    (
        "Fakten von {} per WinRM aktualisiert",
        "Facts of {} updated via WinRM",
    ),
    ("WinRM {}: {}", "WinRM {}: {}"),
    (
        "Fakten und Befehle per WinRM",
        "Facts and commands via WinRM",
    ),
    ("0 = Standard (5986)", "0 = default (5986)"),
    ("0 = Standard (5985)", "0 = default (5985)"),
    (
        "Benutzer und Passwort kommen aus der Identität des Hosts; das Zertifikat wird beim ersten Mal bestätigt und festgehalten.",
        "User and password come from the host's identity; the certificate is confirmed and pinned on first use.",
    ),
    (
        "Ohne HTTPS ist der Verkehr unverschlüsselt; Windows verlangt dafür „AllowUnencrypted“. HTTPS (5986) wird empfohlen.",
        "Without HTTPS the traffic is unencrypted; Windows requires “AllowUnencrypted” for that. HTTPS (5986) is recommended.",
    ),
    ("Auf mehreren Hosts ausführen…", "Run on many hosts…"),
    ("Nur für SSH-Hosts", "SSH hosts only"),
    ("sammle Messwerte…", "collecting readings…"),
    ("vor {} s", "{} s ago"),
    ("Live-Statistik: {}", "Live statistics: {}"),
    ("verbinde…", "connecting…"),
    ("Nicht unterstützt", "Not supported"),
    (
        "Die Live-Statistik liest /proc und braucht einen Linux-Host mit POSIX-Shell (BSD und Windows-OpenSSH liefern keine Werte).",
        "Live statistics read /proc and need a Linux host with a POSIX shell (BSD and Windows OpenSSH give no values).",
    ),
    ("Kein Zugang im Hintergrund", "No background access"),
    (
        "Verbindung unterbrochen – neuer Versuch folgt",
        "Connection lost – retrying",
    ),
    (
        "Messung alle {} s über eine SSH-Verbindung, die schließt, wenn du das Fenster schließt",
        "Sampled every {} s over one SSH connection that closes with this window",
    ),
    (
        "Mini-Graphen in der Hostliste",
        "Mini graphs in the host list",
    ),
    (
        "CPU und RAM der sichtbaren SSH-Hosts live zeigen (pro Host eine Hintergrundverbindung)",
        "Show live CPU and RAM of the visible SSH hosts (one background connection per host)",
    ),
    (
        "Befehl auf allen Hosts ausführen…",
        "Run a command on all hosts…",
    ),
    (
        "Befehl oder Snippet parallel auf der Gruppe ausführen",
        "Run a command or snippet on the group in parallel",
    ),
    ("Befehl ausführen…", "Run command…"),
    (
        "Einen Befehl oder ein Snippet auf allen gewählten Hosts ausführen",
        "Run a command or snippet on all selected hosts",
    ),
    ("Auswahl ({} Hosts)", "Selection ({} hosts)"),
    (
        "Befehl oder Snippet auf diesem Host (und weiteren) ausführen",
        "Run a command or snippet on this host (and others)",
    ),
    ("Live-Statistik…", "Live statistics…"),
    (
        "CPU, RAM, Last, Netzwerk und Datenträger live",
        "CPU, RAM, load, network and disks live",
    ),
    ("Fakten per WinRM aktualisieren", "Update facts via WinRM"),
    (
        "Befehl auf mehreren Hosts ausführen…",
        "Run a command on many hosts…",
    ),
    ("Letzten Mehrfachlauf anzeigen", "Show last multi-host run"),
];
