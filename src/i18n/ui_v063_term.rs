//! English texts for the German source strings (see `super::tr`): 0.6.3 – command
//! history, snippet suggestions, secret guard on paste and time travel in terminals.

pub const T: &[(&str, &str)] = &[
    ("Zurückspulen (Zeitreise)", "Rewind (time travel)"),
    ("📋 Einfügen", "📋 Paste"),
    ("Privater Schlüssel", "Private key"),
    ("AWS-Zugangsschlüssel", "AWS access key"),
    ("GitHub-Token", "GitHub token"),
    ("Slack-Token", "Slack token"),
    ("JSON Web Token", "JSON Web Token"),
    (
        "Passwort/Secret im Klartext",
        "Password/secret in plain text",
    ),
    ("Zufällig wirkende Zeichenfolge", "Random-looking string"),
    ("{} Zeichen", "{} characters"),
    ("Einfügen in {}?", "Paste into {}?"),
    (
        "Der Text enthält etwas, das wie ein Geheimnis aussieht. Im Terminal landet es in der Shell-History und ggf. in Logs.",
        "The text contains something that looks like a secret. In a terminal it ends up in the shell history and possibly in logs.",
    ),
    ("… und {} weitere", "… and {} more"),
    (
        "{} Zeilen werden eingefügt – eine Enter-Taste führt alle aus.",
        "{} lines will be pasted – one press of Enter runs them all.",
    ),
    (
        "{} Zeilen werden sofort ausgeführt.",
        "{} lines will run immediately.",
    ),
    ("Trotzdem einfügen", "Paste anyway"),
    (
        "Als einzelne Zeile ohne Enter",
        "As a single line without Enter",
    ),
    (
        "Abschalten: Einstellungen → Terminal → Geheimnis-Wächter",
        "Turn off: Settings → Terminal → Secret guard",
    ),
    ("Als Snippet speichern?", "Save as a snippet?"),
    ("Später", "Later"),
    (
        "Diesen Befehl hast du schon {}× ausgeführt.",
        "You have run this command {} times.",
    ),
    ("Speichern…", "Save…"),
    ("Nicht mehr vorschlagen", "Don't suggest again"),
    ("Befehls-Gedächtnis", "Command history"),
    ("einfügen + ausführen", "insert + run"),
    ("einfügen", "insert"),
    (
        "Für diesen Host ist noch nichts gespeichert.",
        "Nothing saved for this host yet.",
    ),
    ("Verlauf durchsuchen", "Search history"),
    ("Gelöschter Host", "Deleted host"),
    (
        "Merkt sich pro Host, was du in SSH-Sessions ausführst – verschlüsselt im Tresor. Suchen: Ctrl+K oder Ctrl+R im Terminal. Befehle mit Leerzeichen am Anfang oder mit Passwörtern/Tokens werden übersprungen.",
        "Remembers per host what you run in SSH sessions – encrypted in the vault. Search: Ctrl+K, or Ctrl+R in the terminal. Commands starting with a space or containing passwords/tokens are skipped.",
    ),
    ("Snippet-Vorschläge", "Snippet suggestions"),
    (
        "Bietet lange Befehle, die du oft wiederholst, als Snippet mit Variablen an",
        "Offers long commands you repeat often as a snippet with variables",
    ),
    ("Verlauf löschen", "Clear history"),
    (
        "{} Befehle von {} Hosts gespeichert",
        "{} commands from {} hosts stored",
    ),
    ("Einzelner Host…", "Single host…"),
    ("Geheimnis-Wächter", "Secret guard"),
    (
        "Warnt vor dem Einfügen von Schlüsseln, Tokens und Passwörtern und vor mehrzeiligem Einfügen auf PROD",
        "Warns before pasting keys, tokens and passwords, and before multi-line pastes on PROD",
    ),
    ("Zeitreise", "Time travel"),
    (
        "Hält die letzten 30 Minuten Ausgabe jeder Session im Speicher (höchstens 16 MB), zum Zurückspulen über die Uhr im Terminal-Kopf",
        "Keeps the last 30 minutes of each session's output in memory (at most 16 MB) to rewind with the clock in the terminal header",
    ),
    ("Verlauf gelöscht", "History cleared"),
    ("Noch keine Ausgabe aufgezeichnet", "No output recorded yet"),
    ("ZEITREISE", "TIME TRAVEL"),
    ("Pause", "Pause"),
    ("jetzt", "now"),
    (
        "Zurück zur laufenden Session (oder einfach tippen)",
        "Back to the live session (or just type)",
    ),
    (
        "Text dieses Bildschirms kopieren",
        "Copy the text of this screen",
    ),
    ("Gedächtnis & Schutz", "History & protection"),
    ("Lokales Terminal", "Local terminal"),
];
