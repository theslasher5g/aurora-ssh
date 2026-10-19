//! "What's new": the embedded `CHANGELOG.md` split into versions, and the logic that picks
//! which versions to show after an update.

use crate::update::{Version, parse_version};

/// The changelog shipped inside the program.
pub const CHANGELOG: &str = include_str!("../CHANGELOG.md");
/// At most this many versions are shown after an update; the rest is behind a link.
pub const MAX_VERSIONS: usize = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Block {
    /// `### New`
    Heading(String),
    /// `- item` (continuation lines joined)
    Bullet(String),
    /// Anything else.
    Para(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub blocks: Vec<Block>,
}

impl Release {
    pub fn text(&self) -> String {
        let (a, b, c) = self.version;
        format!("{a}.{b}.{c}")
    }
}

/// Splits a changelog into releases (newest first, as written). `## 0.7.1`, `## [0.7.1]`
/// and `## v0.7.1 – 2026-10-01` are all accepted; sections without a version are skipped.
pub fn parse(md: &str) -> Vec<Release> {
    let mut out: Vec<Release> = Vec::new();
    let mut skipping = true;
    let mut prev_blank = true;
    for raw in md.lines() {
        let line = raw.trim_end();
        let was_blank = std::mem::replace(&mut prev_blank, line.trim().is_empty());
        if let Some(rest) = line.strip_prefix("## ") {
            let tag = rest
                .trim()
                .trim_start_matches('[')
                .split(|c: char| c.is_whitespace() || c == ']')
                .next()
                .unwrap_or_default();
            match parse_version(tag).filter(|_| tag.chars().any(|c| c.is_ascii_digit())) {
                Some(version) => {
                    out.push(Release {
                        version,
                        blocks: Vec::new(),
                    });
                    skipping = false;
                }
                None => skipping = true,
            }
            continue;
        }
        if skipping || line.starts_with("# ") {
            continue;
        }
        let Some(rel) = out.last_mut() else { continue };
        if let Some(h) = line.strip_prefix("### ") {
            rel.blocks.push(Block::Heading(h.trim().to_string()));
        } else if let Some(item) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            rel.blocks.push(Block::Bullet(item.trim().to_string()));
        } else if line.trim().is_empty() {
            continue;
        } else if !was_blank
            && matches!(
                rel.blocks.last(),
                Some(Block::Bullet(_)) | Some(Block::Para(_))
            )
        {
            // Continuation of the previous item / paragraph.
            match rel.blocks.last_mut() {
                Some(Block::Bullet(t)) | Some(Block::Para(t)) => {
                    t.push(' ');
                    t.push_str(line.trim());
                }
                _ => rel.blocks.push(Block::Para(line.trim().to_string())),
            }
        } else {
            rel.blocks.push(Block::Para(line.trim().to_string()));
        }
    }
    out
}

/// A piece of a line with simple Markdown formatting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Span {
    Plain(String),
    Bold(String),
    Code(String),
    Italic(String),
}

/// `**bold**` and `` `code` `` (an unclosed marker stays literal).
pub fn spans(text: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut plain = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("**")
            && let Some(end) = r.find("**")
            && end > 0
        {
            flush(&mut plain, &mut out);
            out.push(Span::Bold(r[..end].to_string()));
            rest = &r[end + 2..];
        } else if let Some(r) = rest.strip_prefix('`')
            && let Some(end) = r.find('`')
            && end > 0
        {
            flush(&mut plain, &mut out);
            out.push(Span::Code(r[..end].to_string()));
            rest = &r[end + 1..];
        } else if let Some(r) = rest.strip_prefix('*')
            && !r.starts_with(['*', ' '])
            && let Some(end) = r.find('*')
            && end > 0
        {
            flush(&mut plain, &mut out);
            out.push(Span::Italic(r[..end].to_string()));
            rest = &r[end + 1..];
        } else {
            let c = rest.chars().next().unwrap_or(' ');
            plain.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    flush(&mut plain, &mut out);
    out
}

fn flush(plain: &mut String, out: &mut Vec<Span>) {
    if !plain.is_empty() {
        out.push(Span::Plain(std::mem::take(plain)));
    }
}

/// Releases newer than `last_seen` and not newer than `current`, newest first. `last_seen`
/// `None` = unknown: only the current version.
pub fn newer_than(rels: &[Release], last_seen: Option<Version>, current: Version) -> Vec<&Release> {
    let mut v: Vec<&Release> = rels
        .iter()
        .filter(|r| r.version <= current)
        .filter(|r| match last_seen {
            Some(seen) => r.version > seen,
            None => r.version == current,
        })
        .collect();
    v.sort_by_key(|r| std::cmp::Reverse(r.version));
    v
}

/// The latest releases up to `current` (for opening the sheet by hand).
pub fn latest(rels: &[Release], current: Version) -> Vec<&Release> {
    let mut v: Vec<&Release> = rels.iter().filter(|r| r.version <= current).collect();
    v.sort_by_key(|r| std::cmp::Reverse(r.version));
    v
}

/// What happens when the app starts after unlocking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Startup {
    /// Nothing to do.
    Nothing,
    /// Remember the version without showing anything (fresh install).
    Record,
    /// Show the sheet.
    Show,
}

