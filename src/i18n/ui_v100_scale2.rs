//! English texts: large inventories 2 (dynamic folders, manual group order, customer focus on
//! more pages, faster inventory page).

pub const T: &[(&str, &str)] = &[
    (
        "In dynamische Ordner lassen sich keine Hosts von Hand legen",
        "Hosts cannot be placed into dynamic folders by hand",
    ),
    ("Keine Quelle angegeben", "No source given"),
    (
        "Die Quelle enthält Steuerzeichen",
        "The source contains control characters",
    ),
    ("Die Adresse hat keinen Host", "The address has no host"),
    (
        "Zugangsdaten gehören nicht in die Adresse (user:passwort@host)",
        "Credentials do not belong in the address (user:password@host)",
    ),
    (
        "Zugangsdaten gehören nicht in die Adresse (Token, Passwort)",
        "Credentials do not belong in the address (token, password)",
    ),
    (
        "Nur https:// ist erlaubt (kein unverschlüsseltes http)",
        "Only https:// is allowed (no unencrypted http)",
    ),
    (
        "Unbekanntes Adress-Schema „{}“ – https:// oder ein Dateipfad",
        "Unknown address scheme “{}” – use https:// or a file path",
    ),
    (
        "Die Liste ist zu groß (höchstens 5 MB).",
        "The list is too big (5 MB at most).",
    ),
    (
        "Unsichere Weiterleitung abgelehnt",
        "Insecure redirect refused",
    ),
    ("Zu viele Weiterleitungen", "Too many redirects"),
    ("keinen Proxmox-Server", "no Proxmox server"),
    (
        "Im Fokus „{}“ gibt es hier {}.",
        "In the focus “{}” there is {} here.",
    ),
    ("{} von {} Hosts angezeigt", "{} of {} hosts shown"),
    ("Weitere anzeigen", "Show more"),
    (
        "Auch Gruppen ohne Kunde zeigen",
        "Also show groups without a customer",
    ),
    (
        "Schlüsselgruppen lassen sich im Gruppen-Editor einem Kunden zuordnen",
        "Key groups can be assigned to a customer in the group editor",
    ),
    ("Kunde", "Customer"),
    ("Wähle einen Proxmox-Server", "Choose a Proxmox server"),
    ("Die Suchregel ist leer", "The search rule is empty"),
    (
        "Der Proxmox-Server existiert nicht mehr",
        "The Proxmox server no longer exists",
    ),
    ("Wird geladen …", "Loading …"),
    ("Fehler: {}", "Error: {}"),
    ("Noch nicht abgeglichen", "Not synced yet"),
    ("Zuletzt {} · {} Treffer", "Last {} · {} matches"),
    (
        "Zuletzt {} · {} neu · {} entfernt · {} geändert · {} Einträge",
        "Last {} · {} new · {} removed · {} changed · {} entries",
    ),
    (" · {} übersprungen", " · {} skipped"),
    ("Dynamischer Ordner", "Dynamic folder"),
    ("Hosts aus einer Quelle", "Hosts from a source"),
    ("Suchregel", "Search rule"),
    ("Regel", "Rule"),
    ("Datei oder URL", "File or URL"),
    ("Beim Start abgleichen", "Sync at start"),
    (
        "Verschwundene Einträge löschen",
        "Delete entries that vanished",
    ),
    (
        "Alle VMs und Container des Servers erscheinen als Hosts in dieser Gruppe und werden beim Aktualisieren abgeglichen. Name, Adresse und Protokoll kommen aus Proxmox; Tags, Notizen und Zugangsdaten kannst du ergänzen. Beim Wechsel der Quelle werden die erzeugten Einträge ersetzt.",
        "All VMs and containers of the server appear as hosts in this group and are synced on refresh. Name, address and protocol come from Proxmox; you can add tags, notes and credentials. Changing the source replaces the generated entries.",
    ),
    (
        "Eine Smart-Gruppe: sie zeigt alle Hosts, auf die die Suche passt (z. B. type:switch customer/*), ohne sie zu kopieren.",
        "A smart folder: it lists all hosts the search matches (e.g. type:switch customer/*) without copying them.",
    ),
    (
        "CSV mit den Spalten name, host, port, protocol, user, group, tags, description (Kopfzeile optional). Nur https:// oder ein Dateipfad, höchstens 5 MB, keine Zugangsdaten in der Adresse. Beim Wechsel der Quelle werden die erzeugten Einträge ersetzt.",
        "CSV with the columns name, host, port, protocol, user, group, tags, description (header optional). Only https:// or a file path, 5 MB at most, no credentials in the address. Changing the source replaces the generated entries.",
    ),
    ("Jetzt abgleichen", "Sync now"),
    (
        "Erzeugt aus „{}“ – die Quelle führt diesen Eintrag nicht mehr. Tags, Notizen und Zugangsdaten bleiben bearbeitbar.",
        "Generated from “{}” – the source no longer lists this entry. Tags, notes and credentials stay editable.",
    ),
    (
        "Erzeugt aus „{}“: Name, Adresse, Protokoll und Gruppe kommen aus der Quelle. Tags, Notizen und Zugangsdaten kannst du ergänzen.",
        "Generated from “{}”: name, address, protocol and group come from the source. You can add tags, notes and credentials.",
    ),
    ("entfernt", "removed"),
    (
        "Dieser Eintrag ist in der Quelle nicht mehr vorhanden",
        "This entry is no longer in the source",
    ),
    (
        "Automatisch erzeugt – Name, Adresse und Protokoll kommen aus der Quelle; Tags, Notizen und Zugangsdaten kannst du ergänzen",
        "Generated automatically – name, address and protocol come from the source; you can add tags, notes and credentials",
    ),
    ("keine Container-Hosts", "no container hosts"),
    ("Reihenfolge", "Order"),
    ("Nach oben verschieben", "Move up"),
    ("Nach unten verschieben", "Move down"),
];
