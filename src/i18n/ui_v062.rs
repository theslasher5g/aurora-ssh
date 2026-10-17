//! English texts for the German source strings (see `super::tr`): 0.6.2 – port scan,
//! persistent session fallback.

pub const T: &[(&str, &str)] = &[
    ("Alle (1–65535)", "All (1–65535)"),
    ("Auf dem Server lauschend", "Listening on the server"),
    ("Bekannte Ports", "Well-known ports"),
    (
        "Dauerhafte Session nicht möglich (kein tmux/screen) – normale Shell",
        "Persistent session not possible (no tmux/screen) – plain shell",
    ),
    ("Eigene", "Custom"),
    ("Erneut scannen", "Scan again"),
    ("Frage den Server…", "Asking the server…"),
    (
        "Fragt den Server per SSH (ss -tln) – zeigt auch Dienste, die nur auf localhost lauschen und per Tunnel erreichbar sind.",
        "Asks the server over SSH (ss -tln) – also shows services that only listen on localhost and can be reached through a tunnel.",
    ),
    ("Im Browser öffnen", "Open in browser"),
    ("Kein offener Port gefunden", "No open port found"),
    ("Keine Ports angegeben", "No ports given"),
    ("Läuft…", "Running…"),
    (
        "Offene Ports des Hosts finden, mit Tunnel per Klick",
        "Find the host's open ports, tunnel in one click",
    ),
    ("Ports scannen…", "Scan ports…"),
    ("Ports · {}", "Ports · {}"),
    ("Tunnel: localhost:{} → {}", "Tunnel: localhost:{} → {}"),
    (
        "Verbindet sich von diesem PC zu jedem Port – zeigt, was von hier erreichbar ist.",
        "Connects from this PC to every port – shows what is reachable from here.",
    ),
    ("Von hier (TCP)", "From here (TCP)"),
    (
        "Weder ss noch netstat auf dem Server gefunden",
        "Neither ss nor netstat found on the server",
    ),
    ("nur localhost", "localhost only"),
    ("{} nicht gefunden", "{} not found"),
    ("{} offen", "{} open"),
    ("{} von {} Ports geprüft", "{} of {} ports checked"),
    (
        "Über SSH auf einen lokalen Port holen (auch wenn der Dienst nur auf localhost lauscht)",
        "Bring it to a local port over SSH (even if the service only listens on localhost)",
    ),
];