pub fn startup(last_seen: &str, onboarded: bool, current: Version) -> Startup {
    let seen = last_seen.trim();
    if seen.is_empty() {
        // A fresh install has not finished the tour yet; older installs have.
        return if onboarded {
            Startup::Show
        } else {
            Startup::Record
        };
    }
    match parse_version(seen) {
        Some(v) if current > v => Startup::Show,
        Some(_) => Startup::Nothing,
        // Garbage in the file: start over silently.
        None => Startup::Record,
    }
}

/// `0.7.1`
pub fn version_text(v: Version) -> String {
    format!("{}.{}.{}", v.0, v.1, v.2)
}

pub fn release_url(v: Version) -> String {
    format!(
        "https://github.com/{}/releases/tag/v{}",
        crate::update::REPO,
        version_text(v)
    )
}

pub fn changelog_url() -> String {
    format!(
        "https://github.com/{}/blob/main/CHANGELOG.md",
        crate::update::REPO
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Changelog

## 0.7.1

### New
- **Local terminal:** your own shell on this PC in a tab –
  `Ctrl+Shift+T` or the palette.
- Plain item

### Fixes
- The *Update available* button opens *Data & updates*.

## [0.7.0] - 2026-09-30

### Proxmox
- **Live graphs:** CPU sparkline.

## Unreleased
- ignored

## 0.6.2
Just a paragraph.
";

    #[test]
    fn parses_versions_and_blocks() {
        let r = parse(SAMPLE);
        assert_eq!(
            r.iter().map(|r| r.version).collect::<Vec<_>>(),
            [(0, 7, 1), (0, 7, 0), (0, 6, 2)]
        );
        assert_eq!(r[0].blocks[0], Block::Heading("New".into()));
        assert_eq!(
            r[0].blocks[1],
            Block::Bullet(
                "**Local terminal:** your own shell on this PC in a tab – `Ctrl+Shift+T` or the palette."
                    .into()
            )
        );
        assert_eq!(r[0].blocks.len(), 5);
        assert_eq!(r[2].blocks, [Block::Para("Just a paragraph.".into())]);
    }

    #[test]
    fn parses_the_real_changelog() {
        let r = parse(CHANGELOG);
        assert!(r.len() >= 3, "{} releases", r.len());
        assert!(r.iter().all(|x| !x.blocks.is_empty()));
        assert!(r.windows(2).all(|w| w[0].version > w[1].version));
        // The program's own version has a section.
        assert!(r.iter().any(|x| x.version <= crate::update::current()));
    }

    #[test]
    fn inline_formatting() {
        assert_eq!(
            spans("a **b** `c` d"),
            [
                Span::Plain("a ".into()),
                Span::Bold("b".into()),
                Span::Plain(" ".into()),
                Span::Code("c".into()),
                Span::Plain(" d".into())
            ]
        );
        assert_eq!(spans("**open"), [Span::Plain("**open".into())]);
        assert_eq!(
            spans("a *b* c * d"),
            [
                Span::Plain("a ".into()),
                Span::Italic("b".into()),
                Span::Plain(" c * d".into())
            ]
        );
        assert_eq!(spans("a ` b"), [Span::Plain("a ` b".into())]);
        assert_eq!(
            spans("äö **ü**"),
            [Span::Plain("äö ".into()), Span::Bold("ü".into())]
        );
        assert!(spans("").is_empty());
    }

    #[test]
    fn selects_newer_versions() {
        let r = parse(SAMPLE);
        let cur = (0, 7, 1);
        let v = |s: Option<Version>| {
            newer_than(&r, s, cur)
                .iter()
                .map(|x| x.version)
                .collect::<Vec<_>>()
        };
        assert_eq!(v(Some((0, 7, 0))), [(0, 7, 1)]);
        assert_eq!(v(Some((0, 6, 2))), [(0, 7, 1), (0, 7, 0)]);
        assert_eq!(v(Some((0, 6, 0))).len(), 3);
        assert!(v(Some((0, 7, 1))).is_empty());
        // Unknown last version: only the current one.
        assert_eq!(v(None), [(0, 7, 1)]);
        // A changelog entry for a future version is not shown.
        assert_eq!(newer_than(&r, Some((0, 6, 0)), (0, 7, 0)).len(), 2);
    }

    #[test]
    fn latest_is_capped_by_current() {
        let r = parse(SAMPLE);
        assert_eq!(latest(&r, (0, 7, 0))[0].version, (0, 7, 0));
        assert_eq!(latest(&r, (9, 0, 0)).len(), 3);
    }

    #[test]
    fn startup_decisions() {
        let cur = (0, 7, 1);
        assert_eq!(startup("", false, cur), Startup::Record);
        assert_eq!(startup("", true, cur), Startup::Show);
        assert_eq!(startup("0.7.0", true, cur), Startup::Show);
        assert_eq!(startup("v0.7.0", false, cur), Startup::Show);
        assert_eq!(startup("0.7.1", true, cur), Startup::Nothing);
        assert_eq!(startup("0.8.0", true, cur), Startup::Nothing);
        assert_eq!(startup("banana", true, cur), Startup::Record);
    }

    #[test]
    fn version_ordering_is_numeric() {
        assert!(parse_version("0.10.0") > parse_version("0.9.9"));
        assert_eq!(version_text((1, 2, 3)), "1.2.3");
        assert!(release_url((0, 7, 1)).ends_with("/releases/tag/v0.7.1"));
    }
}
