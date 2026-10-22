//! Parser for the tracked repository policy document.

use std::collections::BTreeMap;
use std::fmt;

use toml::Value;

use super::matcher::normalize_path;
use super::{
    BoundName, CompileError, FileClasses, OneLetterBehavior, Policy, PolicyDocument,
    RuleDeclaration, RuleId, RuleSettings, RuleSource, RuleSourceKind, ScopeSelector,
};

const DEFAULT_REPO_TRUTH_DOCS: &[&str] = &["AGENTS.md"];
const MARKDOWN_EXTENSION: &str = ".md";
const MARKDOWN_BASE_CHARS: u64 = 10_000;
const MARKDOWN_MAX_BOUNDED_CHARS: u64 = 30_000;

/// Source location every packaged default is refused at.
///
/// A location rather than a path: these bytes ship with the release and no
/// repository has a file at this name, which is the point. The refusal renders
/// as `{source}:{field}`, so an operator reads a sentence that cannot be
/// mistaken for one naming something they can edit.
const PACKAGED_POLICY_SOURCE: &str = "gate packaged policy defaults";

/// Field a repository's own document inventory is refused under.
const DECLARED_REPO_TRUTH_FIELD: &str = "policy.repo_truth.docs";

/// Field the packaged document inventory is refused under.
///
/// The constant's own name rather than a policy field path, because there is no
/// field: an operator reading this refusal is being sent to this release's
/// source, not to anything their repository can spell.
const PACKAGED_REPO_TRUTH_FIELD: &str = "DEFAULT_REPO_TRUTH_DOCS";

/// Parse the tracked policy document into the sole compiled representation.
///
/// The document contributes its repository-truth inventory and file classes;
/// limits, ratchet, magic threshold, and Markdown depth budgets are packaged.
/// Every path and stem matcher compiles before this returns, so a policy the
/// gate holds is one it can evaluate.
///
/// # Errors
///
/// Returns the exact tracked field whose type or selector has no exact
/// compiled meaning.
pub fn parse_tracked_policy(source: impl Into<String>, text: &str) -> Result<Policy, ParseError> {
    let source = source.into();
    if source.is_empty() || source.chars().any(char::is_control) {
        return Err(ParseError::new(
            source,
            "<source>",
            "source location must be nonempty text without control characters",
        ));
    }
    let value = toml::from_str::<Value>(text).map_err(|error| {
        ParseError::new(&source, "<document>", format!("invalid TOML: {error}"))
    })?;
    let root = value
        .as_table()
        .ok_or_else(|| ParseError::new(&source, "<document>", "TOML root must be a table"))?;
    let policy = optional_table(root, "policy", "policy", &source)?;
    let document = PolicyDocument::tracked(
        parse_repo_truth_docs(policy, &source)?,
        parse_file_classes(policy, &source)?,
        markdown_depth_rules(&source),
    );
    Policy::compile(document).map_err(|error| compile_error(&source, &error))
}

/// Resolve the repository-document inventory one policy layer declares.
///
/// A layer that declares nothing takes the packaged inventory, and a layer that
/// declares documents names the whole set it wants measured. Normalization
/// happens here so a compiled policy can never carry a spelling this refusal
/// would have rejected, and so the refusal names the declaring field rather
/// than the compiled document it would have become.
///
/// The two halves are refused separately because they have different authors. A
/// declared path is a repository's spelling and names the field that spelled it;
/// a packaged path is this release's own, and no repository can edit it or
/// repair it, so attributing one to a tracked field would send an operator to a
/// file whose bytes are not at fault. Nothing tracked reaches the packaged
/// branch, which is exactly why it must not borrow the tracked source.
///
/// The packaged inventory is a parameter rather than a constant read here, so
/// the branch that attributes it can be refused on demand. Reading
/// [`DEFAULT_REPO_TRUTH_DOCS`] directly would leave this branch's attribution
/// unprovable until the day a bad default shipped, which is the one day it has
/// to already be right — and a test that reached the packaged normalization by
/// some other door would prove the spelling without proving that this branch
/// goes through it.
///
/// # Errors
///
/// Returns the declared or packaged path that is not a repository-relative
/// spelling.
fn repo_truth_inventory(
    declared: Option<&[String]>,
    packaged: &[&str],
    source: &str,
) -> Result<Vec<String>, ParseError> {
    match declared {
        Some(declared) => normalize_inventory(
            declared.iter().map(String::as_str),
            source,
            DECLARED_REPO_TRUTH_FIELD,
        ),
        None => normalize_inventory(
            packaged.iter().copied(),
            PACKAGED_POLICY_SOURCE,
            PACKAGED_REPO_TRUTH_FIELD,
        ),
    }
}

