use std::collections::BTreeMap;
#[cfg(not(target_arch = "wasm32"))]
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;
#[cfg(not(target_arch = "wasm32"))]
use tree_sitter::{Language, Node, Parser};

use super::{ParseError, canonical_path, python_splitlines};
#[cfg(not(target_arch = "wasm32"))]
use crate::metrics::declaration::RUST_DOC_MARKDOWN_CITATION_ALLOWANCES;

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

/// One Rust doc-comment citation whose Markdown target is not tracked or allowed.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RustDocMarkdownCitationFinding {
    source_path: String,
    line: u64,
    target_path: String,
}

#[cfg(not(target_arch = "wasm32"))]
impl RustDocMarkdownCitationFinding {
    #[must_use]
    pub fn source_path(&self) -> &str {
        &self.source_path
    }

    #[must_use]
    pub const fn line(&self) -> u64 {
        self.line
    }

    #[must_use]
    pub fn target_path(&self) -> &str {
        &self.target_path
    }
}

/// Complete scan of one Rust source's doc-comment Markdown citations.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RustDocMarkdownCitationScan {
    citations: u64,
    findings: Vec<RustDocMarkdownCitationFinding>,
}

#[cfg(not(target_arch = "wasm32"))]
impl RustDocMarkdownCitationScan {
    #[must_use]
    pub const fn citations(&self) -> u64 {
        self.citations
    }

    #[must_use]
    pub fn findings(&self) -> &[RustDocMarkdownCitationFinding] {
        &self.findings
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

/// Scan Markdown paths named in Rust doc comments against tracked paths.
///
/// A Markdown path spelled in doc text is a citation whether or not it sits in
/// a code span, because a code span is typography and carries no intent. The
/// prose this arm exists to catch can ground a rule in `README.md` without a
/// code span just as readily as with one, so reading only the decorated
/// spelling would refuse citations that were merely typeset and admit the ones
/// doing the work.
///
/// What separates a citation from an incidental mention is therefore not the
/// punctuation around the name but whether the name resolves. Naming a document
/// in doc text sends the reader to it, so it has to be one they can open here.
/// A spelling absent from the live inventory is clean only when an exact
/// compiled declaration says who owns it instead -- an external document this
/// repository does not track, or a path a test plants rather than one a reader
/// is being sent to.
///
/// Line and block doc comments are both admitted. Ordinary comments and string
/// literals are not doc text and never enter the candidate set.
///
/// # Errors
///
/// Non-Rust sources are not applicable. Noncanonical or non-ASCII source,
/// target, or inventory paths refuse rather than acquiring a different path
/// identity from the one reports retain.
#[cfg(not(target_arch = "wasm32"))]
pub fn scan_rust_doc_markdown_citations(
    source_path: &str,
    text: &str,
    tracked_paths: &[String],
) -> Result<RustDocMarkdownCitationScan, ParseError> {
    let source_path = canonical_path(source_path)?;
    if !source_path.to_ascii_lowercase().ends_with(".rs") {
        return Err(ParseError::NotApplicable {
            reason: format!("tracked source {source_path:?} is not Rust"),
        });
    }
    require_ascii_path(&source_path, "tracked Rust source")?;
    let mut parser = Parser::new();
    parser
        .set_language(&Language::new(tree_sitter_rust::LANGUAGE))
        .map_err(|error| {
            ParseError::malformed(
                format!("Rust doc-comment grammar for {source_path:?}"),
                error.to_string(),
            )
        })?;
    let tree = parser.parse(text, None).ok_or_else(|| {
        ParseError::malformed(
            format!("Rust doc-comment source {source_path:?}"),
            "grammar produced no tree",
        )
    })?;
    let tracked = tracked_case_map(tracked_paths)?
        .into_values()
        .collect::<BTreeSet<_>>();
    let mut citations = 0_u64;
    let mut findings = Vec::new();
    visit_rust_doc_comments(
        tree.root_node(),
        text,
        &source_path,
        &tracked,
        &mut citations,
        &mut findings,
    )?;
    findings.sort();
    Ok(RustDocMarkdownCitationScan {
        citations,
        findings,
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn visit_rust_doc_comments(
    node: Node<'_>,
    text: &str,
    source_path: &str,
    tracked: &BTreeSet<String>,
    citations: &mut u64,
    findings: &mut Vec<RustDocMarkdownCitationFinding>,
) -> Result<(), ParseError> {
    if node.kind() == "doc_comment" {
        let fragment = node.utf8_text(text.as_bytes()).map_err(|error| {
            ParseError::malformed(
                format!("Rust doc comment in {source_path:?}"),
                error.to_string(),
            )
        })?;
        for captures in rust_doc_citation_regex().captures_iter(fragment) {
            let matched = captures.get(1).expect("citation regex has one capture");
            let raw = matched.as_str();
            let target_path = canonical_path(raw)?;
            require_ascii_path(&target_path, "Rust doc Markdown target")?;
            *citations = citations.saturating_add(1);
            let allowed = tracked.contains(&target_path)
                || RUST_DOC_MARKDOWN_CITATION_ALLOWANCES
                    .into_iter()
                    .any(|allowance| allowance.matches(source_path, &target_path));
            if !allowed {
                findings.push(RustDocMarkdownCitationFinding {
                    source_path: source_path.to_owned(),
                    line: rust_source_line(text, node.start_byte() + matched.start()),
                    target_path,
                });
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        visit_rust_doc_comments(child, text, source_path, tracked, citations, findings)?;
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn rust_source_line(text: &str, byte_offset: usize) -> u64 {
    let newlines = text.as_bytes()[..byte_offset]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count();
    u64::try_from(newlines + 1).expect("line number fits in u64")
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

/// A Markdown path in doc text, decorated or bare.
///
/// The trailing `\b` ends the match at the name. Without it the pattern reads a
/// Markdown name out of the middle of any longer suffix, so a bundle spelled
/// `page.mdoc` would be refused for not tracking a document nobody wrote.
#[cfg(not(target_arch = "wasm32"))]
fn rust_doc_citation_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"((?:[A-Za-z0-9_.-]+/)*[A-Za-z0-9_.-]+\.md)\b")
            .expect("compiled Rust doc Markdown citation regex is valid")
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
