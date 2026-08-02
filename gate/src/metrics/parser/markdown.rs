use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;

use super::{ParseError, canonical_path, python_splitlines};

/// One case-mismatched tracked Markdown target.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MarkdownLinkFinding {
    source_path: String,
    line: u64,
    raw_target: String,
    resolved_path: String,
    expected_path: String,
}

impl MarkdownLinkFinding {
    #[must_use]
    pub fn source_path(&self) -> &str {
        &self.source_path
    }

    #[must_use]
    pub const fn line(&self) -> u64 {
        self.line
    }

    #[must_use]
    pub fn raw_target(&self) -> &str {
        &self.raw_target
    }

    #[must_use]
    pub fn resolved_path(&self) -> &str {
        &self.resolved_path
    }

    #[must_use]
    pub fn expected_path(&self) -> &str {
        &self.expected_path
    }
}

/// Scan one tracked Markdown source against a complete projected inventory.
///
/// # Errors
///
/// Missing/non-Markdown sources are not-applicable. Non-ASCII paths are a
/// release-owned refusal because Rust lowercase is not Python Unicode
/// `casefold`; substituting it would change target identity.
pub fn scan_markdown_link_case(
    source_path: &str,
    text: &str,
    tracked_paths: &[String],
) -> Result<Vec<MarkdownLinkFinding>, ParseError> {
    let source_path = canonical_path(source_path)?;
    if !source_path.to_ascii_lowercase().ends_with(".md") {
        return Err(ParseError::NotApplicable {
            reason: format!("tracked source {source_path:?} is not Markdown"),
        });
    }
    require_ascii_path(&source_path, "tracked Markdown source")?;
    let tracked = tracked_case_map(tracked_paths)?;
    let mut findings = Vec::new();
    for (offset, line) in python_splitlines(text).into_iter().enumerate() {
        for captures in link_regex().captures_iter(line) {
            let raw = captures.get(1).map_or("", |value| value.as_str());
            let destination = destination(raw);
            let Some(resolved) = resolve_target(&source_path, &destination)? else {
                continue;
            };
            if let Some(expected) = tracked.get(&resolved.to_ascii_lowercase())
                && expected != &resolved
            {
                findings.push(MarkdownLinkFinding {
                    source_path: source_path.clone(),
                    line: u64::try_from(offset + 1).expect("line number fits in u64"),
                    raw_target: destination,
                    resolved_path: resolved,
                    expected_path: expected.clone(),
                });
            }
        }
    }
    findings.sort();
    Ok(findings)
}

fn tracked_case_map(paths: &[String]) -> Result<BTreeMap<String, String>, ParseError> {
    let mut tracked = BTreeMap::new();
    for path in paths {
        let path = canonical_path(path)?;
        require_ascii_path(&path, "tracked path inventory")?;
        let folded = path.to_ascii_lowercase();
        if let Some(previous) = tracked.insert(folded.clone(), path.clone())
            && previous != path
        {
            return Err(ParseError::collision(
                folded,
                format!("tracked paths {previous:?} and {path:?} differ only by case"),
            ));
        }
    }
    Ok(tracked)
}

fn require_ascii_path(path: &str, field: &str) -> Result<(), ParseError> {
    if path.is_ascii() {
        Ok(())
    } else {
        Err(ParseError::unsupported(
            field,
            format!("path {path:?} requires Python Unicode casefold semantics"),
            "rename the tracked path to ASCII or compile an exact Unicode casefold provider",
        ))
    }
}

fn link_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"!?\[[^\]\n]*\]\(([^)\n]*)\)").expect("compiled Markdown link regex is valid")
    })
}

fn url_scheme_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"^[A-Za-z][A-Za-z0-9+.-]*:").expect("compiled URL scheme regex is valid")
    })
}

fn destination(raw: &str) -> String {
    let stripped = raw.trim();
    if let Some(inside) = stripped.strip_prefix('<') {
        return inside
            .split_once('>')
            .map_or(inside, |(target, _)| target)
            .trim()
            .to_owned();
    }
    stripped.split_whitespace().next().unwrap_or("").to_owned()
}

fn resolve_target(source: &str, raw: &str) -> Result<Option<String>, ParseError> {
    let stripped = raw.trim();
    let without_fragment = stripped.split_once('#').map_or(stripped, |(path, _)| path);
    let path_target = without_fragment
        .split_once('?')
        .map_or(without_fragment, |(path, _)| path)
        .trim();
    let outside = stripped.is_empty()
        || stripped.starts_with('#')
        || url_scheme_regex().is_match(stripped)
        || path_target.is_empty()
        || path_target.starts_with('/');
    if outside {
        return Ok(None);
    }
    require_ascii_path(path_target, "markdown link target")?;
    let parent = Path::new(source).parent().unwrap_or_else(|| Path::new(""));
    normalize_relative(&parent.to_string_lossy(), path_target)
}

fn normalize_relative(parent: &str, target: &str) -> Result<Option<String>, ParseError> {
    let mut parts = Vec::new();
    for part in parent.split('/').chain(target.split('/')) {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Ok(None);
                }
            }
            value => parts.push(value),
        }
    }
    if parts.is_empty() {
        Ok(Some(".".to_owned()))
    } else {
        canonical_path(&parts.join("/")).map(Some)
    }
}
