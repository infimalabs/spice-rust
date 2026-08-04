//! One-way translation of predecessor anonymous policy rows.

use std::collections::BTreeMap;
use std::fmt;

use toml::Value;

use super::matcher::{compile_stem_expression, normalize_path};
use super::{
    BoundLimits, BoundName, CompileError, GlobalRatchet, MagicRule, OneLetterBehavior, Policy,
    PolicyDocument, RuleDeclaration, RuleId, RuleSettings, RuleSource, RuleSourceKind,
    ScopeSelector, encode_id_component,
};

const RULE_SETTING_KEYS: [&str; 5] = ["multiplier", "min", "max", "unlimited", "flex"];
const DEFAULT_MAGIC_THRESHOLD: u64 = 10;
const DEFAULT_MARKDOWN_BASE: u64 = 10_000;
const DEFAULT_MARKDOWN_MAX: u64 = 30_000;
const DEFAULT_REPO_TRUTH_DOCS: &[&str] = &["AGENTS.md"];

/// Translate one predecessor policy document into the sole compiled current
/// representation.
///
/// This is intentionally not a deserializer used by [`Policy::compile`]. A
/// caller performs migration once, persists [`Policy::document`], and runtime
/// compilation thereafter accepts only named [`PolicyDocument`] rules.
///
/// # Errors
///
/// Returns the exact predecessor field whose type, selector, limit, or regular
/// expression has no exact current meaning.
pub fn migrate_predecessor_policy(
    source: impl Into<String>,
    text: &str,
) -> Result<Policy, MigrationError> {
    let source = source.into();
    if source.is_empty() || source.chars().any(char::is_control) {
        return Err(MigrationError::new(
            source,
            "<source>",
            "source location must be nonempty text without control characters",
        ));
    }
    let value = toml::from_str::<Value>(text).map_err(|error| {
        MigrationError::new(&source, "<document>", format!("invalid TOML: {error}"))
    })?;
    let root = value
        .as_table()
        .ok_or_else(|| MigrationError::new(&source, "<document>", "TOML root must be a table"))?;
    let policy = optional_table(root, "policy", "policy", &source)?;

    let limits = parse_limits(policy, &source)?;
    let ratchet = parse_legacy_ratchet(policy, &limits, &source)?;
    let magic_threshold = parse_magic(policy, &source)?;
    let repo_truth_docs = parse_repo_truth_docs(policy, &source)?;
    let markdown = parse_markdown(policy, &source)?;
    let mut rules = generated_markdown_rules(&source, &markdown)?;
    let generated = rules.len();
    rules.extend(parse_list_rules(policy, &source, generated)?);

    let document =
        PolicyDocument::predecessor(limits, ratchet, magic_threshold, repo_truth_docs, rules);
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
/// # Errors
///
/// Returns the declared path that is not a repository-relative spelling.
fn repo_truth_inventory(
    declared: Option<&[String]>,
    source: &str,
) -> Result<Vec<String>, MigrationError> {
    declared
        .map_or_else(
            || {
                DEFAULT_REPO_TRUTH_DOCS
                    .iter()
                    .map(|path| (*path).to_owned())
                    .collect()
            },
            <[String]>::to_vec,
        )
        .into_iter()
        .map(|path| {
            normalize_path(&path).map_err(|reason| {
                MigrationError::new(
                    source,
                    "policy.repo_truth.docs",
                    format!("{path:?}: {reason}"),
                )
            })
        })
        .collect()
}

fn parse_repo_truth_docs(
    policy: Option<&toml::Table>,
    source: &str,
) -> Result<Vec<String>, MigrationError> {
    let table = optional_nested(policy, "repo_truth", "policy.repo_truth", source)?;
    reject_unknown(table, &["docs"], "policy.repo_truth", source)?;
    let declared = table
        .and_then(|table| table.get("docs"))
        .map(|value| string_list(value, "policy.repo_truth.docs", source, true))
        .transpose()?;
    repo_truth_inventory(declared.as_deref(), source)
}

#[derive(Debug)]
struct MarkdownPolicy {
    extensions: Vec<String>,
    stem_pattern: Option<String>,
    base_chars: u64,
    max_bounded_chars: u64,
}

fn parse_limits(policy: Option<&toml::Table>, source: &str) -> Result<BoundLimits, MigrationError> {
    let table = optional_nested(policy, "limits", "policy.limits", source)?;
    reject_unknown(
        table,
        &BoundName::ALL.map(BoundName::as_str),
        "policy.limits",
        source,
    )?;
    let mut limits = BoundLimits::PREDECESSOR;
    for bound in BoundName::ALL {
        if let Some(value) = table.and_then(|table| table.get(bound.as_str())) {
            limits.set(
                bound,
                positive_integer(value, &format!("policy.limits.{bound}"), source)?,
            );
        }
    }
    Ok(limits)
}

fn parse_legacy_ratchet(
    policy: Option<&toml::Table>,
    limits: &BoundLimits,
    source: &str,
) -> Result<GlobalRatchet, MigrationError> {
    const FLEX_KEYS: [&str; 6] = [
        "ratio",
        "jitter_percent",
        "file_loc",
        "file_bytes",
        "routine_ccn",
        "routine_length",
    ];
    let table = optional_nested(policy, "flex", "policy.flex", source)?;
    reject_unknown(table, &FLEX_KEYS, "policy.flex", source)?;
    let mut ratchet = GlobalRatchet::PREDECESSOR;
    if let Some(value) = table.and_then(|table| table.get("ratio")) {
        ratchet.set_ratio(ratio(value, "policy.flex.ratio", source)?);
    }
    if let Some(value) = table.and_then(|table| table.get("jitter_percent")) {
        ratchet.set_variation_percent(non_negative_integer(
            value,
            "policy.flex.jitter_percent",
            source,
        )?);
    }
    for bound in [
        BoundName::FileLoc,
        BoundName::FileBytes,
        BoundName::RoutineCcn,
        BoundName::RoutineLength,
    ] {
        if let Some(value) = table.and_then(|table| table.get(bound.as_str())) {
            let value = positive_integer(value, &format!("policy.flex.{bound}"), source)?;
            if value < limits.get(bound) {
                return Err(MigrationError::new(
                    source,
                    format!("policy.flex.{bound}"),
                    format!("must be >= policy.limits.{bound}"),
                ));
            }
            ratchet.set(bound, value);
        }
    }
    Ok(ratchet)
}

fn parse_magic(policy: Option<&toml::Table>, source: &str) -> Result<u64, MigrationError> {
    let table = optional_nested(policy, "magic", "policy.magic", source)?;
    reject_unknown(table, &["examine_threshold"], "policy.magic", source)?;
    let Some(value) = table.and_then(|table| table.get("examine_threshold")) else {
        return Ok(DEFAULT_MAGIC_THRESHOLD);
    };
    positive_integer(value, "policy.magic.examine_threshold", source)
}

fn parse_markdown(
    policy: Option<&toml::Table>,
    source: &str,
) -> Result<MarkdownPolicy, MigrationError> {
    const BUDGET_KEYS: [&str; 2] = ["extensions", "stem_pattern"];
    const DEPTH_KEYS: [&str; 2] = ["base_chars", "max_bounded_chars"];
    let budget = optional_nested(
        policy,
        "markdown_depth_budget",
        "policy.markdown_depth_budget",
        source,
    )?;
    reject_unknown(budget, &BUDGET_KEYS, "policy.markdown_depth_budget", source)?;
    let extensions = match budget.and_then(|table| table.get("extensions")) {
        Some(value) => string_list(
            value,
            "policy.markdown_depth_budget.extensions",
            source,
            true,
        )?,
        None => vec![".md".to_owned()],
    };
    for (index, extension) in extensions.iter().enumerate() {
        if !extension.is_ascii()
            || extension.len() < 2
            || !extension.starts_with('.')
            || extension.contains(['/', '\\'])
            || extension[1..].contains('.')
        {
            return Err(MigrationError::new(
                source,
                format!("policy.markdown_depth_budget.extensions[{}]", index + 1),
                format!(
                    "suffix {extension:?} has no exact current meaning; \
                     use one ASCII final suffix"
                ),
            ));
        }
    }
    let stem_pattern = budget
        .and_then(|table| table.get("stem_pattern"))
        .map(|value| nonempty_string(value, "policy.markdown_depth_budget.stem_pattern", source))
        .transpose()?;
    if let Some(pattern) = &stem_pattern {
        compile_stem_expression(pattern).map_err(|reason| {
            MigrationError::new(source, "policy.markdown_depth_budget.stem_pattern", reason)
        })?;
    }

    let depth = optional_nested(policy, "markdown_depth", "policy.markdown_depth", source)?;
    reject_unknown(depth, &DEPTH_KEYS, "policy.markdown_depth", source)?;
    let base_chars = depth
        .and_then(|table| table.get("base_chars"))
        .map(|value| positive_integer(value, "policy.markdown_depth.base_chars", source))
        .transpose()?
        .unwrap_or(DEFAULT_MARKDOWN_BASE);
    let max_bounded_chars = depth
        .and_then(|table| table.get("max_bounded_chars"))
        .map(|value| positive_integer(value, "policy.markdown_depth.max_bounded_chars", source))
        .transpose()?
        .unwrap_or(DEFAULT_MARKDOWN_MAX);
    if max_bounded_chars < base_chars {
        return Err(MigrationError::new(
            source,
            "policy.markdown_depth.max_bounded_chars",
            "must be greater than or equal to policy.markdown_depth.base_chars",
        ));
    }
    Ok(MarkdownPolicy {
        extensions,
        stem_pattern,
        base_chars,
        max_bounded_chars,
    })
}

fn generated_markdown_rules(
    source: &str,
    markdown: &MarkdownPolicy,
) -> Result<Vec<RuleDeclaration>, MigrationError> {
    if markdown.extensions.is_empty() {
        return Ok(Vec::new());
    }
    let bounded_count = markdown
        .max_bounded_chars
        .checked_add(markdown.base_chars - 1)
        .ok_or_else(|| {
            MigrationError::new(
                source,
                "policy.markdown_depth.max_bounded_chars",
                "depth calculation exceeds u64",
            )
        })?
        / markdown.base_chars;
    let mut rules = Vec::new();
    for extension in &markdown.extensions {
        for depth in 0..bounded_count {
            let budget = markdown
                .base_chars
                .checked_mul(depth + 1)
                .map(|value| value.min(markdown.max_bounded_chars))
                .ok_or_else(|| {
                    MigrationError::new(
                        source,
                        "policy.markdown_depth.base_chars",
                        "depth budget exceeds u64",
                    )
                })?;
            rules.push(markdown_rule(
                source,
                extension,
                depth,
                rules.len(),
                markdown,
                RuleSettings::predecessor(1.0, Some(budget), Some(budget), false, None),
            ));
        }
        rules.push(markdown_rule(
            source,
            extension,
            bounded_count,
            rules.len(),
            markdown,
            RuleSettings::predecessor(1.0, None, None, true, None),
        ));
    }
    Ok(rules)
}

fn markdown_rule(
    source: &str,
    extension: &str,
    depth: u64,
    source_order: usize,
    markdown: &MarkdownPolicy,
    settings: RuleSettings,
) -> RuleDeclaration {
    let field = format!(
        "policy.markdown_depth_budget.generated[{}][{depth}]",
        extension
    );
    let matcher = std::iter::repeat_n("*", usize::try_from(depth + 1).unwrap_or(usize::MAX))
        .collect::<Vec<_>>()
        .join("/");
    let mut bounds = BTreeMap::new();
    bounds.insert(BoundName::RepoTruthDocChars, settings);
    RuleDeclaration {
        id: RuleId::legacy(source, &field),
        source: RuleSource::predecessor(source, field, RuleSourceKind::GeneratedMarkdownDepth),
        source_order,
        priority: 0,
        scopes: ScopeSelector::predecessor(vec![matcher], vec![extension.to_owned()]),
        bounds,
        magic: None,
        stem_pattern: markdown.stem_pattern.clone(),
        one_letter: OneLetterBehavior::Exclude,
    }
}

fn parse_list_rules(
    policy: Option<&toml::Table>,
    source: &str,
    source_order: usize,
) -> Result<Vec<RuleDeclaration>, MigrationError> {
    let Some(raw_rules) = policy.and_then(|table| table.get("rules")) else {
        return Ok(Vec::new());
    };
    let rows = raw_rules.as_array().ok_or_else(|| {
        MigrationError::new(source, "policy.rules", "must be a list of rule tables")
    })?;
    rows.iter()
        .enumerate()
        .map(|(offset, value)| parse_list_rule(value, source, offset + 1, source_order + offset))
        .collect()
}

fn parse_list_rule(
    value: &Value,
    source: &str,
    legacy_index: usize,
    source_order: usize,
) -> Result<RuleDeclaration, MigrationError> {
    let field = format!("policy.rules[{legacy_index}]");
    let table = value
        .as_table()
        .ok_or_else(|| MigrationError::new(source, &field, "must be a rule table"))?;
    let scopes = parse_legacy_scopes(table.get("scopes"), source, &field)?;
    let bounds = parse_rule_bounds(table, source, &field)?;
    let magic = parse_rule_magic(table.get("magic"), source, &field)?;
    Ok(RuleDeclaration {
        id: RuleId::legacy(source, &field),
        source: RuleSource::predecessor(source, field, RuleSourceKind::PredecessorList),
        source_order,
        priority: 1,
        scopes,
        bounds,
        magic,
        stem_pattern: None,
        one_letter: OneLetterBehavior::Include,
    })
}

fn parse_legacy_scopes(
    value: Option<&Value>,
    source: &str,
    rule_field: &str,
) -> Result<ScopeSelector, MigrationError> {
    let Some(value) = value else {
        return Ok(ScopeSelector::default());
    };
    let field = format!("{rule_field}.scopes");
    let table = value
        .as_table()
        .ok_or_else(|| MigrationError::new(source, &field, "must be an inline table"))?;
    for key in table.keys() {
        if !matches!(key.as_str(), "paths" | "extensions") {
            return Err(MigrationError::new(
                source,
                format!("{field}.{key}"),
                concat!(
                    "predecessor policy rules support only paths and extensions; ",
                    "author this dimension in the named current schema"
                ),
            ));
        }
    }
    let paths = table
        .get("paths")
        .map(|value| string_list(value, &format!("{field}.paths"), source, false))
        .transpose()?
        .unwrap_or_default();
    let extensions = table
        .get("extensions")
        .map(|value| string_list(value, &format!("{field}.extensions"), source, false))
        .transpose()?
        .unwrap_or_default();
    Ok(ScopeSelector::predecessor(paths, extensions))
}

fn parse_rule_bounds(
    table: &toml::Table,
    source: &str,
    field: &str,
) -> Result<BTreeMap<BoundName, RuleSettings>, MigrationError> {
    let mut allowed = vec!["scopes", "magic"];
    allowed.extend(RULE_SETTING_KEYS);
    allowed.extend(BoundName::ALL.map(BoundName::as_str));
    reject_unknown(Some(table), &allowed, field, source)?;

    let mut bounds = BTreeMap::new();
    if RULE_SETTING_KEYS.iter().any(|key| table.contains_key(*key)) {
        let settings = parse_rule_settings(table, source, field, false)?;
        for bound in BoundName::ALL {
            bounds.insert(bound, settings.clone());
        }
    }
    for bound in BoundName::ALL {
        let Some(value) = table.get(bound.as_str()) else {
            continue;
        };
        let bound_field = format!("{field}.{bound}");
        let settings_table = value
            .as_table()
            .ok_or_else(|| MigrationError::new(source, &bound_field, "must be a settings table"))?;
        bounds.insert(
            bound,
            parse_rule_settings(settings_table, source, &bound_field, true)?,
        );
    }
    Ok(bounds)
}

fn parse_rule_settings(
    table: &toml::Table,
    source: &str,
    field: &str,
    reject_unrelated: bool,
) -> Result<RuleSettings, MigrationError> {
    if reject_unrelated {
        reject_unknown(Some(table), &RULE_SETTING_KEYS, field, source)?;
    }
    let multiplier = table
        .get("multiplier")
        .map(|value| non_negative_number(value, &format!("{field}.multiplier"), source))
        .transpose()?
        .unwrap_or(1.0);
    let minimum = table
        .get("min")
        .map(|value| positive_integer(value, &format!("{field}.min"), source))
        .transpose()?;
    let maximum = table
        .get("max")
        .map(|value| positive_integer(value, &format!("{field}.max"), source))
        .transpose()?;
    if matches!((minimum, maximum), (Some(min), Some(max)) if min > max) {
        return Err(MigrationError::new(source, field, "min must be <= max"));
    }
    let unlimited = table
        .get("unlimited")
        .map(|value| {
            value.as_bool().ok_or_else(|| {
                MigrationError::new(
                    source,
                    format!("{field}.unlimited"),
                    "must be true or false",
                )
            })
        })
        .transpose()?
        .unwrap_or(false);
    let ratchet = table
        .get("flex")
        .map(|value| ratio(value, &format!("{field}.flex"), source))
        .transpose()?;
    Ok(RuleSettings::predecessor(
        multiplier, minimum, maximum, unlimited, ratchet,
    ))
}

fn parse_rule_magic(
    value: Option<&Value>,
    source: &str,
    rule_field: &str,
) -> Result<Option<MagicRule>, MigrationError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let field = format!("{rule_field}.magic");
    let table = value
        .as_table()
        .ok_or_else(|| MigrationError::new(source, &field, "must be a table"))?;
    reject_unknown(Some(table), &["examine_threshold"], &field, source)?;
    let threshold = table.get("examine_threshold").ok_or_else(|| {
        MigrationError::new(
            source,
            format!("{field}.examine_threshold"),
            "must be a positive integer",
        )
    })?;
    Ok(Some(MagicRule::predecessor(positive_integer(
        threshold,
        &format!("{field}.examine_threshold"),
        source,
    )?)))
}

