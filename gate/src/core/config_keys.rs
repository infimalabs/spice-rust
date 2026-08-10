//! Layer-aware admission of captured configuration keys.

use std::fmt;

use toml::Value;
use wire::ConfigSource;

use super::{ConfigKeyEvidence, CoreEvidence, CoreFinding, CoreInputError, CoreObservation};
use crate::input::Contract;

const TRACKED: &[ConfigSource] = &[ConfigSource::Tracked];
const TRACKED_LOCAL: &[ConfigSource] = &[ConfigSource::Tracked, ConfigSource::Local];
const TRACKED_FLAG: &[ConfigSource] = &[ConfigSource::Tracked, ConfigSource::Flag];
const LOCAL: &[ConfigSource] = &[ConfigSource::Local];

/// One packaged configuration key and the authored layers that may supply it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfigKeyDeclaration {
    template: &'static str,
    layers: &'static [ConfigSource],
}

impl ConfigKeyDeclaration {
    const fn new(template: &'static str, layers: &'static [ConfigSource]) -> Self {
        Self { template, layers }
    }

    /// Fixed or dynamic key path, using `<name>` and `[]` placeholders.
    #[must_use]
    pub const fn template(self) -> &'static str {
        self.template
    }

    /// Authored layers admitted for this key in increasing precedence order.
    #[must_use]
    pub const fn layers(self) -> &'static [ConfigSource] {
        self.layers
    }
}

/// The current packaged configuration-key registry in canonical resolver order.
pub const CONFIG_KEY_DECLARATIONS: &[ConfigKeyDeclaration] = &[
    ConfigKeyDeclaration::new("tasks.stems", TRACKED),
    ConfigKeyDeclaration::new("tasks.hidden_stems", TRACKED),
    ConfigKeyDeclaration::new("tasks.project_min_depth", TRACKED),
    ConfigKeyDeclaration::new("tasks.project_max_depth", TRACKED),
    ConfigKeyDeclaration::new("judge.enabled", TRACKED_LOCAL),
    ConfigKeyDeclaration::new("policy.env_names", TRACKED),
    ConfigKeyDeclaration::new("policy.pre_commit_builtins", TRACKED),
    ConfigKeyDeclaration::new("policy.pre_commit[].label", TRACKED),
    ConfigKeyDeclaration::new("policy.pre_commit[].run", TRACKED),
    ConfigKeyDeclaration::new("policy.pre_commit[].scopes.paths", TRACKED),
    ConfigKeyDeclaration::new("policy.commit_message.allowed_trailers", TRACKED),
    ConfigKeyDeclaration::new("policy.commit_message.blocked_trailers", TRACKED),
    ConfigKeyDeclaration::new("policy.admission", TRACKED),
    ConfigKeyDeclaration::new("policy.repo_truth.docs", TRACKED),
    ConfigKeyDeclaration::new("policy.file_shape.source_suffixes", TRACKED),
    ConfigKeyDeclaration::new("policy.languages.complexity", TRACKED),
    ConfigKeyDeclaration::new("policy.languages.magic", TRACKED),
    ConfigKeyDeclaration::new("policy.package_roots", TRACKED),
    ConfigKeyDeclaration::new("policy.generated_paths", TRACKED),
    ConfigKeyDeclaration::new("policy.name_cluster_threshold", TRACKED),
    ConfigKeyDeclaration::new("policy.test_paths", TRACKED),
    ConfigKeyDeclaration::new("policy.package.boundary_underscore_pattern", TRACKED),
    ConfigKeyDeclaration::new("taste.<name>.words", TRACKED),
    ConfigKeyDeclaration::new("taste.<name>.message", TRACKED),
    ConfigKeyDeclaration::new("consent.posture", TRACKED),
    ConfigKeyDeclaration::new("consent.rule[].name", TRACKED),
    ConfigKeyDeclaration::new("consent.rule[].tool", TRACKED),
    ConfigKeyDeclaration::new("consent.rule[].answer", TRACKED),
    ConfigKeyDeclaration::new("consent.rule[].budget", TRACKED),
    ConfigKeyDeclaration::new("gate.walk.skip", TRACKED),
    ConfigKeyDeclaration::new("board.next.band", TRACKED),
    ConfigKeyDeclaration::new("lane.life", TRACKED_FLAG),
    ConfigKeyDeclaration::new("lane.lease", TRACKED),
    ConfigKeyDeclaration::new("lane.vendor", TRACKED_FLAG),
    ConfigKeyDeclaration::new("lane.deaths", TRACKED),
    ConfigKeyDeclaration::new("lane.young", TRACKED),
    ConfigKeyDeclaration::new("lane.hold", TRACKED),
    ConfigKeyDeclaration::new("steer.expiry", TRACKED),
    ConfigKeyDeclaration::new("steer.resend", TRACKED),
    ConfigKeyDeclaration::new("vendor.claude.binary", TRACKED),
    ConfigKeyDeclaration::new("vendor.claude.model", TRACKED),
    ConfigKeyDeclaration::new("vendor.claude.effort", TRACKED),
    ConfigKeyDeclaration::new("vendor.claude.auto_compact_window_tokens", TRACKED),
    ConfigKeyDeclaration::new("vendor.codex.binary", TRACKED),
    ConfigKeyDeclaration::new("vendor.codex.model", TRACKED),
    ConfigKeyDeclaration::new("vendor.codex.effort", TRACKED),
    ConfigKeyDeclaration::new("vendor.codex.personality", TRACKED),
    ConfigKeyDeclaration::new("vendor.codex.fast_mode", TRACKED),
    ConfigKeyDeclaration::new("listen.window", TRACKED_LOCAL),
    ConfigKeyDeclaration::new("lock.busy", TRACKED_LOCAL),
    ConfigKeyDeclaration::new("lock.taken", TRACKED_LOCAL),
    ConfigKeyDeclaration::new("lock.full", TRACKED_LOCAL),
    ConfigKeyDeclaration::new("lock.<name>.path", TRACKED_LOCAL),
    ConfigKeyDeclaration::new("lock.<name>.directory", TRACKED_LOCAL),
    ConfigKeyDeclaration::new("lock.<name>.shards", TRACKED_LOCAL),
    ConfigKeyDeclaration::new("canvas.pack.rows", TRACKED_LOCAL),
    ConfigKeyDeclaration::new("say.backend", TRACKED_LOCAL),
    ConfigKeyDeclaration::new("say.command", LOCAL),
    ConfigKeyDeclaration::new("say.voice", TRACKED_LOCAL),
    ConfigKeyDeclaration::new("say.content_type", TRACKED_LOCAL),
    ConfigKeyDeclaration::new("say.words_per_minute", TRACKED_LOCAL),
    ConfigKeyDeclaration::new("proof.image", TRACKED),
    ConfigKeyDeclaration::new("proof.container", LOCAL),
    ConfigKeyDeclaration::new("proof.runner", LOCAL),
];

