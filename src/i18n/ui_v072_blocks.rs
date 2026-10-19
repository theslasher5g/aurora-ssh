//! English texts: command blocks, shell integration, session history and comments.

pub const T: &[(&str, &str)] = &[
    // app.rs
    (
        "Marker und Notizen zur Sitzung (Ctrl+Shift+M)",
        "Markers and notes for the session (Ctrl+Shift+M)",
    ),
    (
        "Marker in einer Aufnahme: erst die Zeitreise öffnen und eine Stelle wählen",
        "Markers in a recording: open time travel first and pick a position",
    ),
    // terminal.rs: command blocks
    ("Befehl", "Command"),
    ("Beides", "Both"),
    ("Erneut", "Rerun"),
    ("Ausklappen", "Expand"),
    ("Einklappen", "Collapse"),
    ("Befehl kopieren", "Copy command"),
    ("Ausgabe kopieren", "Copy output"),
    ("Befehl und Ausgabe kopieren", "Copy command and output"),
    (
        "Befehl am Prompt eintippen (PROD: nur eintippen, Enter bleibt bei dir)",
        "Type the command at the prompt (PROD: only types it, Enter stays with you)",
    ),
    (
        "Ausgabe ein- oder ausklappen",
        "Collapse or expand the output",
    ),
    ("Dauer {}", "Duration {}"),
    (
        "Dieser Befehl lässt sich nicht erneut eintippen",
        "This command cannot be typed again",
    ),
    ("Befehl kopiert", "Command copied"),
    ("Ausgabe kopiert", "Output copied"),
    ("Befehl und Ausgabe kopiert", "Command and output copied"),
    ("Nichts zu kopieren", "Nothing to copy"),
    (
        "Letzte Ausgabe kopieren  (Ctrl+Shift+O)",
        "Copy last output  (Ctrl+Shift+O)",
    ),
    ("Zum letzten Fehler springen", "Jump to the last error"),
    (
        "Kein fehlgeschlagener Befehl bekannt",
        "No failed command known",
    ),
    (
        "Ausgabe des letzten Befehls kopiert",
        "Output of the last command copied",
    ),
    ("Keine Ausgabe zum Kopieren", "No output to copy"),
    ("+ {} Zeilen eingeklappt", "+ {} lines collapsed"),
    // blocks_setup.rs / editors.rs
    ("Shell-Integration", "Shell integration"),
    ("Befehlsblöcke", "Command blocks"),
    (
        "Markiert Befehl und Ausgabe jedes Befehls am linken Rand: Kopieren, Wiederholen, Einklappen, Ctrl+↑/↓ springt zwischen Befehlen",
        "Marks command and output of every command at the left edge: copy, rerun, collapse; Ctrl+↑/↓ jumps between commands",
    ),
    (
        "Mit diesen Zeilen meldet die Shell Prompt, Befehl, Ausgabe und Exit-Code (OSC 133) sowie den Arbeitsordner (OSC 7). Ohne sie rät Aurora die Blöcke am Prompt. Aurora ändert keine Konfigurationsdateien: Einfügen in die Datei der Shell oder pro Host „Shell-Integration beim Start automatisch einrichten“ einschalten.",
        "With these lines the shell reports prompt, command, output and exit code (OSC 133) and the working directory (OSC 7). Without them Aurora guesses the blocks from the prompt. Aurora never changes configuration files: paste them into the shell's file or turn on \"Set up shell integration automatically at start\" per host.",
    ),
    (
        "Shell-Integration beim Start automatisch einrichten",
        "Set up shell integration automatically at start",
    ),
    (
        "Schickt nach dem Login eine kurze Zeile an bash, zsh oder fish, damit Befehlsblöcke Exit-Codes und Arbeitsordner kennen. Ändert keine Konfigurationsdateien und gilt nur für diese Sitzung.",
        "After login sends one short line to bash, zsh or fish so command blocks know exit codes and the working directory. Changes no configuration files and only applies to this session.",
    ),
    // sessionlog_ui.rs
    ("Marker", "Marker"),
    ("Exportiert: {}", "Exported: {}"),
    ("Export läuft…", "Exporting…"),
    (
        "Marker setzen  (Ctrl+Shift+M)",
        "Set marker  (Ctrl+Shift+M)",
    ),
    (
        "In der Aufnahme: erst die Zeitreise öffnen und eine Stelle wählen",
        "In a recording: open time travel first and pick a position",
    ),
    (
        "Noch keine Marker. Mit einem Marker merkst du dir Stellen der Sitzung samt Notiz.",
        "No markers yet. A marker bookmarks a spot in the session together with a note.",
    ),
    ("Zu dieser Stelle springen", "Jump to this spot"),
    ("Marker löschen", "Delete marker"),
    ("Als Text exportieren", "Export as text"),
    ("Als Markdown exportieren", "Export as Markdown"),
    ("Aufnahmen durchsuchen", "Search recordings"),
    (
        "Text in allen Aufnahmen und Markern suchen",
        "Search text in all recordings and markers",
    ),
    (
        "Index wird aktualisiert… {} von {}",
        "Updating index… {} of {}",
    ),
    (
        "{} Aufnahmen, {} Zeilen im Index",
        "{} recordings, {} lines in the index",
    ),
    (
        "Aufnahme an dieser Stelle öffnen",
        "Open the recording at this spot",
    ),
    ("Verlauf der Sitzungen", "Session history"),
    (
        "Sitzungen mit Dauer, Aufnahme und Markern",
        "Sessions with duration, recording and markers",
    ),
    ("Noch keine Sitzungen", "No sessions yet"),
    ("aufgenommen", "recorded"),
    ("nicht aufgenommen", "not recorded"),
    ("{} Marker", "{} markers"),
    ("Export", "Export"),
    ("Text", "Text"),
    // sessionlog_export.rs
    ("Datum", "Date"),
    ("Ungültiger Dateiname", "Invalid file name"),
    // hosts.rs
    ("Verlauf…", "History…"),
];
