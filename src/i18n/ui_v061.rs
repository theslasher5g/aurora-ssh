//! English texts for the German source strings (see `super::tr`): 0.6.1 – log panel in
//! sessions, own log commands, session recording, persistent sessions.

pub const T: &[(&str, &str)] = &[
    ("Abspielen", "Play"),
    ("Alle Hosts", "All hosts"),
    ("Alle SSH-Sessions aufzeichnen", "Record all SSH sessions"),
    (
        "Als eigenen Log-Befehl für diesen Host speichern",
        "Save as an own log command for this host",
    ),
    ("Andere Log-Quelle wählen", "Choose another log source"),
    ("Aufnahme · {}", "Recording · {}"),
    ("Aufnahme beenden", "Stop recording"),
    (
        "Aufnahme zu Ende · Enter spielt erneut ab",
        "Recording finished · Enter plays it again",
    ),
    ("Ende der Aufnahme.", "End of recording."),
    ("Aufnahme fehlgeschlagen: {}", "Recording failed: {}"),
    ("Aufnahme gespeichert: {}", "Recording saved: {}"),
    ("Aufnahmen", "Recordings"),
    ("Auth", "Auth"),
    (
        "Datei (wird mit tail -F verfolgt) oder ein Befehl, der laufend ausgibt – läuft auf {}",
        "A file (followed with tail -F) or a command that keeps printing – runs on {}",
    ),
    (
        "Dateien im aktuellen Ordner des Terminals öffnen",
        "Open files in the terminal's current folder",
    ),
    ("Dauerhafte Session (tmux)", "Persistent session (tmux)"),
    (
        "Die Shell läuft in tmux (sonst screen) auf dem Server weiter: nach einem Abbruch landet das Neu-Verbinden in derselben Session mit allen laufenden Programmen.",
        "The shell keeps running in tmux (or screen) on the server: after a drop, reconnecting lands in the same session with all running programs.",
    ),
    ("Eigene Befehle", "Own commands"),
    ("Eigene Log-Befehle", "Own log commands"),
    ("Journal", "Journal"),
    ("Journal (Warnungen)", "Journal (warnings)"),
    (
        "Keine asciinema-v2-Aufnahme",
        "Not an asciinema v2 recording",
    ),
    ("Klicken zum Starten", "Click to start"),
    ("Leere Datei", "Empty file"),
    ("Leertaste pausiert", "Space pauses"),
    ("Log-Befehl gespeichert", "Log command saved"),
    ("Log-Leiste schließen", "Close the log panel"),
    ("Logs", "Logs"),
    ("Logs neben dem Terminal", "Logs next to the terminal"),
    (
        "Logs rechts neben dem Terminal (Ctrl+Shift+L)",
        "Logs to the right of the terminal (Ctrl+Shift+L)",
    ),
    ("Neuer Log-Befehl", "New log command"),
    ("Noch keine Aufnahmen", "No recordings yet"),
    ("Ordner", "Folder"),
    ("Ordner öffnen", "Open folder"),
    (
        "Ordner unbekannt – die Shell meldet ihn nicht, öffne den Home-Ordner",
        "Folder unknown – the shell doesn't report it, opening the home folder",
    ),
    ("Quelle", "Source"),
    (
        "Session aufzeichnen (asciinema)",
        "Record session (asciinema)",
    ),
    (
        "Sonst über den Aufnahme-Knopf in der Session. Aufgezeichnet wird, was das Terminal zeigt (asciinema-Format).",
        "Otherwise with the record button in the session. What the terminal shows is recorded (asciinema format).",
    ),
    ("Starten", "Start"),
    ("Vorlagen", "Presets"),
];
