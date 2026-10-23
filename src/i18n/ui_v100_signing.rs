//! English texts for the German source strings (see `super::tr`): signed releases.

pub const T: &[(&str, &str)] = &[
    ("Signaturdatei ist ungültig", "Signature file is invalid"),
    (
        "Signatur der Prüfsummen ist ungültig",
        "The signature of the checksums is invalid",
    ),
    (
        "Dieses Release ist nicht signiert (oder diese Version kann die Signatur nicht prüfen). Nur die Prüfsumme aus demselben Release schützt den Download.",
        "This release is not signed (or this build cannot check the signature). Only the checksum from the same release protects the download.",
    ),
    (
        "Nur signierte Updates installieren",
        "Only install signed updates",
    ),
    (
        "Updates ohne gültige Release-Signatur werden abgelehnt",
        "Updates without a valid release signature are refused",
    ),
    (
        "Nicht signiert – Installation abgelehnt (Einstellung „Nur signierte Updates installieren“)",
        "Not signed - installation refused (setting \"Only install signed updates\")",
    ),
    ("Trotzdem installieren", "Install anyway"),
];
