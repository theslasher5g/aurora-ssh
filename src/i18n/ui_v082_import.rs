//! English texts: migration importer (mRemoteNG, Royal TS, Remote Desktop Manager, CSV).

pub const T: &[(&str, &str)] = &[
    (
        "Royal-TS-Dokumente (.rtsz/.rtsx) sind Archive und können nicht gelesen werden. In Royal TS: Datei → Importieren & Exportieren → Exportieren → \"Royal JSON\" wählen und die .json-Datei hier öffnen.",
        "Royal TS documents (.rtsz/.rtsx) are archives and cannot be read. In Royal TS choose File → Import & Export → Export → \"Royal JSON\" and open the .json file here.",
    ),
    (
        "ZIP-Archive werden nicht gelesen. Bitte die Exportdatei (.json, .xml, .csv) entpacken und diese öffnen.",
        "ZIP archives are not read. Please unpack the export file (.json, .xml, .csv) and open that.",
    ),
    (
        "Unbekanntes XML-Format. Unterstützt: mRemoteNG confCons.xml und Remote Desktop Manager .rdm.",
        "Unknown XML format. Supported: mRemoteNG confCons.xml and Remote Desktop Manager .rdm.",
    ),
    ("Die CSV-Datei ist leer.", "The CSV file is empty."),
    (
        "In der CSV-Datei fehlt die Spalte \"host\".",
        "The CSV file has no \"host\" column.",
    ),
    ("Kein Hostname", "No host name"),
    (
        "{} Zeilen enthalten ein Passwort im Klartext. Es wird nur importiert, wenn du es unten ausdrücklich erlaubst (Ablage im Tresor).",
        "{} rows contain a clear-text password. It is only imported if you explicitly allow it below (stored in the vault).",
    ),
    (
        "Keine Kopfzeile erkannt: Spaltenreihenfolge name, host, port, protocol, user, group, tags, description angenommen.",
        "No header row found: assuming the column order name, host, port, protocol, user, group, tags, description.",
    ),
    (
        "Das ist keine mRemoteNG-Datei (Wurzelelement <Connections> fehlt).",
        "This is not an mRemoteNG file (root element <Connections> is missing).",
    ),
    (
        "Die Datei ist vollständig verschlüsselt (FullFileEncryption). In mRemoteNG unter Extras → Optionen → Sicherheit die Verschlüsselung der gesamten Datei ausschalten oder per Datei → Exportieren ohne Verschlüsselung speichern.",
        "The file is fully encrypted (FullFileEncryption). In mRemoteNG turn off whole-file encryption under Tools → Options → Security, or save an export without encryption (File → Export).",
    ),
    (
        "mRemoteNG verschlüsselt die Passwörter in dieser Datei; sie werden nicht entschlüsselt und nicht importiert.",
        "mRemoteNG encrypts the passwords in this file; they are not decrypted and not imported.",
    ),
    (
        "{} Einträge hatten ein Passwort; Passwörter werden bei mRemoteNG nicht importiert.",
        "{} entries had a password; passwords are not imported from mRemoteNG.",
    ),
    (
        "Keine Verbindungen gefunden (<Connection> fehlt).",
        "No connections found (<Connection> is missing).",
    ),
    (
        "Das ist kein Remote-Desktop-Manager-Export.",
        "This is not a Remote Desktop Manager export.",
    ),
    (
        "Anmeldedaten-Eintrag (Passwörter werden nicht importiert)",
        "Credential entry (passwords are not imported)",
    ),
    ("Nicht unterstützter Typ: {}", "Unsupported type: {}"),
    (
        "{} Anmeldedaten-Einträge übersprungen; Passwörter werden aus RDM-Exporten nicht übernommen.",
        "{} credential entries skipped; passwords are not taken from RDM exports.",
    ),
    (
        "{} Einträge hatten ein Passwort, das nicht importiert wird.",
        "{} entries had a password that is not imported.",
    ),
    (
        "Zu viele Einträge (Obergrenze erreicht)",
        "Too many entries (limit reached)",
    ),
    ("Das ist keine Datei.", "This is not a file."),
    (
        "Die Datei ist zu groß (höchstens 50 MB).",
        "The file is too large (50 MB at most).",
    ),
    (
        "Vom Parser übersprungen (siehe Liste)",
        "Skipped while reading (see list)",
    ),
    ("Protokoll {} nicht übernommen", "Protocol {} not imported"),
    ("Ungültiger Hostname", "Invalid host name"),
    (
        "Existiert schon (gleicher Name und Host)",
        "Already exists (same name and host)",
    ),
    ("Import: {}", "Import: {}"),
    (
        "Das ist kein Royal-TS-Export (\"Objects\" fehlt).",
        "This is not a Royal TS export (\"Objects\" is missing).",
    ),
    (
        "{} Anmeldedaten-Objekte nicht importiert: Royal TS exportiert keine Passwörter im Klartext. Benutzernamen wurden den Verbindungen zugeordnet.",
        "{} credential objects not imported: Royal TS does not export passwords in clear text. User names were assigned to the connections.",
    ),
    (
        "Kein Verbindungsobjekt ({})",
        "Not a connection object ({})",
    ),
    ("Lesen abgebrochen.", "Reading was interrupted."),
    (
        "Aus anderem Verbindungsmanager importieren",
        "Import from another connection manager",
    ),
    ("Schließen", "Close"),
    ("{} importieren", "Import {}"),
    ("Abbrechen", "Cancel"),
    ("Andere Datei", "Other file"),
    ("Fertig", "Done"),
    (
        "{} Passwörter aus der Quelle wurden nicht importiert.",
        "{} passwords from the source were not imported.",
    ),
    (
        "{} Hosts importiert, {} übersprungen",
        "{} hosts imported, {} skipped",
    ),
    (
        "Hosts aus mRemoteNG, Royal TS, Remote Desktop Manager oder einer CSV-Tabelle übernehmen. Die Datei wird nur lokal gelesen.",
        "Take over hosts from mRemoteNG, Royal TS, Remote Desktop Manager or a CSV table. The file is only read locally.",
    ),
    ("Pfad zur Exportdatei …", "Path to the export file …"),
    ("Öffnen", "Open"),
    ("Lese Datei …", "Reading file …"),
    ("Datei hierher ziehen", "Drop the file here"),
    ("So exportierst du", "How to export"),
    (
        "confCons.xml (unverschlüsselt) – liegt unter %APPDATA%\\mRemoteNG oder per Datei → Verbindungen exportieren.",
        "confCons.xml (unencrypted) – found under %APPDATA%\\mRemoteNG, or use File → Export connections.",
    ),
    (
        "Datei → Importieren & Exportieren → Exportieren → Royal JSON (.json). Passwörter werden von Royal nicht im Klartext exportiert.",
        "File → Import & Export → Export → Royal JSON (.json). Royal does not export passwords in clear text.",
    ),
    (
        "Datei → Exportieren → JSON (oder .rdm als XML). Anmeldedaten-Einträge werden übersprungen.",
        "File → Export → JSON (or .rdm as XML). Credential entries are skipped.",
    ),
    (
        "Spalten name, host, port, protocol, user, group, tags, description (Kopfzeile, ; oder , als Trenner, optional password).",
        "Columns name, host, port, protocol, user, group, tags, description (header row, ; or , as delimiter, optional password).",
    ),
    ("{} Einträge in {} Ordnern", "{} entries in {} folders"),
    ("{} Konflikte", "{} conflicts"),
    (
        "Verschlüsselte Datei – es kann nichts importiert werden.",
        "Encrypted file – nothing can be imported.",
    ),
    (
        "Passwörter werden nicht importiert ({} Einträge hatten eines).",
        "Passwords are not imported ({} entries had one).",
    ),
    (
        "{} Einträge nicht lesbar / nicht unterstützt",
        "{} entries unreadable / unsupported",
    ),
    ("Ausblenden", "Hide"),
    ("Anzeigen", "Show"),
    ("Ziel", "Target"),
    ("Neue Gruppe", "New group"),
    ("Bestehende Gruppe", "Existing group"),
    ("Oberste Ebene", "Top level"),
    ("Name der neuen Gruppe", "Name of the new group"),
    ("Gruppe wählen …", "Choose a group …"),
    ("Es gibt noch keine Gruppe.", "There is no group yet."),
    (
        "Schon vorhanden (gleicher Name und Host)",
        "Already present (same name and host)",
    ),
    ("Überspringen", "Skip"),
    ("Überschreiben", "Overwrite"),
    ("Beide behalten", "Keep both"),
    ("Protokolle zuordnen", "Map protocols"),
    ("Nicht importieren", "Do not import"),
    (
        "Passwörter aus der Datei im Tresor speichern",
        "Store passwords from the file in the vault",
    ),
    (
        "Die Datei enthält Passwörter im Klartext. Sie landen nur im verschlüsselten Tresor (Schlüsselbund), nie in der Hostliste. Danach die Datei löschen.",
        "The file contains clear-text passwords. They go only into the encrypted vault (keychain), never into the host list. Delete the file afterwards.",
    ),
    (
        "Der Tresor ist nicht entsperrt.",
        "The vault is not unlocked.",
    ),
    ("Einträge", "Entries"),
    ("Suchen …", "Search …"),
    ("Alle", "All"),
    ("Keine", "None"),
    ("{} von {} ausgewählt", "{} of {} selected"),
    ("Konflikt", "Conflict"),
    ("{} Hosts importiert", "{} hosts imported"),
    ("Neu angelegt", "Created"),
    ("Überschrieben", "Overwritten"),
    ("Neue Gruppen", "New groups"),
    ("Passwörter im Tresor", "Passwords in the vault"),
    ("Übersprungen", "Skipped"),
    ("Gründe fürs Überspringen", "Reasons for skipping"),
    ("Nicht gelesen", "Not read"),
    ("… und {} weitere", "… and {} more"),
    ("Anderer Verbindungsmanager", "Other connection manager"),
    (
        "mRemoteNG, Royal TS, Remote Desktop Manager oder CSV importieren",
        "Import mRemoteNG, Royal TS, Remote Desktop Manager or CSV",
    ),
    (
        "Du kommst von mRemoteNG, Royal TS oder RDM?",
        "Coming from mRemoteNG, Royal TS or RDM?",
    ),
];
