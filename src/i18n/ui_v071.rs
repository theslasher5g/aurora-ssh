//! English texts for the German source strings (see `super::tr`): Proxmox guest consoles
//! (VM login sheet, serial console over the API, browser fallback).

pub const T: &[(&str, &str)] = &[
    (
        "Keine Berechtigung für die Konsole – das Konto oder Token braucht VM.Console auf /vms/{}",
        "No permission for the console – the account or token needs VM.Console on /vms/{}",
    ),
    (
        "Öffne Konsole von {} auf {}…",
        "Opening console of {} on {}…",
    ),
    (
        "pct enter auf dem Node (über den SSH-Host), ohne SSH-Host über die Proxmox-API",
        "pct enter on the node (via the SSH host), through the Proxmox API without one",
    ),
    (
        "Gespeicherter Host oder SSH zur IP der VM (Benutzer aus Cloud-Init); ohne IP die serielle Schnittstelle über die API, sonst noVNC im Browser",
        "Saved host or SSH to the VM's IP (user from cloud-init); without an IP the serial port through the API, otherwise noVNC in the browser",
    ),
    (
        "SSH-Host für die Nodes (optional): Container öffnen sich dort mit „pct enter“, ohne SSH-Host über die Proxmox-API. VMs ohne IP nutzen ihre serielle Schnittstelle über die API – „qm terminal“ nur, wenn das Konto kein VM.Console hat. Liegt der Gast auf einem anderen Node, springt Aurora per SSH dorthin.",
        "SSH host for the nodes (optional): containers open there with “pct enter”, through the Proxmox API without one. VMs without an IP use their serial port through the API – “qm terminal” only when the account lacks VM.Console. If the guest lives on another node, Aurora hops there via SSH.",
    ),
    (
        "„{}“: keine IP und keine serielle Schnittstelle – Konsole im Browser geöffnet",
        "“{}”: no IP and no serial port – console opened in the browser",
    ),
    ("Serielle Konsole · {}", "Serial console · {}"),
    ("Konsole · {}", "Console · {}"),
    ("Anmelden an {}", "Log in to {}"),
    (
        "SSH zu {} – Benutzer aus Cloud-Init, falls gesetzt.",
        "SSH to {} – user from cloud-init, if set.",
    ),
    ("— keine (Passwort) —", "— none (password) —"),
    ("Ungültiger Benutzername", "Invalid user name"),
    ("Serielle Konsole", "Serial console"),
    (
        "Serielle Schnittstelle der VM über die Proxmox-API",
        "The VM's serial port through the Proxmox API",
    ),
];
