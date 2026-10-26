//! English texts for the German source strings (see `super::tr`): RDP drive redirection.

pub const T: &[(&str, &str)] = &[
    ("Split", "Split"),
    ("Kein Ordner angegeben", "No folder given"),
    ("Ordner nicht gefunden", "Folder not found"),
    ("Das ist kein Ordner", "That is not a folder"),
    (
        "Ein ganzes Laufwerk kann nicht freigegeben werden",
        "A whole drive cannot be shared",
    ),
    ("lesen und schreiben", "read and write"),
    ("nur lesen", "read only"),
    ("Laufwerk: {} ({})", "Drive: {} ({})"),
    (
        "Laufwerk: {} Zugriffe wurden blockiert (außerhalb des Ordners oder nur lesen)",
        "Drive: {} accesses were blocked (outside the folder, or read only)",
    ),
    ("Laufwerk: {}", "Drive: {}"),
    ("Laufwerk: nicht freigegeben", "Drive: not shared"),
    ("{} gelesen · {} geschrieben", "{} read · {} written"),
    ("{} Zugriffe blockiert", "{} accesses blocked"),
    ("Freigabe trennen", "Disconnect share"),
    ("Ordner wieder freigeben", "Share folder again"),
    ("Laufwerk freigeben", "Share a drive"),
    ("Ordner", "Folder"),
    (
        "Pfad eintippen oder Ordner hierher ziehen",
        "Type a path or drag a folder here",
    ),
    ("Ordnername", "Folder name"),
    ("Schreiben erlauben", "Allow writing"),
    (
        "Ohne Ordner wird kein Laufwerk freigegeben",
        "Without a folder no drive is shared",
    ),
    (
        "Die Sitzung darf in „{}“ Dateien anlegen, ändern und löschen – nur dort",
        "The session may create, change and delete files in “{}” – only there",
    ),
    (
        "Die Sitzung darf „{}“ nur lesen; Änderungen sind nicht möglich",
        "The session may only read “{}”; changes are not possible",
    ),
    (
        "Gilt ab der nächsten Verbindung. Der Ordner erscheint im Explorer des Hosts; Verknüpfungen aus dem Ordner heraus werden nicht gefolgt, nichts wird ausgeführt.",
        "Applies from the next connection. The folder appears in the host's Explorer; links pointing out of the folder are not followed and nothing is executed.",
    ),
];
