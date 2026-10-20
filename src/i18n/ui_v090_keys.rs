//! English texts for the German source strings (see `super::tr`): adding existing SSH keys
//! to the keychain (drop files, import from `~/.ssh`).

pub const T: &[(&str, &str)] = &[
    (
        "Datei zu groß für einen Schlüssel",
        "file too big for a key",
    ),
    ("keine Textdatei", "not a text file"),
    (
        "nur der öffentliche Schlüssel (.pub) – der private fehlt",
        "public key only (.pub) – the private one is missing",
    ),
    (
        "PuTTY-Format (.ppk) – in PuTTYgen als OpenSSH exportieren",
        "PuTTY format (.ppk) – export it as OpenSSH in PuTTYgen",
    ),
    ("kein privater SSH-Schlüssel", "not a private SSH key"),
    ("{} Schlüssel hinzugefügt", "{} keys added"),
    ("{} schon vorhanden", "{} already there"),
    (
        "{} mit Passphrase – bitte eintragen",
        "{} with passphrase – please enter it",
    ),
    (
        "Schlüsseldatei hier ablegen – sie wird in den Schlüsselbund aufgenommen",
        "Drop the key file here – it is added to the keychain",
    ),
    (
        "In ~/.ssh gibt es keine neuen Schlüssel",
        "There are no new keys in ~/.ssh",
    ),
    ("Aus ~/.ssh", "From ~/.ssh"),
    (
        "Sucht private Schlüssel in ~/.ssh, die noch nicht im Schlüsselbund sind",
        "Looks for private keys in ~/.ssh that are not in the keychain yet",
    ),
    ("Schlüssel aus ~/.ssh hinzufügen", "Add keys from ~/.ssh"),
    ("mit Passphrase", "with passphrase"),
    ("{} hinzufügen", "Add {}"),
    (
        "Speichere Passwörter und SSH-Keys einmal und verwende sie für beliebig viele Hosts. Vorhandene Schlüsseldateien kannst du einfach hierher ziehen.",
        "Save passwords and SSH keys once and use them for any number of hosts. You can simply drag existing key files here.",
    ),
];
