//! English texts for the German source strings (see `super::tr`): password-manager
//! integration (Bitwarden, 1Password, KeePassXC, Proton Pass).

pub const T: &[(&str, &str)] = &[
    // backend/pwmanager_types.rs
    ("{}-CLI ({}) nicht gefunden", "{} CLI ({}) not found"),
    (
        "{} ist nicht aktiviert (Einstellungen → Sicherheit → Passwort-Manager)",
        "{} is not enabled (Settings → Security → Password managers)",
    ),
    ("{} ist gesperrt", "{} is locked"),
    (
        "Bitwarden ist nicht angemeldet – im Terminal „bw login“ ausführen",
        "Bitwarden is not logged in – run \"bw login\" in a terminal",
    ),
    (
        "Proton Pass ist nicht angemeldet – im Terminal „pass-cli login“ ausführen",
        "Proton Pass is not logged in – run \"pass-cli login\" in a terminal",
    ),
    (
        "{} ist nicht angemeldet – App entsperren bzw. im Terminal anmelden",
        "{} is not signed in – unlock the app or sign in from a terminal",
    ),
    ("Falsches Passwort", "Wrong password"),
    ("Eintrag nicht gefunden", "Item not found"),
    ("Ungültige Eintrags-Referenz", "Invalid item reference"),
    (
        "Ungültige Einstellung (Pfad/Konto)",
        "Invalid setting (path/account)",
    ),
    ("Zeitüberschreitung (20 s)", "Timed out (20 s)"),
    ("Antwort zu groß", "Response too large"),
    (
        "Unerwartete Antwort des Programms",
        "Unexpected answer from the program",
    ),
    (
        "Der Eintrag enthält kein Passwort bzw. keinen Schlüssel",
        "The item has no password or key",
    ),
    (
        "Programm fehlgeschlagen (Code {})",
        "Program failed (code {})",
    ),
    ("Programm fehlgeschlagen", "Program failed"),
    // backend/pwmanager_cli.rs
    ("nicht gefunden", "not found"),
    ("gesperrt", "locked"),
    ("nicht angemeldet", "not signed in"),
    ("bereit", "ready"),
    // ui/pwmanager_pick.rs
    ("Aus Keychain wählen", "Choose from keychain"),
    ("Suchen…", "Search…"),
    (
        "Suchen (auch im Passwort-Manager)…",
        "Search (also in the password manager)…",
    ),
    ("Für diesen Host merken", "Remember for this host"),
    // ui/pwmanager_ui.rs
    ("Hole Zugangsdaten aus {}…", "Fetching credentials from {}…"),
    ("{} entsperren", "Unlock {}"),
    (
        "Passwort der KeePass-Datenbank",
        "Password of the KeePass database",
    ),
    (
        "Master-Passwort des Tresors",
        "Master password of the vault",
    ),
    ("Zugangsdaten nicht verfügbar", "Credentials not available"),
    ("Passwort eingeben", "Enter password"),
    ("Erneut versuchen", "Try again"),
    ("Suche…", "Searching…"),
    ("Aus Passwort-Manager…", "From password manager…"),
    (
        "Passwort aus Passwort-Manager",
        "Password from password manager",
    ),
    (
        "Kein Passwort-Manager aktiviert. Unter Einstellungen → Sicherheit → Passwort-Manager einschalten.",
        "No password manager enabled. Turn one on under Settings → Security → Password managers.",
    ),
    ("Eintrag suchen…", "Search items…"),
    ("Verwenden als", "Use as"),
    ("SSH-Key (Notiz/Feld)", "SSH key (note/field)"),
    (
        "Benutzernamen des Eintrags verwenden, wenn der Host keinen hat",
        "Use the item's user name if the host has none",
    ),
    ("Passwort-Manager", "Password managers"),
    ("Manager", "Manager"),
    (
        "Befehlszeile „bw“ – Tresor mit Master-Passwort entsperren",
        "Command line \"bw\" – unlock the vault with the master password",
    ),
    (
        "Befehlszeile „op“ – App-Anbindung / Biometrie",
        "Command line \"op\" – app integration / biometrics",
    ),
    (
        "Befehlszeile „keepassxc-cli“ – Datenbank-Passwort",
        "Command line \"keepassxc-cli\" – database password",
    ),
    (
        "Befehlszeile „pass-cli“ – vorher „pass-cli login“",
        "Command line \"pass-cli\" – run \"pass-cli login\" first",
    ),
    ("prüfe…", "checking…"),
    ("Programm", "Program"),
    ("leer = aus PATH ({})", "empty = from PATH ({})"),
    ("Konto", "Account"),
    ("leer = Standardkonto", "empty = default account"),
    ("Datenbank", "Database"),
    ("Schlüsseldatei", "Key file"),
    ("Sitzung vergessen", "Forget session"),
    (
        "Entsperrte Sitzungen nur im Speicher; nach Leerlauf verworfen",
        "Unlocked sessions live in memory only and are dropped when idle",
    ),
    ("beim Sperren von Aurora", "when Aurora locks"),
    ("Sitzungen sperren", "Lock sessions"),
];