/// Normalize every path in one inventory, attributing a refusal to its author.
fn normalize_inventory<'path>(
    paths: impl Iterator<Item = &'path str>,
    source: &str,
    field: &str,
) -> Result<Vec<String>, ParseError> {
    paths
        .map(|path| {
            normalize_path(path)
                .map_err(|reason| ParseError::new(source, field, format!("{path:?}: {reason}")))
        })
        .collect()
}

fn parse_repo_truth_docs(
    policy: Option<&toml::Table>,
    source: &str,
) -> Result<Vec<String>, ParseError> {
    let table = optional_nested(policy, "repo_truth", "policy.repo_truth", source)?;
    reject_unknown(table, &["docs"], "policy.repo_truth", source)?;
    let declared = table
        .and_then(|table| table.get("docs"))
        .map(|value| string_list(value, DECLARED_REPO_TRUTH_FIELD, source, true))
        .transpose()?;
    repo_truth_inventory(declared.as_deref(), DEFAULT_REPO_TRUTH_DOCS, source)
}

/// Resolve the three file classes the structural arms select their scopes from.
///
/// Every one is required, which is the opposite of the inventory above. A
/// repository that declares no truth documents still has a packaged answer,
/// but nothing packages an answer to which files this repository wants
/// measured: a selection defaulted here would measure a set nobody chose, and
/// an empty one would let a structural arm report a clean family over no files
/// at all. Both are indistinguishable from having passed.
fn parse_file_classes(
    policy: Option<&toml::Table>,
    source: &str,
) -> Result<FileClasses, ParseError> {
    let shape = optional_nested(policy, "file_shape", "policy.file_shape", source)?;
    reject_unknown(shape, &["source_suffixes"], "policy.file_shape", source)?;
    let languages = optional_nested(policy, "languages", "policy.languages", source)?;
    reject_unknown(
        languages,
        &["complexity", "magic"],
        "policy.languages",
        source,
    )?;
    Ok(FileClasses::tracked(
        suffix_list(
            shape,
            "source_suffixes",
            "policy.file_shape.source_suffixes",
            source,
        )?,
        suffix_list(
            languages,
            "complexity",
            "policy.languages.complexity",
            source,
        )?,
        suffix_list(languages, "magic", "policy.languages.magic", source)?,
    ))
}

/// One required selection, spelled the way the extension of a path is spelled.
///
/// The leading period is demanded rather than added, because a selection this
/// silently rewrote would no longer be the text the repository can read back
/// and predict its own measured set from.
fn suffix_list(
    table: Option<&toml::Table>,
    key: &str,
    field: &str,
    source: &str,
) -> Result<Vec<String>, ParseError> {
    let value = table.and_then(|table| table.get(key)).ok_or_else(|| {
        ParseError::new(
            source,
            field,
            "must be declared; no file-class selection is packaged",
        )
    })?;
    let suffixes = string_list(value, field, source, false)?;
    for (index, suffix) in suffixes.iter().enumerate() {
        if !suffix.starts_with('.') || suffix.len() == 1 {
            return Err(ParseError::new(
                source,
                format!("{field}[{}]", index + 1),
                format!("{suffix:?}: must be a file suffix beginning with a period"),
            ));
        }
    }
    Ok(suffixes)
}

