//! English texts for the German source strings (see `super::tr`): `{{placeholders}}` in
//! log commands.

pub const T: &[(&str, &str)] = &[
    (
        "Erscheinen in der Log-Leiste einer SSH-Session (Ctrl+Shift+L) und unter „Log ansehen“. Eine Datei wird mit tail -F verfolgt, alles andere als Befehl ausgeführt. Mit {{variable}} oder {{variable=Standard}} fragt Aurora beim Start nach dem Wert, z. B. journalctl -u {{dienst}} -n {{zeilen=200}} – ein Befehl für alle Dienste.",
        "Shown in the log panel of an SSH session (Ctrl+Shift+L) and under \"View log\". A file is followed with tail -F, anything else runs as a command. With {{variable}} or {{variable=default}} Aurora asks for the value when it starts, e.g. journalctl -u {{service}} -n {{lines=200}} – one command for every service.",
    ),
    ("Werte für den Log-Befehl", "Values for the log command"),
    (
        "Platzhalter ausfüllen – die letzten Werte merkt sich Aurora",
        "Fill in the placeholders – Aurora remembers the last values",
    ),
    ("▶ Starten", "▶ Start"),
];
