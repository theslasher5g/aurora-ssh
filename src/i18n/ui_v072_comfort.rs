//! English texts: reconnect, what is new, custom themes.

pub const T: &[(&str, &str)] = &[
    // theme import (parser)
    (
        "Die Datei ist zu groß (höchstens 256 KB).",
        "The file is too large (256 KB at most).",
    ),
    (
        "Nichts zum Importieren – die Eingabe ist leer.",
        "Nothing to import – the input is empty.",
    ),
    ("Importiertes Theme", "Imported theme"),
    (
        "Keine Hintergrundfarbe gefunden.",
        "No background colour found.",
    ),
    (
        "Keine Vordergrundfarbe gefunden.",
        "No foreground colour found.",
    ),
    (
        "Nur {} von 16 Terminalfarben gefunden – das sieht nicht nach einem Farbschema aus.",
        "Only {} of 16 terminal colours found – this does not look like a colour scheme.",
    ),
    (
        "In der Datei stehen keine iTerm-Farben.",
        "The file contains no iTerm colours.",
    ),
    (
        "Das ist kein gültiges JSON: {}",
        "This is not valid JSON: {}",
    ),
    ("Unbekanntes JSON-Format.", "Unknown JSON format."),
    (
        "Die Datei enthält keine Farbschemas.",
        "The file contains no colour schemes.",
    ),
    (
        "base0{} fehlt im Schema.",
        "base0{} is missing in the scheme.",
    ),
    (
        "Das Format wurde nicht erkannt (iTerm, Windows Terminal, base16, Alacritty, Kitty/Ghostty).",
        "Format not recognised (iTerm, Windows Terminal, base16, Alacritty, Kitty/Ghostty).",
    ),
    // theme import (UI)
    ("Das ist keine Datei.", "This is not a file."),
    ("Design · Eigene", "Theme · Custom"),
    ("Eigene Themes", "Custom themes"),
    (
        "Farbschemas aus iTerm2 (.itermcolors), Windows Terminal (JSON), base16 (YAML/JSON), Alacritty (TOML) und Kitty/Ghostty importieren. Datei hierher ziehen, Pfad eingeben oder den Text einfügen.",
        "Import colour schemes from iTerm2 (.itermcolors), Windows Terminal (JSON), base16 (YAML/JSON), Alacritty (TOML) and Kitty/Ghostty. Drop a file here, type a path or paste the text.",
    ),
    ("Datei laden", "Load file"),
    (
        "…oder das Farbschema hier einfügen",
        "…or paste the colour scheme here",
    ),
    ("Text einlesen", "Read text"),
    (
        "{} Farbschemas in der Datei",
        "{} colour schemes in the file",
    ),
    ("Alle {} speichern", "Save all {}"),
    ("Verwerfen", "Discard"),
    (
        "Theme gespeichert und aktiviert.",
        "Theme saved and activated.",
    ),
    ("Importierte Themes", "Imported themes"),
    ("OK", "OK"),
    ("JSON kopieren", "Copy JSON"),
    (
        "JSON in die Zwischenablage kopiert.",
        "JSON copied to the clipboard.",
    ),
    (
        "Zwischenablage nicht verfügbar.",
        "Clipboard not available.",
    ),
    ("Exportiert nach {}", "Exported to {}"),
    // reconnect
    (
        "{}: Verbindung verloren – verbinde automatisch neu",
        "{}: connection lost – reconnecting automatically",
    ),
    (
        "Verbindung zum Server verloren",
        "Connection to the server lost",
    ),
    (
        "Verbindung verloren – neuer Versuch in {} s (Versuch {})",
        "Connection lost – next attempt in {} s (attempt {})",
    ),
    (
        "Verbindung verloren – verbinde neu… (Versuch {})",
        "Connection lost – reconnecting… (attempt {})",
    ),
    (
        "─── Verbindung verloren ({}) ───",
        "─── Connection lost ({}) ───",
    ),
    ("Warte auf neuen Versuch", "Waiting for the next attempt"),
    (
        "Automatisches Wiederverbinden beendet – der Fehler lässt sich nicht durch Warten beheben.",
        "Automatic reconnect stopped – waiting will not fix this error.",
    ),
    ("✔ wieder verbunden nach {} s", "✔ reconnected after {} s"),
    (
        "Automatisches Wiederverbinden abgebrochen – Enter verbindet neu.",
        "Automatic reconnect cancelled – Enter reconnects.",
    ),
    (
        "Aufgegeben nach {} Min. – Enter verbindet neu.",
        "Gave up after {} min – Enter reconnects.",
    ),
    ("Jetzt verbinden", "Connect now"),
    ("Auto-Reconnect", "Auto-reconnect"),
    ("An", "On"),
    ("Aus", "Off"),
    ("Automatisch wiederverbinden", "Reconnect automatically"),
    (
        "SSH-Sessions, deren Verbindung abbricht (Netz weg, Standby), verbinden sich selbst wieder – mit Scrollback, tmux/screen und ohne neue Passwortabfrage",
        "SSH sessions whose connection drops (network gone, standby) reconnect by themselves – with scrollback, tmux/screen and without asking for the password again",
    ),
    ("Höchstens so lange versuchen", "Keep trying for at most"),
    (
        "Danach gibt Aurora auf; Enter verbindet dann von Hand",
        "After that Aurora gives up; Enter then reconnects by hand",
    ),
    ("Min.", "min"),
    // what's new
    ("Was ist neu", "What's new"),
    ("Neu in Aurora {}", "New in Aurora {}"),
    ("Verstanden", "Got it"),
    ("Auf GitHub ansehen", "View on GitHub"),
    ("Alle Änderungen", "All changes"),
];