/// The packaged Markdown depth budgets, one rule per directory depth.
///
/// Each bounded depth allows one more base budget than the depth above it, up
/// to the bounded maximum; the first depth past that is unlimited. Rule
/// identities name their generated field, so persisted bound state keeps its
/// keys.
fn markdown_depth_rules(source: &str) -> Vec<RuleDeclaration> {
    let bounded = MARKDOWN_MAX_BOUNDED_CHARS.div_ceil(MARKDOWN_BASE_CHARS);
    (0..bounded)
        .map(|depth| {
            let budget = (MARKDOWN_BASE_CHARS * (depth + 1)).min(MARKDOWN_MAX_BOUNDED_CHARS);
            RuleSettings::tracked(1.0, Some(budget), Some(budget), false, None)
        })
        .chain([RuleSettings::tracked(1.0, None, None, true, None)])
        .enumerate()
        .map(|(order, settings)| markdown_rule(source, order, settings))
        .collect()
}

fn markdown_rule(source: &str, depth: usize, settings: RuleSettings) -> RuleDeclaration {
    let field = format!("policy.markdown_depth_budget.generated[{MARKDOWN_EXTENSION}][{depth}]");
    let matcher = vec!["*"; depth + 1].join("/");
    RuleDeclaration {
        id: RuleId::legacy(source, &field),
        source: RuleSource::tracked(source, field, RuleSourceKind::GeneratedMarkdownDepth),
        source_order: depth,
        priority: 0,
        scopes: ScopeSelector::tracked(vec![matcher], vec![MARKDOWN_EXTENSION.to_owned()]),
        bounds: BTreeMap::from([(BoundName::RepoTruthDocChars, settings)]),
        magic: None,
        stem_pattern: None,
        one_letter: OneLetterBehavior::Exclude,
    }
}

fn optional_nested<'a>(
    parent: Option<&'a toml::Table>,
    key: &str,
    field: &str,
    source: &str,
) -> Result<Option<&'a toml::Table>, ParseError> {
    let Some(parent) = parent else {
        return Ok(None);
    };
    optional_table(parent, key, field, source)
}

fn optional_table<'a>(
    parent: &'a toml::Table,
    key: &str,
    field: &str,
    source: &str,
) -> Result<Option<&'a toml::Table>, ParseError> {
    parent
        .get(key)
        .map(|value| {
            value
                .as_table()
                .ok_or_else(|| ParseError::new(source, field, "must be a table"))
        })
        .transpose()
}

fn reject_unknown(
    table: Option<&toml::Table>,
    allowed: &[&str],
    field: &str,
    source: &str,
) -> Result<(), ParseError> {
    let Some(table) = table else {
        return Ok(());
    };
    if let Some(key) = table.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(ParseError::new(
            source,
            format!("{field}.{key}"),
            format!("unknown field; expected one of: {}", allowed.join(", ")),
        ));
    }
    Ok(())
}

fn string_list(
    value: &Value,
    field: &str,
    source: &str,
    allow_empty: bool,
) -> Result<Vec<String>, ParseError> {
    let values = value
        .as_array()
        .ok_or_else(|| ParseError::new(source, field, "must be a list of strings"))?;
    if values.is_empty() && !allow_empty {
        return Err(ParseError::new(
            source,
            field,
            "must be a non-empty list of non-empty strings",
        ));
    }
    let mut normalized = Vec::new();
    for (index, value) in values.iter().enumerate() {
        let item = value.as_str().ok_or_else(|| {
            ParseError::new(
                source,
                format!("{field}[{}]", index + 1),
                "must be a non-empty string",
            )
        })?;
        let item = item.trim();
        if item.is_empty() {
            return Err(ParseError::new(
                source,
                format!("{field}[{}]", index + 1),
                "must be a non-empty string",
            ));
        }
        if !normalized.iter().any(|existing| existing == item) {
            normalized.push(item.to_owned());
        }
    }
    Ok(normalized)
}

fn compile_error(source: &str, error: &CompileError) -> ParseError {
    ParseError::new(source, format!("policy.{}", error.field()), error.reason())
}

/// A tracked policy field that has no exact compiled meaning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    source: String,
    field: String,
    reason: String,
}

impl ParseError {
    fn new(source: impl Into<String>, field: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            field: field.into(),
            reason: reason.into(),
        }
    }

    /// Source document location.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Exact tracked field.
    #[must_use]
    pub fn field(&self) -> &str {
        &self.field
    }

    /// Why the field has no exact compiled meaning.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{} is refused by tracked policy parsing: {}",
            self.source, self.field, self.reason
        )
    }
}

impl std::error::Error for ParseError {}
