//! English texts for the German source strings (see `super::tr`): 0.6.3 – jump chains and
//! route finder, certificate radar, Wake-on-LAN.

pub const T: &[(&str, &str)] = &[
    // Jump chains and route finder
    ("Als Weg speichern", "Save as route"),
    (
        "Die Jump-Kette enthält eine Schleife",
        "The jump chain contains a loop",
    ),
    (
        "Ein Jump-Host der Kette existiert nicht mehr",
        "A jump host of the chain no longer exists",
    ),
    ("Erreichbar über:", "Reachable via:"),
    ("Erreichbar über: {}", "Reachable via: {}"),
    ("Erst eine Adresse eintragen", "Enter an address first"),
    (
        "Frage {} mögliche Jump-Hosts… ({} fertig)",
        "Asking {} possible jump hosts… ({} done)",
    ),
    (
        "Fragt deine anderen SSH-Hosts, ob sie diesen Host erreichen – für Hosts hinter einem Bastion/Jump-Host",
        "Asks your other SSH hosts whether they reach this host – for hosts behind a bastion/jump host",
    ),
    ("Gefragte Hosts", "Hosts asked"),
    (
        "Kein anderer SSH-Host mit gespeicherten Zugangsdaten, über den Aurora es versuchen könnte",
        "No other SSH host with stored credentials that Aurora could try",
    ),
    (
        "Kein anderer Weg zu {} gefunden",
        "No other route to {} found",
    ),
    (
        "Keiner der gefragten Hosts erreicht das Ziel.",
        "None of the hosts asked reaches the target.",
    ),
    (
        "Mehr als 4 Jump-Hosts hintereinander",
        "More than 4 jump hosts in a row",
    ),
    (
        "Ohne gespeicherte Zugangsdaten übersprungen: {}",
        "Skipped (no stored credentials): {}",
    ),
    ("Route", "Route"),
    (
        "Trägt den Host als „Verbinden über“ ein – künftig geht jede Verbindung diesen Weg",
        "Sets the host as “Connect via” – every future connection takes this route",
    ),
    ("Ungültiger Hostname", "Invalid hostname"),
    (
        "Von diesem PC aus nicht erreichbar – aber über einen deiner Hosts:",
        "Not reachable from this PC – but through one of your hosts:",
    ),
    ("Weg finden · {}", "Find route · {}"),
    (
        "Weg finden: fragt deine anderen SSH-Hosts, ob sie diesen Host erreichen",
        "Find route: asks your other SSH hosts whether they reach this host",
    ),
    ("Weg finden…", "Find route…"),
    ("Weg: {} → dieser Host", "Route: {} → this host"),
    ("erreicht das Ziel nicht", "doesn't reach the target"),
    (
        "erreicht das Ziel, erlaubt aber keine Weiterleitung (AllowTcpForwarding)",
        "reaches the target but doesn't allow forwarding (AllowTcpForwarding)",
    ),
    ("prüft…", "checking…"),
    ("{} geht jetzt über {}", "{} now goes via {}"),
    (
        "{} hat keine gespeicherten Zugangsdaten (Identität mit Passwort oder Key)",
        "{} has no stored credentials (identity with password or key)",
    ),
    (
        "{} ist direkt nicht erreichbar – Aurora sucht einen Weg über deine anderen Hosts…",
        "{} isn't reachable directly – Aurora is looking for a route through your other hosts…",
    ),
    ("{} von {} geprüft", "{} of {} checked"),
    ("über {}", "via {}"),
    // Certificate radar
    ("1 Zertifikat ist abgelaufen", "1 certificate has expired"),
    (
        "1 Zertifikat läuft in {} Tagen ab",
        "1 certificate expires in {} days",
    ),
    ("Aussteller", "Issuer"),
    (
        "Keine TLS-Zertifikate gefunden. Geprüft werden die Ports aus dem letzten Port-Scan eines Hosts, sonst 443, 8443, 8006, 9443, 636, 993, 995, 465 und 587.",
        "No TLS certificates found. Aurora checks the ports from a host's last port scan, otherwise 443, 8443, 8006, 9443, 636, 993, 995, 465 and 587.",
    ),
    ("Läuft bald ab", "Expiring soon"),
    ("Name passt nicht", "Name mismatch"),
    ("Nichts passt zum Filter", "Nothing matches the filter"),
    ("Stand: {}", "As of: {}"),
    ("Tag", "day"),
    ("Tage", "days"),
    (
        "Täglich im Hintergrund prüfen",
        "Check daily in the background",
    ),
    ("Zertifikats-Radar", "Certificate radar"),
    (
        "Zertifikats-Radar – {} laufen bald ab",
        "Certificate radar – {} expiring soon",
    ),
    ("abgel.", "expired"),
    ("bis {}", "until {}"),
    ("selbst signiert", "self-signed"),
    ("siehe Zertifikats-Radar", "see certificate radar"),
    ("von {}", "by {}"),
    ("{} Zertifikate", "{} certificates"),
    (
        "{} Zertifikate laufen in den nächsten {} Tagen ab",
        "{} certificates expire within the next {} days",
    ),
    (
        "{} Zertifikate sind abgelaufen",
        "{} certificates have expired",
    ),
    ("{} laufen bald ab", "{} expiring soon"),
    ("\nGültig: {} – {}", "\nValid: {} – {}"),
    ("\nKette: {} Zertifikat(e)", "\nChain: {} certificate(s)"),
    (
        "„{}“ steht weder im Zertifikat (SAN) noch im Common Name",
        "“{}” is neither in the certificate's SANs nor its common name",
    ),
    // Wake-on-LAN
    ("Aufwecken", "Wake up"),
    ("Aufwecken abbrechen", "Stop waking"),
    ("Aufwecken und verbinden", "Wake up and connect"),
    (
        "Der Jump-Host kann kein Magic Packet senden (weder wakeonlan, python3, etherwake noch bash)",
        "The jump host can't send a magic packet (no wakeonlan, python3, etherwake or bash)",
    ),
    ("Keine gültige MAC-Adresse", "Not a valid MAC address"),
    (
        "Liest die ARP-/Nachbartabelle dieses PCs – oder die des Jump-Hosts, wenn der Host über einen erreicht wird",
        "Reads the ARP/neighbour table of this PC – or of the jump host if the host is reached through one",
    ),
    (
        "Loopback-Adressen haben keine MAC-Adresse",
        "Loopback addresses have no MAC address",
    ),
    ("MAC ermitteln", "Look up MAC"),
    (
        "MAC von {} nicht in der ARP-Tabelle des Jump-Hosts",
        "MAC of {} not in the jump host's ARP table",
    ),
    (
        "MAC von {} nicht in der ARP-Tabelle – nur Geräte im selben Netz sind sichtbar",
        "MAC of {} not in the ARP table – only devices in the same network are visible",
    ),
    ("MAC-Adresse", "MAC address"),
    ("MAC-Adresse gefunden: {}", "MAC address found: {}"),
    ("Magic Packet an {} gesendet{}", "Magic packet sent to {}{}"),
    (
        "Mit MAC-Adresse zeigt die Hostliste „Aufwecken“, wenn der Host aus ist. Hinter einem Jump-Host sendet der Jump-Host das Paket (Broadcast seines Subnetzes).",
        "With a MAC address the host list offers “Wake up” when the host is off. Behind a jump host, the jump host sends the packet (broadcast of its subnet).",
    ),
    (
        "Ungültige Broadcast-Adresse: {}",
        "Invalid broadcast address: {}",
    ),
    ("Wake-on-LAN-Paket senden", "Send a Wake-on-LAN packet"),
    (
        "Wake-on-LAN: Magic Packet senden, auf den Host warten und verbinden",
        "Wake-on-LAN: send the magic packet, wait for the host and connect",
    ),
    ("wartet auf {}… {} s", "waiting for {}… {} s"),
    ("weckt…", "waking…"),
    (
        "{} ist nach 3 Minuten noch nicht erreichbar – ist Wake-on-LAN im BIOS/der Netzwerkkarte aktiv?",
        "{} still isn't reachable after 3 minutes – is Wake-on-LAN enabled in the BIOS/network card?",
    ),
    ("{} ist wach – verbinde…", "{} is awake – connecting…"),
    (
        "{}: keine gültige MAC-Adresse hinterlegt",
        "{}: no valid MAC address stored",
    ),
];
