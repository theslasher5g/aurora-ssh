pub mod dialogs;
pub mod editors;
pub mod hosts;
pub mod keychain;
pub mod lock;
pub mod palette;
pub mod settings;
pub mod sftp;
pub mod snippets;

/// Case-insensitive subsequence match; returns a score (higher is better).
pub fn fuzzy(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let text_l = text.to_lowercase();
    let query_l = query.to_lowercase();
    if let Some(pos) = text_l.find(&query_l) {
        return Some(1000 - pos as i32 - text_l.len() as i32 / 4);
    }
    let mut score = 0;
    let mut chars = text_l.chars().enumerate();
    let mut last = 0usize;
    for q in query_l.chars() {
        let (i, _) = chars.by_ref().find(|(_, c)| *c == q)?;
        score -= (i - last) as i32;
        last = i;
    }
    Some(score)
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

pub fn relative_time(t: chrono::DateTime<chrono::Utc>) -> String {
    let secs = (chrono::Utc::now() - t).num_seconds().max(0);
    match secs {
        0..=59 => "gerade eben".into(),
        60..=3599 => format!("vor {} min", secs / 60),
        3600..=86399 => format!("vor {} h", secs / 3600),
        _ => format!("vor {} Tagen", secs / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_matching() {
        assert!(fuzzy("web", "web-01").unwrap() > fuzzy("wb", "web-01").unwrap());
        assert!(fuzzy("xyz", "web-01").is_none());
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2048), "2.0 KB");
    }
}
