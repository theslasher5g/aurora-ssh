//! English texts: web interface entries (firewall, switch, NAS … in the `aurora-web`
//! helper window), vendor templates and certificate pinning.

pub const T: &[(&str, &str)] = &[
    // Messages of the URL check (looked up dynamically).
    ("Keine Adresse angegeben", "No address given"),
    ("Die Adresse ist zu lang", "The address is too long"),
    (
        "Nur http:// und https:// sind erlaubt",
        "Only http:// and https:// are allowed",
    ),
    (
        "Die Adresse enthält keinen Host",
        "The address contains no host",
    ),
    (
        "Benutzer/Passwort gehören nicht in die Adresse",
        "User name and password do not belong in the address",
    ),
    ("Ungültiger Host-Name", "Invalid host name"),
    ("Ungültiger Port", "Invalid port"),
    (
        "Die Adresse enthält ungültige Zeichen",
        "The address contains invalid characters",
    ),
    // Vendor templates (looked up dynamically).
    ("Sophos Firewall", "Sophos Firewall"),
    (
        "Standard-Anmeldung: admin / pfsense (sofort ändern)",
        "Default login: admin / pfsense (change it right away)",
    ),
    (
        "Standard-Anmeldung: root / opnsense (sofort ändern)",
        "Default login: root / opnsense (change it right away)",
    ),
    (
        "Admin-Port oft 443 oder 8443",
        "Admin port is often 443 or 8443",
    ),
    ("Admin-Konsole auf Port 4444", "Admin console on port 4444"),
    (
        "UniFi OS; ältere Controller nutzen Port 8443",
        "UniFi OS; older controllers use port 8443",
    ),
    (
        "WebFig ist standardmäßig unverschlüsselt (http) – besser www-ssl aktivieren",
        "WebFig is unencrypted (http) by default – better enable www-ssl",
    ),
    ("DSM: https 5001, http 5000", "DSM: https 5001, http 5000"),
    ("Weboberfläche auf Port 8006", "Web interface on port 8006"),
    ("Weboberfläche auf Port 8007", "Web interface on port 8007"),
    (
        "Remote-Management-Karte (BMC)",
        "Remote management card (BMC)",
    ),
    // Dialog, form and tab.
    ("Web-Oberfläche", "Web interface"),
    ("— keine (eigene Adresse) —", "— none (own address) —"),
    ("TLS-Zertifikat", "TLS certificate"),
    ("Streng", "Strict"),
    ("Selbstsigniert (anheften)", "Self-signed (pin)"),
    ("Alles ignorieren", "Ignore all"),
    (
        "Nur Zertifikate, denen das System vertraut. Selbstsignierte Geräte werden abgelehnt.",
        "Only certificates the system trusts. Self-signed devices are rejected.",
    ),
    (
        "Beim ersten Öffnen zeigt Aurora den Fingerabdruck des Zertifikats und fragt. Danach wird genau dieses Zertifikat akzeptiert; ändert es sich, warnt Aurora laut.",
        "On first open Aurora shows the certificate fingerprint and asks. After that exactly this certificate is accepted; if it changes, Aurora warns loudly.",
    ),
    (
        "ACHTUNG: Alle Zertifikatsfehler werden ignoriert – jeder im Netz könnte sich als dieses Gerät ausgeben und das Passwort mitlesen. Nur für Geräte, die du nicht anders erreichst.",
        "WARNING: All certificate errors are ignored – anyone on the network could impersonate this device and read the password. Only for devices you cannot reach any other way.",
    ),
    ("Angeheftet", "Pinned"),
    ("Automatisch anmelden", "Log in automatically"),
    (
        "Füllt das Anmeldeformular mit Benutzer und Passwort der Identität (oder des Passwort-Managers) oben. Das Passwort wird nur an das Fenster dieses Eintrags übergeben.",
        "Fills the login form with the user name and password of the identity (or password manager) above. The password is only handed to this entry's window.",
    ),
    ("Anmelden-Taste drücken", "Press the login button"),
    ("Benutzer-Feld", "User field"),
    ("Passwort-Feld", "Password field"),
    (
        "CSS-Selektor, leer = automatisch",
        "CSS selector, empty = detect",
    ),
    (
        "Automatisch heißt: erstes sichtbares Passwortfeld und das Textfeld davor. Es wird höchstens einmal pro Öffnen abgeschickt, damit ein falsches Passwort das Konto nicht sperrt.",
        "Detect means: first visible password field and the text field before it. It is submitted at most once per open so a wrong password cannot lock the account.",
    ),
    (
        "Die Adresse ist unverschlüsselt (http): das Passwort ginge im Klartext durchs Netz.",
        "The address is unencrypted (http): the password would cross the network in clear text.",
    ),
    ("Weitere Hosts", "More hosts"),
    (
        "z.B. sso.example.com (Anmelde-Weiterleitung)",
        "e.g. sso.example.com (login redirect)",
    ),
    ("Gespeicherte Anmeldungen löschen", "Delete saved logins"),
    (
        "Löscht Cookies, Anmeldungen und Cache dieses Eintrags (nur möglich, solange das Fenster zu ist)",
        "Deletes cookies, logins and cache of this entry (only while the window is closed)",
    ),
    (
        "Jeder Eintrag hat sein eigenes Profil: Cookies und Anmeldungen sind getrennt. Links zu anderen Adressen öffnen im Standard-Browser; Downloads landen in Downloads/Aurora.",
        "Every entry has its own profile: cookies and logins are separate. Links to other addresses open in the default browser; downloads go to Downloads/Aurora.",
    ),
    ("Fenster wird geöffnet…", "Opening window…"),
    (
        "{}: Zertifikat wartet auf deine Bestätigung",
        "{}: certificate waits for your confirmation",
    ),
    (
        "Link im Browser geöffnet: {}",
        "Link opened in the browser: {}",
    ),
    (
        "Link nicht geöffnet (ungültig)",
        "Link not opened (invalid)",
    ),
    ("Download gespeichert: {}", "Download saved: {}"),
    ("Navigation blockiert: {}", "Navigation blocked: {}"),
    (
        "Web-Komponente unerwartet beendet",
        "Web component ended unexpectedly",
    ),
    (
        "Web-Komponente (aurora-web) nicht gefunden",
        "Web component (aurora-web) not found",
    ),
    ("Fenster geöffnet", "Window open"),
    ("Aktuelle Seite", "Current page"),
    ("streng geprüft", "strictly verified"),
    (
        "selbstsigniert erlaubt – noch nichts angeheftet",
        "self-signed allowed – nothing pinned yet",
    ),
    (
        "Fehler werden ignoriert (unsicher)",
        "errors are ignored (unsafe)",
    ),
    (
        "Das Zertifikat wurde abgelehnt (SHA-256 {}). Stelle den Eintrag auf „Selbstsigniert (anheften)“, wenn du diesem Gerät vertraust.",
        "The certificate was rejected (SHA-256 {}). Set the entry to “Self-signed (pin)” if you trust this device.",
    ),
    ("Browser-Engine", "Browser engine"),
    (
        "Die Weboberfläche läuft in einem eigenen Fenster mit eigenem Profil. Schließt du den Tab, schließt auch das Fenster – und umgekehrt.",
        "The web interface runs in its own window with its own profile. Closing the tab closes the window – and vice versa.",
    ),
    ("Neu öffnen", "Reopen"),
    ("Fenster nach vorn", "Bring window to front"),
    ("Eintrag bearbeiten", "Edit entry"),
    ("Zertifikat hat sich geändert", "Certificate has changed"),
    (
        "Das passiert nach einer Neuinstallation oder einem Zertifikatswechsel, kann aber auch ein Angriff sein (jemand gibt sich als dieses Gerät aus). Vertraue nur, wenn du den neuen Fingerabdruck am Gerät geprüft hast.",
        "This happens after a reinstall or a certificate renewal, but can also be an attack (someone impersonating this device). Only trust it if you checked the new fingerprint on the device.",
    ),
    (
        "Selbstsigniertes Zertifikat anheften?",
        "Pin self-signed certificate?",
    ),
    (
        "Das Gerät weist sich mit einem Zertifikat aus, dem das System nicht vertraut. Vergleiche den Fingerabdruck mit der Anzeige am Gerät (oder vertraue deinem eigenen Netz). Aurora akzeptiert danach nur genau dieses Zertifikat.",
        "The device identifies itself with a certificate the system does not trust. Compare the fingerprint with the one shown on the device (or trust your own network). Afterwards Aurora accepts exactly this certificate only.",
    ),
    ("bisher angeheftet", "pinned so far"),
    (
        "Wenn du trotzdem vertrauen willst, tippe den Host ein: {}",
        "If you still want to trust it, type the host: {}",
    ),
    ("Neu anheften und öffnen", "Pin again and open"),
    ("Anheften und öffnen", "Pin and open"),
    ("Nur Web – keine Fakten", "Web only – no facts"),
    ("Web-Oberfläche öffnen", "Open web interface"),
];