pub(super) fn config_keys(contract: &Contract) -> Result<CoreObservation, CoreInputError> {
    let mut findings = Vec::new();
    let Value::Table(table) = contract.tracked_configuration() else {
        return Err(CoreInputError::new(
            "tracked configuration root must be a TOML table",
        ));
    };
    let mut path = Vec::new();
    for (key, value) in table {
        path.push(Segment::Key(key));
        scan(value, &mut path, &mut findings)?;
        path.pop();
    }
    Ok(CoreObservation::Measured {
        evidence: CoreEvidence::ConfigKeyValidity(ConfigKeyEvidence::new(findings)),
    })
}

fn scan<'a>(
    value: &'a Value,
    path: &mut Vec<Segment<'a>>,
    findings: &mut Vec<CoreFinding>,
) -> Result<(), CoreInputError> {
    match disposition(path, ConfigSource::Tracked) {
        Disposition::Admitted => return Ok(()),
        Disposition::Forbidden(layers) => {
            let key = ConfigPath(path).to_string();
            let admitted = layers
                .iter()
                .map(|layer| layer.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            findings.push(CoreFinding::new(
                "forbidden-layer",
                None::<String>,
                format!(
                    "tracked layer key {key} is packaged for the {admitted} layer, not tracked"
                ),
            )?);
            return Ok(());
        }
        Disposition::Unknown => {
            let key = ConfigPath(path).to_string();
            findings.push(CoreFinding::new(
                "unknown-key",
                None::<String>,
                format!(
                    "tracked layer key {key} is absent from the packaged configuration registry"
                ),
            )?);
            return Ok(());
        }
        Disposition::Prefix => {}
    }

    match value {
        Value::Table(table) => {
            for (key, child) in table {
                path.push(Segment::Key(key));
                scan(child, path, findings)?;
                path.pop();
            }
        }
        Value::Array(values) => {
            for (index, child) in values.iter().enumerate() {
                path.push(Segment::Index(index));
                scan(child, path, findings)?;
                path.pop();
            }
        }
        _ => {}
    }
    Ok(())
}

enum Disposition {
    Admitted,
    Forbidden(&'static [ConfigSource]),
    Prefix,
    Unknown,
}

fn disposition(path: &[Segment<'_>], source: ConfigSource) -> Disposition {
    for declaration in CONFIG_KEY_DECLARATIONS {
        if template_covers(declaration.template, path, true) {
            return if declaration.layers.contains(&source) {
                Disposition::Admitted
            } else {
                Disposition::Forbidden(declaration.layers)
            };
        }
    }
    if CONFIG_KEY_DECLARATIONS
        .iter()
        .any(|declaration| template_covers(declaration.template, path, false))
    {
        Disposition::Prefix
    } else {
        Disposition::Unknown
    }
}

#[derive(Clone, Copy)]
enum Segment<'a> {
    Key(&'a str),
    Index(usize),
}

fn template_covers(template: &str, path: &[Segment<'_>], exact: bool) -> bool {
    let mut at = 0;
    for template_segment in template.split('.') {
        if at == path.len() {
            return !exact;
        }
        if template_segment == "<name>" {
            if !matches!(path.get(at), Some(Segment::Key(_))) {
                return false;
            }
            at += 1;
        } else if let Some(key) = template_segment.strip_suffix("[]") {
            if !matches!(path.get(at), Some(Segment::Key(actual)) if *actual == key) {
                return false;
            }
            at += 1;
            if at == path.len() {
                return !exact;
            }
            if !matches!(path.get(at), Some(Segment::Index(_))) {
                return false;
            }
            at += 1;
        } else {
            if !matches!(path.get(at), Some(Segment::Key(actual)) if *actual == template_segment) {
                return false;
            }
            at += 1;
        }
    }
    at == path.len()
}

struct ConfigPath<'a>(&'a [Segment<'a>]);

impl fmt::Display for ConfigPath<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for segment in self.0 {
            match segment {
                Segment::Key(key) => {
                    if !first {
                        formatter.write_str(".")?;
                    }
                    if !key.is_empty()
                        && key
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                    {
                        formatter.write_str(key)?;
                    } else {
                        write!(formatter, "{key:?}")?;
                    }
                }
                Segment::Index(index) => write!(formatter, "[{index}]")?,
            }
            first = false;
        }
        Ok(())
    }
}
