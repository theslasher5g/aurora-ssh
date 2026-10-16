//! English texts for the German source strings (see `super::tr`).

pub const T: &[(&str, &str)] = &[
    // backend/vnc.rs
    (
        "Der VNC-Server hat die Verbindung getrennt",
        "The VNC server closed the connection",
    ),
    (
        "Zeitüberschreitung beim Verbinden mit {}:{}",
        "Timed out connecting to {}:{}",
    ),
    (
        "Verbindung abgelehnt – läuft auf {}:{} ein VNC-Server?",
        "Connection refused – is a VNC server running on {}:{}?",
    ),
    (
        "Verbindung zu {}:{} fehlgeschlagen: {}",
        "Connection to {}:{} failed: {}",
    ),
    ("Verbinde mit {}:{}…", "Connecting to {}:{}…"),
    (
        "Zeitüberschreitung bei der VNC-Anmeldung",
        "Timed out during VNC sign-in",
    ),
    (
        "Der VNC-Server bietet keine unterstützte Anmeldung an ({}). Unterstützt werden „Keine“ und „VNC-Passwort“.",
        "The VNC server offers no supported authentication ({}). Supported are \"None\" and \"VNC password\".",
    ),
    (
        "Auf dem Mac unter Systemeinstellungen → Allgemein → Teilen → Bildschirmfreigabe → ⓘ die Option „VNC-Viewer dürfen den Bildschirm mit Passwort steuern“ aktivieren.",
        "On the Mac, enable \"VNC viewers may control screen with password\" under System Settings → General → Sharing → Screen Sharing → ⓘ.",
    ),
    (
        "Am Server die Sicherheitstypen „VncAuth“ oder „None“ erlauben (TigerVNC: -SecurityTypes VncAuth).",
        "Allow the security types \"VncAuth\" or \"None\" on the server (TigerVNC: -SecurityTypes VncAuth).",
    ),
    (
        "Keine Antwort vom VNC-Server",
        "No response from the VNC server",
    ),
    ("Das ist kein VNC-Server", "This is not a VNC server"),
    (
        "Der VNC-Server lehnt die Verbindung ab: {}",
        "The VNC server rejected the connection: {}",
    ),
    (
        "Der VNC-Server verlangt ein Passwort",
        "The VNC server requires a password",
    ),
    ("Anmeldung…", "Signing in…"),
    ("Passwort falsch", "Wrong password"),
    ("Anmeldung abgelehnt", "Sign-in rejected"),
    (
        "Ungültige Bildschirmgröße vom VNC-Server: {}×{}",
        "Invalid screen size from the VNC server: {}×{}",
    ),
    (
        "Unbekannte Nachricht vom VNC-Server ({})",
        "Unknown message from the VNC server ({})",
    ),
    (
        "Ungültige Daten vom VNC-Server",
        "Invalid data from the VNC server",
    ),
    (
        "Der VNC-Server sendet eine nicht unterstützte Kodierung ({})",
        "The VNC server sends an unsupported encoding ({})",
    ),
    // ui/vnc.rs
    ("Originalgröße (1:1)", "Actual size (1:1)"),
    ("Ansicht anpassen", "Fit to view"),
    ("Bild neu anfordern", "Refresh screen"),
    ("Zwischenablage senden", "Send clipboard"),
    (
        "Strg+Alt+Entf an den Host senden",
        "Send Ctrl+Alt+Del to the host",
    ),
];
