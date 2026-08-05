use std::collections::BTreeSet;
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;

use super::{ParseError, canonical_path, python_splitlines};

const SUPPORTED_SUFFIXES: &[&str] = &[
    ".c", ".cc", ".cpp", ".go", ".h", ".hpp", ".java", ".kt", ".m", ".mm", ".rs", ".scala",
    ".swift", ".ts",
];
const PREDECESSOR_PARSER_SUFFIXES: &[&str] = &[".py", ".cs", ".js"];

/// One new comparison-adjacent literal absent from the baseline file.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MagicFinding {
    path: String,
    line: u64,
    literal: String,
}

impl MagicFinding {
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    #[must_use]
    pub const fn line(&self) -> u64 {
        self.line
    }

    #[must_use]
    pub fn literal(&self) -> &str {
        &self.literal
    }
}

/// Detect predecessor-equivalent C-grammar magic-number regressions.
///
/// Python, C#, and JavaScript used AST/tree-sitter parent classifications in
/// release 0.31.0. They are refused by field until an exact native parser is
/// compiled; substituting this comparison scanner would silently weaken them.
pub fn scan_magic_regressions(
    path: &str,
    current: &str,
    baseline: Option<&str>,
    examine_threshold: u64,
) -> Result<Vec<MagicFinding>, ParseError> {
    let path = canonical_path(path)?;
    if examine_threshold == 0 {
        return Err(ParseError::malformed(
            "magic.examine_threshold",
            "threshold must be positive",
        ));
    }
    admit_suffix(&path)?;
    let baseline_literals = baseline
        .map(|text| scan_literals(&path, text, examine_threshold))
        .transpose()?
        .unwrap_or_default()
        .into_iter()
        .map(|finding| finding.literal)
        .collect::<BTreeSet<_>>();
    let current = scan_literals(&path, current, examine_threshold)?;
    Ok(current
        .into_iter()
        .filter(|finding| !baseline_literals.contains(&finding.literal))
        .collect())
}

/// Whether this build can read the magic-number class a path belongs to.
///
/// The scanner asks this on the way in, and the acquiring arm asks it before it
/// asks anything else about a path, so both settle a class the same way. It is
/// separate from scanning because it needs neither side of the comparison: which
/// classes are readable is a property of the compiled parser, answerable for a
/// path holding no bytes this build can reach.
///
/// # Errors
///
/// Returns the predecessor-owned refusal for a class released 0.31.0 parsed by
/// AST parent semantics, and the inapplicability for a path in no declared magic
/// class at all. A path this parser reads exactly returns nothing.
pub fn admit_magic_source(path: &str) -> Result<(), ParseError> {
    admit_suffix(&canonical_path(path)?)
}

fn admit_suffix(path: &str) -> Result<(), ParseError> {
    let suffix = Path::new(path)
        .extension()
        .map(|value| format!(".{}", value.to_string_lossy().to_ascii_lowercase()))
        .unwrap_or_default();
    if SUPPORTED_SUFFIXES.contains(&suffix.as_str()) {
        return Ok(());
    }
    if PREDECESSOR_PARSER_SUFFIXES.contains(&suffix.as_str()) {
        return Err(ParseError::unsupported(
            "policy.languages.magic",
            format!(
                "source class {suffix:?} requires predecessor AST/tree-sitter parent semantics"
            ),
            "compile an exact native parser for this source class or remove it from magic scope",
        ));
    }
    Err(ParseError::NotApplicable {
        reason: format!("path {path:?} has no declared magic-number source class"),
    })
}

fn scan_literals(path: &str, text: &str, threshold: u64) -> Result<Vec<MagicFinding>, ParseError> {
    let mut findings = Vec::new();
    let mut in_template = false;
    for (offset, raw_line) in python_splitlines(text).into_iter().enumerate() {
        let (line, template) = strip_template_literals(raw_line, in_template);
        in_template = template;
        let line = strip_comments(&line);
        findings.extend(line_findings(
            path,
            u64::try_from(offset + 1).expect("line number fits in u64"),
            &line,
            threshold,
        )?);
    }
    findings.sort();
    Ok(findings)
}

