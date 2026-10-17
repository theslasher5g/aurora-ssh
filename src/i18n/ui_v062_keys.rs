//! English texts for the German source strings (see `super::tr`): 0.6.2 – groups and
//! search on the keys page.

pub const T: &[(&str, &str)] = &[
    ("+ Schlüssel", "+ Key"),
    ("Gruppe „{}“ löschen?", "Delete group “{}”?"),
    (
        "Gruppe „{}“ löschen? Ihre {} Schlüssel landen in „Ohne Gruppe“.",
        "Delete group “{}”? Its {} keys move to “No group”.",
    ),
    (
        "Gruppe „{}“ löschen? Der Schlüssel landet in „Ohne Gruppe“.",
        "Delete group “{}”? Its key moves to “No group”.",
    ),
    ("In Gruppe verschieben", "Move to group"),
    (
        "Kein Schlüssel passt zu „{}“. Gesucht wird in Name, Benutzer, Typ, Kommentar, Fingerprint und Gruppe.",
        "No key matches “{}”. The search looks at name, user, type, comment, fingerprint and group.",
    ),
    ("Suchen: Name, Typ, Gruppe …", "Search: name, type, group …"),
    (
        "Neuen Schlüssel in dieser Gruppe anlegen",
        "Create a new key in this group",
    ),
    (
        "Noch leer – Schlüssel hierher ziehen oder über „In Gruppe verschieben“ zuordnen",
        "Empty – drag keys here or use “Move to group”",
    ),
    ("Schlüssel hinzufügen", "Add key"),
    ("Umbenennen…", "Rename…"),
    ("z.B. Produktion", "e.g. Production"),
    ("{} von {}", "{} of {}"),
];
