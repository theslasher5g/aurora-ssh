//! English texts for the German source strings (see `super::tr`): file transfer through
//! the RDP clipboard and PowerShell snippets on RDP hosts.

pub const T: &[(&str, &str)] = &[
    (
        "Abgelegte Dateien erneut einfügen (Strg+V auf dem Host)",
        "Paste the dropped files again (Ctrl+V on the host)",
    ),
    (
        "Datei konnte nicht gelesen werden: {}",
        "Could not read file: {}",
    ),
    (
        "Datei konnte nicht gespeichert werden: {}",
        "Could not save file: {}",
    ),
    (
        "Datei zu groß für diesen Host (über 2 GB)",
        "File too large for this host (over 2 GB)",
    ),
    ("Dateien einfügen", "Paste files"),
    ("Dateien vom Host speichern", "Save files from host"),
    (
        "Dateiliste des Hosts konnte nicht gelesen werden",
        "Could not read the host's file list",
    ),
    (
        "Der Host erlaubt keine Dateiübertragung über die Zwischenablage",
        "The host does not allow file transfer through the clipboard",
    ),
    (
        "Der Host hat die Dateiliste abgelehnt",
        "The host rejected the file list",
    ),
    (
        "Die Zwischenablage des Hosts enthält keine Dateien",
        "The host's clipboard holds no files",
    ),
    (
        "Es läuft bereits eine Übertragung",
        "A transfer is already running",
    ),
    (
        "Große Dateien gehen per Dateibrowser (SFTP) schneller",
        "Large files are faster through the file browser (SFTP)",
    ),
    (
        "Keine PowerShell-Snippets – im Snippet-Editor „PowerShell“ wählen",
        "No PowerShell snippets – choose “PowerShell” in the snippet editor",
    ),
    (
        "Keine übertragbaren Dateien",
        "No files that can be transferred",
    ),
    (
        "Kopierte Dateien des Hosts in Downloads/Aurora speichern",
        "Save the files copied on the host to Downloads/Aurora",
    ),
    ("Loslassen zum Kopieren auf {}", "Release to copy to {}"),
    ("PowerShell-Snippet ausführen", "Run PowerShell snippet"),
    (
        "PowerShell-Snippets lassen sich auch in RDP-Sessions auf Windows-Hosts starten",
        "PowerShell snippets can also be started in RDP sessions on Windows hosts",
    ),
    (
        "Snippet ist zu lang zum Eintippen",
        "Snippet is too long to type",
    ),
    (
        "Zwischenablage ist für diesen Host ausgeschaltet",
        "Clipboard is turned off for this host",
    ),
    ("{} Dateien", "{} files"),
    ("{} Dateien gespeichert in {}", "{} files saved to {}"),
    (
        "{} übersprungen (Links nach außen oder ungültige Namen)",
        "{} skipped (links leaving the folder or invalid names)",
    ),
    (
        "Übertragung vom Host abgebrochen",
        "Transfer from the host was cancelled",
    ),
];