fn optional_nested<'a>(
    parent: Option<&'a toml::Table>,
    key: &str,
    field: &str,
    source: &str,
) -> Result<Option<&'a toml::Table>, MigrationError> {
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
) -> Result<Option<&'a toml::Table>, MigrationError> {
    parent
        .get(key)
        .map(|value| {
            value
                .as_table()
                .ok_or_else(|| MigrationError::new(source, field, "must be a table"))
        })
        .transpose()
}

fn reject_unknown(
    table: Option<&toml::Table>,
    allowed: &[&str],
    field: &str,
    source: &str,
) -> Result<(), MigrationError> {
    let Some(table) = table else {
        return Ok(());
    };
    if let Some(key) = table.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(MigrationError::new(
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
) -> Result<Vec<String>, MigrationError> {
    let values = value
        .as_array()
        .ok_or_else(|| MigrationError::new(source, field, "must be a list of strings"))?;
    if values.is_empty() && !allow_empty {
        return Err(MigrationError::new(
            source,
            field,
            "must be a non-empty list of non-empty strings",
        ));
    }
    let mut normalized = Vec::new();
    for (index, value) in values.iter().enumerate() {
        let item = value.as_str().ok_or_else(|| {
            MigrationError::new(
                source,
                format!("{field}[{}]", index + 1),
                "must be a non-empty string",
            )
        })?;
        let item = item.trim();
        if item.is_empty() {
            return Err(MigrationError::new(
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

fn nonempty_string(value: &Value, field: &str, source: &str) -> Result<String, MigrationError> {
    let value = value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| MigrationError::new(source, field, "must be a non-empty string"))?;
    Ok(value.to_owned())
}

fn positive_integer(value: &Value, field: &str, source: &str) -> Result<u64, MigrationError> {
    let value = value
        .as_integer()
        .ok_or_else(|| MigrationError::new(source, field, "must be a positive integer"))?;
    u64::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| MigrationError::new(source, field, "must be a positive integer"))
}

fn non_negative_integer(value: &Value, field: &str, source: &str) -> Result<u64, MigrationError> {
    value
        .as_integer()
        .and_then(|value| u64::try_from(value).ok())
        .ok_or_else(|| MigrationError::new(source, field, "must be a non-negative integer"))
}

fn non_negative_number(value: &Value, field: &str, source: &str) -> Result<f64, MigrationError> {
    number(value)
        .filter(|value| value.is_finite() && *value >= 0.0)
        .ok_or_else(|| MigrationError::new(source, field, "must be a finite number >= 0.0"))
}

fn ratio(value: &Value, field: &str, source: &str) -> Result<f64, MigrationError> {
    number(value)
        .filter(|value| value.is_finite() && *value >= 1.0)
        .ok_or_else(|| MigrationError::new(source, field, "must be a finite number >= 1.0"))
}

fn number(value: &Value) -> Option<f64> {
    value
        .as_float()
        .or_else(|| value.as_integer().map(|integer| integer as f64))
}

fn compile_error(source: &str, error: &CompileError) -> MigrationError {
    let field = error.field();
    let prefix = format!("legacy-rule:{}:", encode_id_component(source));
    let legacy_field = if let Some((id, suffix)) = field
        .strip_prefix("rule[")
        .and_then(|rest| rest.split_once(']'))
    {
        let encoded = id.strip_prefix(&prefix).unwrap_or(id);
        let decoded = decode_id_component(encoded).unwrap_or_else(|| encoded.to_owned());
        if decoded.starts_with("policy.markdown_depth_budget.generated")
            && suffix == ".stem_pattern"
        {
            "policy.markdown_depth_budget.stem_pattern".to_owned()
        } else {
            format!("{decoded}{suffix}")
        }
    } else {
        match field {
            "magic_examine_threshold" => "policy.magic.examine_threshold".to_owned(),
            _ => format!("policy.{field}"),
        }
    };
    MigrationError::new(source, legacy_field, error.reason())
}

fn decode_id_component(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let pair = std::str::from_utf8(bytes.get(index + 1..index + 3)?).ok()?;
            decoded.push(u8::from_str_radix(pair, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

/// A predecessor field that cannot be translated exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationError {
    source: String,
    field: String,
    reason: String,
}

impl MigrationError {
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

    /// Exact predecessor field.
    #[must_use]
    pub fn field(&self) -> &str {
        &self.field
    }

    /// Why migration would change meaning.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for MigrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{} is refused during predecessor policy migration: {}",
            self.source, self.field, self.reason
        )
    }
}

impl std::error::Error for MigrationError {}
