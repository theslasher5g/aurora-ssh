//! English texts for the German source strings (see `super::tr`).

pub const T: &[(&str, &str)] = &[
    // shared
    ("Suchen", "Search"),
    ("Neu", "New"),
    ("Bearbeiten", "Edit"),
    ("Löschen", "Delete"),
    // ui/keychain.rs
    ("Schlüssel", "Keys"),
    ("Noch keine Schlüssel", "No keys yet"),
    (
        "Speichere Passwörter und SSH-Keys einmal und verwende sie für beliebig viele Hosts.",
        "Save passwords and SSH keys once and use them for as many hosts as you like.",
    ),
    ("Schlüssel anlegen", "Add key"),
    ("(ohne Namen)", "(unnamed)"),
    ("Public Key kopieren", "Copy public key"),
    ("Public Key kopiert", "Public key copied"),
    ("Key ungültig: {}", "Invalid key: {}"),
    ("Schlüssel „{}“ löschen?", "Delete key “{}”?"),
    ("nicht verwendet", "unused"),
    ("1 Host", "1 host"),
    ("{} Hosts", "{} hosts"),
    // ui/snippets.rs
    ("Noch keine Snippets", "No snippets yet"),
    (
        "Speichere Befehle, die du oft brauchst. Platzhalter wie {{dienst}} fragt Aurora beim Ausführen ab.",
        "Save commands you use often. Aurora asks for placeholders like {{service}} when running them.",
    ),
    ("Snippet anlegen", "Add snippet"),
    ("Kein Snippet gefunden", "No snippet found"),
    (
        "Leg eins an – oder markiere Text im Terminal und wähle „Als Snippet speichern“.",
        "Create one – or select text in the terminal and choose “Save as snippet”.",
    ),
    ("Ausführen", "Run"),
    ("In allen", "In all"),
    ("Snippet „{}“ löschen?", "Delete snippet “{}”?"),
    // ui/lock.rs
    ("Tresor anlegen", "Create vault"),
    ("Willkommen zurück", "Welcome back"),
    (
        "Ein Master-Passwort verschlüsselt Passwörter und Schlüssel. Es lässt sich nicht wiederherstellen.",
        "A master password encrypts passwords and keys. It cannot be recovered.",
    ),
    (
        "Master-Passwort eingeben, um den Tresor zu öffnen.",
        "Enter the master password to open the vault.",
    ),
    ("Master-Passwort", "Master password"),
    ("Wiederholen", "Repeat"),
    ("Entsperren", "Unlock"),
    ("Mindestens 8 Zeichen.", "At least 8 characters."),
    (
        "Passwörter stimmen nicht überein.",
        "Passwords do not match.",
    ),
];
