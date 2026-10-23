//! English texts for the German source strings (see `super::tr`): web interfaces
//! embedded in the tab.

pub const T: &[(&str, &str)] = &[
    ("Weboberflächen", "Web interfaces"),
    ("Anzeige", "Display"),
    (
        "eingebettet im Tab oder in einem eigenen Fenster",
        "embedded in the tab or in its own window",
    ),
    (
        "Einbetten ist auf diesem System nicht möglich (Wayland/macOS): eigenes Fenster",
        "Embedding is not possible on this system (Wayland/macOS): own window",
    ),
    ("Im Tab (eingebettet)", "In the tab (embedded)"),
    ("Eigenes Fenster", "Own window"),
    ("Seite anzeigen", "Show page"),
    ("Details", "Details"),
    ("In eigenem Fenster öffnen", "Open in own window"),
    (
        "Öffnet die Seite neu in einem eigenen Fenster",
        "Reopens the page in its own window",
    ),
    ("Im Tab einbetten", "Embed in the tab"),
    (
        "Öffnet die Seite neu eingebettet in diesem Tab",
        "Reopens the page embedded in this tab",
    ),
    (
        "Die Seite ist in das Hauptfenster eingebettet und hier nicht sichtbar. Öffne sie in einem eigenen Fenster oder hole den Tab zurück.",
        "The page is embedded in the main window and not visible here. Open it in its own window or bring the tab back.",
    ),
    ("Seite im Tab geöffnet", "Page open in the tab"),
    (
        "Die Weboberfläche läuft eingebettet in diesem Tab mit eigenem Profil. Schließt du den Tab, endet auch die Seite.",
        "The web interface runs embedded in this tab with its own profile. Closing the tab ends the page.",
    ),
];