fn line_findings(
    path: &str,
    line_number: u64,
    line: &str,
    threshold: u64,
) -> Result<Vec<MagicFinding>, ParseError> {
    let mut candidates = Vec::new();
    for number in number_regex().find_iter(line) {
        if number_end_admitted(line, number.end())
            && let Some(operator) = comparison_before(line, number.start())
        {
            candidates.push((operator, number.end(), number.as_str()));
        }
        if number_start_admitted(line, number.start())
            && let Some(operator_end) = comparison_after(line, number.end())
        {
            candidates.push((number.start(), operator_end, number.as_str()));
        }
    }
    candidates.sort_by_key(|candidate| (candidate.0, candidate.1));
    let mut findings = Vec::new();
    let mut cursor = 0;
    for (start, end, candidate) in candidates {
        if start < cursor {
            continue;
        }
        cursor = end;
        let literal = candidate.replace('_', "");
        let value = literal.parse::<f64>().map_err(|_| {
            ParseError::malformed(
                format!("{path}:{line_number}"),
                format!("numeric token {literal:?} is not finite numeric text"),
            )
        })?;
        if value.is_finite() && value.abs() >= threshold as f64 {
            findings.push(MagicFinding {
                path: path.to_owned(),
                line: line_number,
                literal,
            });
        }
    }
    Ok(findings)
}

fn number_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"-?\d[\d_]*(?:\.\d+)?").expect("compiled numeric-token regex is valid")
    })
}

fn number_start_admitted(line: &str, start: usize) -> bool {
    let before = line[..start].chars().next_back();
    !before.is_some_and(|value| value == '.' || value == '_' || value.is_alphanumeric())
}

fn number_end_admitted(line: &str, end: usize) -> bool {
    let after = line[end..].chars().next();
    !after.is_some_and(|value| value == '.' || value == '_' || value.is_alphanumeric())
}

fn comparison_before(line: &str, number_start: usize) -> Option<usize> {
    let prefix = line[..number_start].trim_end();
    for operator in ["===", "!==", "<=", ">=", "==", "!=", "<", ">"] {
        let Some(start) = prefix.len().checked_sub(operator.len()) else {
            continue;
        };
        if &prefix[start..] != operator {
            continue;
        }
        let before = prefix[..start].chars().next_back();
        if before.is_none_or(|value| !matches!(value, '=' | '<' | '>' | '!')) {
            return Some(start);
        }
    }
    None
}

fn comparison_after(line: &str, number_end: usize) -> Option<usize> {
    let suffix = &line[number_end..];
    let leading = suffix.len() - suffix.trim_start().len();
    let suffix = &suffix[leading..];
    for operator in ["===", "!==", "<=", ">=", "==", "!=", "<", ">"] {
        if !suffix.starts_with(operator) {
            continue;
        }
        let after = suffix[operator.len()..].chars().next();
        let compound = matches!(operator, "<" | ">")
            && after.is_some_and(|value| matches!(value, '<' | '>' | '='));
        if !compound {
            return Some(number_end + leading + operator.len());
        }
    }
    None
}

fn strip_template_literals(line: &str, mut in_template: bool) -> (String, bool) {
    let mut kept = String::new();
    for character in line.chars() {
        if character == '`' {
            in_template = !in_template;
        } else if !in_template {
            kept.push(character);
        }
    }
    (kept, in_template)
}

fn strip_comments(line: &str) -> String {
    let mut kept = String::new();
    let mut remainder = line;
    loop {
        let slash = remainder.find("//");
        let block = remainder.find("/*");
        match (slash, block) {
            (None, None) => {
                kept.push_str(remainder);
                break;
            }
            (Some(slash), None) => {
                kept.push_str(&remainder[..slash]);
                break;
            }
            (Some(slash), Some(block)) if slash < block => {
                kept.push_str(&remainder[..slash]);
                break;
            }
            (_, Some(block)) => {
                kept.push_str(&remainder[..block]);
                let after = &remainder[block + 2..];
                if let Some(end) = after.find("*/") {
                    remainder = &after[end + 2..];
                } else {
                    kept.push_str(&remainder[block..]);
                    break;
                }
            }
        }
    }
    kept
}
