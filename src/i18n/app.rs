//! English texts for the German source strings (see `super::tr`).

pub const T: &[(&str, &str)] = &[
    // app.rs
    ("Läuft in {}", "Running in {}"),
    ("Speichern fehlgeschlagen: {}", "Saving failed: {}"),
    (
        "Keychain-Speichern fehlgeschlagen: {}",
        "Saving the keychain failed: {}",
    ),
    ("Lokal", "Local"),
    ("Verbinde…", "Connecting…"),
    ("Schlüssel nicht lesbar: {}", "Key not readable: {}"),
    (
        "{}: Anmeldung jetzt per SSH-Key",
        "{}: now logging in with SSH key",
    ),
    (
        "1 Host hinzugefügt – der Name kommt beim ersten Login",
        "1 host added – its name arrives with the first login",
    ),
    (
        "{} Hosts hinzugefügt – Namen kommen beim ersten Login",
        "{} hosts added – names arrive with the first login",
    ),
    (
        "{} Sessions der letzten Sitzung wiederhergestellt",
        "{} sessions from the last run restored",
    ),
    (
        "Export-Passwort: mindestens 8 Zeichen",
        "Export password: at least 8 characters",
    ),
    (
        "Exportiert nach {} – verschlüsselt",
        "Exported to {} – encrypted",
    ),
    ("Export fehlgeschlagen: {}", "Export failed: {}"),
    ("Import fehlgeschlagen: {}", "Import failed: {}"),
    ("{} Einträge importiert", "{} entries imported"),
    ("hier ablegen", "drop here"),
    (
        "Keine verbundene Terminal-Session aktiv",
        "No connected terminal session active",
    ),
    (
        "Snippet an {} Sessions gesendet",
        "Snippet sent to {} sessions",
    ),
    ("Aurora {} ist verfügbar", "Aurora {} is available"),
    ("Aurora ist aktuell.", "Aurora is up to date."),
    (
        "Installiert – Neustart aktiviert die neue Version.",
        "Installed – restart to activate the new version.",
    ),
    ("Hochgeladen nach {}", "Uploaded to {}"),
    ("Upload fehlgeschlagen: {}", "Upload failed: {}"),
    ("Verbindung beendet.", "Connection closed."),
    (
        "{} getrennt – Klick auf die Kachel verbindet neu",
        "{} disconnected – click the tile to reconnect",
    ),
    ("{}: Befehl fertig nach {}", "{}: command finished after {}"),
    ("RDP-Sitzung beendet", "RDP session ended"),
    ("RDP-Client beendet ({})", "RDP client exited ({})"),
    ("{} heißt jetzt „{}“", "{} is now called \"{}\""),
    (
        "Tunnel „{}“ offen: localhost:{}",
        "Tunnel \"{}\" open: localhost:{}",
    ),
    ("{} (Kopie)", "{} (copy)"),
    (
        "Die Gruppe enthält keine Hosts",
        "The group contains no hosts",
    ),
    (
        "Keine neuen Hosts in ~/.ssh/config",
        "No new hosts in ~/.ssh/config",
    ),
    ("1 Host importiert", "1 host imported"),
    ("{} Hosts importiert", "{} hosts imported"),
    ("{} Hosts in „LAN {}“", "{} hosts in \"LAN {}\""),
    ("Snippets ein-/ausblenden", "Show/hide snippets"),
    (
        "Im Terminal suchen (Ctrl+Shift+F)",
        "Search in terminal (Ctrl+Shift+F)",
    ),
    ("Notizen zum Host", "Host notes"),
    ("Als Host speichern", "Save as host"),
    (
        "Diese Schnellverbindung in die Hostliste übernehmen",
        "Add this quick connection to the host list",
    ),
    ("Neu verbinden (Enter)", "Reconnect (Enter)"),
    ("Ausblenden", "Hide"),
    ("Filtern", "Filter"),
    ("Noch keine Snippets", "No snippets yet"),
    ("Klicken zum Ausführen", "Click to run"),
    (
        "Hochladen geht nur bei SSH-Sessions",
        "Uploading only works in SSH sessions",
    ),
    ("Nicht verbunden", "Not connected"),
    ("Loslassen zum Hochladen nach {}", "Release to upload to {}"),
    ("Beenden", "Stop"),
    ("Erneut starten", "Start again"),
    ("Läuft im System-Client", "Running in the system client"),
    ("Fenster geschlossen", "Window closed"),
    (
        "Automatisch gesperrt – Sessions laufen weiter",
        "Locked automatically – sessions keep running",
    ),
    ("Upload startet…", "Upload starting…"),
    (
        "Getrennt · Enter verbindet neu",
        "Disconnected · Enter reconnects",
    ),
    // terminal.rs
    ("Im Terminal suchen", "Search in terminal"),
    ("Älterer Treffer (Enter)", "Older match (Enter)"),
    ("Neuerer Treffer (Shift+Enter)", "Newer match (Shift+Enter)"),
    ("Schließen (Esc)", "Close (Esc)"),
    ("📋 Kopieren", "📋 Copy"),
    ("⚡ Als Snippet speichern", "⚡ Save as snippet"),
    ("🔍 Suchen  (Ctrl+Shift+F)", "🔍 Search  (Ctrl+Shift+F)"),
    ("📥 Alles kopieren", "📥 Copy all"),
    ("🗑 Bildschirm leeren", "🗑 Clear screen"),
    ("⬆ {} Zeilen zurück", "⬆ {} lines back"),
];
