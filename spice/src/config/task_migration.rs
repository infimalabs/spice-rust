//! One-shot translation of an explicit release-0.31.0 task export.
//!
//! Ordinary configuration admission never calls this module. The translator
//! accepts one closed JSON schema, emits the sole current [`TaskMetadata`]
//! shape, and refuses every raw Taskwarrior override before execution.
//!
//! # The only cross-system bridge
//!
//! This is the sole bridge from foreign state into this runtime, and the
//! ownership rules in `crate::repository` bind it:
//!
//! - **Operator trigger.** Refused by default. Nothing invokes this module on
//!   its own; it runs only when an operator types `spice config migrate-tasks`
//!   and names an export path, and it reads only the file so named. It never
//!   discovers, opens, or converts the predecessor's live databases.
//! - **Version mapping.** [`PREDECESSOR_TASK_RELEASE`] names the one source
//!   release and [`TASK_MIGRATION_SCHEMA_VERSION`] the one export schema.
//!   Anything else refuses rather than being interpreted generously.
//! - **Identity mapping.** Every emitted value carries a
//!   [`MigrationProvenance`] naming the source file, the exact predecessor
//!   field path, its list index, and the team it came from, so each output
//!   traces back to the byte that produced it.
//! - **Verified backup.** None is required, and that is a property rather than
//!   an omission: the translation reads bytes and returns a report, so there is
//!   no prior state to preserve.
//! - **Failure boundary.** The whole export is validated before anything is
//!   emitted. A refusal is typed, names its witness, and is total -- there is no
//!   partially translated result to reason about.
//! - **Reversal.** Discarding the report reverses it completely, because
//!   nothing durable was written. Applying the report is a separate, deliberate
//!   act by the operator and is not part of this bridge.
//!
//! A bridge that ever needs to write would not qualify under any of the five
//! above as they stand, and would have to re-specify all of them first.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use super::TaskMetadata;
use serde::Serialize;

mod input;

/// Release owning the predecessor task export grammar.
pub const PREDECESSOR_TASK_RELEASE: &str = "0.31.0";
/// The only accepted task-export schema.
pub const TASK_MIGRATION_SCHEMA_VERSION: u64 = 1;

/// One raw predecessor override with a current typed semantic equivalent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RawOverrideMapping {
    pub predecessor: &'static str,
    pub current: &'static str,
}

/// No arbitrary Taskwarrior `rc.*` override has a current typed equivalent.
pub const RAW_OVERRIDE_MAPPINGS: &[RawOverrideMapping] = &[];

/// One already-acquired predecessor export and its exact source identity.
#[derive(Clone, Copy, Debug)]
pub struct PredecessorTaskMigration<'a> {
    source_file: &'a str,
    bytes: &'a [u8],
}

impl<'a> PredecessorTaskMigration<'a> {
    /// Bind explicit bytes to the source file named by every diagnostic.
    #[must_use]
    pub const fn new(source_file: &'a str, bytes: &'a [u8]) -> Self {
        Self { source_file, bytes }
    }
}

/// Stable source evidence for one migrated field or selected empty override.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MigrationProvenance {
    release: &'static str,
    source_kind: MigrationSource,
    decision: MigrationDecision,
    source_file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    team: Option<String>,
    field: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    list_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<String>,
}

/// Whether one migrated value came from release-owned defaults or export bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MigrationSource {
    Packaged,
    Export,
}

/// How one predecessor source contributed to the current representation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MigrationDecision {
    Packaged,
    Emitted,
    Deduplicated,
    SelectedEmpty,
}

impl MigrationProvenance {
    /// Release owning this source interpretation.
    #[must_use]
    pub const fn release(&self) -> &str {
        self.release
    }

    /// Whether the value came from packaged declarations or export bytes.
    #[must_use]
    pub const fn source_kind(&self) -> MigrationSource {
        self.source_kind
    }

    /// Exact resolution decision for this source value.
    #[must_use]
    pub const fn decision(&self) -> MigrationDecision {
        self.decision
    }

    /// Exact export file bound to this migration.
    #[must_use]
    pub fn source_file(&self) -> &str {
        &self.source_file
    }

    /// Team identity for one selected shell setting.
    #[must_use]
    pub fn team(&self) -> Option<&str> {
        self.team.as_deref()
    }

    /// Exact predecessor field path.
    #[must_use]
    pub fn field(&self) -> &str {
        &self.field
    }

    /// Source list index when the value came from a list member.
    #[must_use]
    pub const fn list_index(&self) -> Option<usize> {
        self.list_index
    }

    /// Current typed destination when this source emitted a value.
    #[must_use]
    pub fn target(&self) -> Option<&str> {
        self.target.as_deref()
    }
}

/// Successful translation into the one current task configuration.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TaskMigrationReport {
    schema_version: u64,
    source_release: &'static str,
    source_file: String,
    tasks: TaskMetadata,
    provenance: Vec<MigrationProvenance>,
}

impl TaskMigrationReport {
    /// Stable TOML rendering for the text command form.
    pub fn text(&self) -> Result<Vec<u8>, toml::ser::Error> {
        let mut bytes = toml::to_string_pretty(self)?.into_bytes();
        if bytes.last() != Some(&b'\n') {
            bytes.push(b'\n');
        }
        Ok(bytes)
    }

    /// The admitted current task configuration.
    #[must_use]
    pub const fn tasks(&self) -> &TaskMetadata {
        &self.tasks
    }

    /// Ordered migration provenance.
    #[must_use]
    pub fn provenance(&self) -> &[MigrationProvenance] {
        &self.provenance
    }
}

/// Stable predecessor task migration refusal category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MigrationErrorKind {
    Syntax,
    Schema,
    Release,
    Malformed,
    MalformedOverride,
    UnsupportedOverride,
}

impl fmt::Display for MigrationErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Syntax => "syntax",
            Self::Schema => "schema",
            Self::Release => "release",
            Self::Malformed => "malformed",
            Self::MalformedOverride => "malformed-override",
            Self::UnsupportedOverride => "unsupported-override",
        })
    }
}

/// One exact release-owned refusal with a concrete recovery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationError {
    kind: MigrationErrorKind,
    detail: Box<MigrationErrorDetail>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MigrationErrorDetail {
    source_file: String,
    team: Option<String>,
    field: String,
    list_index: Option<usize>,
    token: Option<String>,
    message: String,
    recovery: String,
}

impl MigrationError {
    /// Stable refusal category.
    #[must_use]
    pub const fn kind(&self) -> MigrationErrorKind {
        self.kind
    }

    /// Exact source file supplied to migration.
    #[must_use]
    pub fn source_file(&self) -> &str {
        &self.detail.source_file
    }

    /// Team owning a shell setting, when the refusal is team-scoped.
    #[must_use]
    pub fn team(&self) -> Option<&str> {
        self.detail.team.as_deref()
    }

    /// Exact predecessor field path.
    #[must_use]
    pub fn field(&self) -> &str {
        &self.detail.field
    }

    /// Source list index, when the field selected a list member.
    #[must_use]
    pub const fn list_index(&self) -> Option<usize> {
        self.detail.list_index
    }

    /// Exact refused override token, when present.
    #[must_use]
    pub fn token(&self) -> Option<&str> {
        self.detail.token.as_deref()
    }

    /// Concrete edit or export recovery.
    #[must_use]
    pub fn recovery(&self) -> &str {
        &self.detail.recovery
    }
}

impl fmt::Display for MigrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            concat!(
                "predecessor task migration release={} kind={} source={} team={} ",
                "field={} index={} token={}: {}; recovery={}"
            ),
            PREDECESSOR_TASK_RELEASE,
            self.kind,
            self.detail.source_file,
            self.detail.team.as_deref().unwrap_or("-"),
            self.detail.field,
            self.detail
                .list_index
                .map_or_else(|| "-".to_owned(), |index| index.to_string()),
            self.detail.token.as_deref().unwrap_or("-"),
            self.detail.message,
            self.detail.recovery,
        )
    }
}

impl Error for MigrationError {}

#[derive(Clone, Debug)]
struct Source {
    source_file: String,
    team: Option<String>,
    field: String,
    list_index: Option<usize>,
}

impl Source {
    fn root(source_file: &str, field: &str) -> Self {
        Self {
            source_file: source_file.to_owned(),
            team: None,
            field: field.to_owned(),
            list_index: None,
        }
    }

    fn team(source_file: &str, team: &str, field: String) -> Self {
        Self {
            source_file: source_file.to_owned(),
            team: Some(team.to_owned()),
            field,
            list_index: None,
        }
    }

    fn at(mut self, index: usize) -> Self {
        self.list_index = Some(index);
        self.field.push_str(&format!("[{index}]"));
        self
    }

    fn error(
        &self,
        kind: MigrationErrorKind,
        token: Option<String>,
        message: impl Into<String>,
        recovery: impl Into<String>,
    ) -> MigrationError {
        MigrationError {
            kind,
            detail: Box::new(MigrationErrorDetail {
                source_file: self.source_file.clone(),
                team: self.team.clone(),
                field: self.field.clone(),
                list_index: self.list_index,
                token,
                message: message.into(),
                recovery: recovery.into(),
            }),
        }
    }

    fn provenance(
        &self,
        decision: MigrationDecision,
        target: Option<String>,
    ) -> MigrationProvenance {
        MigrationProvenance {
            release: PREDECESSOR_TASK_RELEASE,
            source_kind: MigrationSource::Export,
            decision,
            source_file: self.source_file.clone(),
            team: self.team.clone(),
            field: self.field.clone(),
            list_index: self.list_index,
            target,
        }
    }
}

/// Translate one explicit predecessor export without reading ambient state.
pub fn migrate_predecessor_tasks(
    input: PredecessorTaskMigration<'_>,
) -> Result<TaskMigrationReport, MigrationError> {
    let input::Parsed {
        configured_hidden,
        hidden_provenance,
        team_provenance,
    } = input::parse(input)?;
    let tasks = TaskMetadata::migrated(configured_hidden.clone()).map_err(|error| {
        let index = match error {
            board::ProjectPolicyError::HiddenStem { index, .. } => index,
            board::ProjectPolicyError::Depth { .. } => None,
        };
        let source = index.map_or_else(
            || Source::root(input.source_file, "tasks.hidden_stems"),
            |index| Source::root(input.source_file, "tasks.hidden_stems").at(index),
        );
        source.error(
            MigrationErrorKind::Malformed,
            None,
            error.to_string(),
            "edit tasks.hidden_stems to satisfy the current project policy",
        )
    })?;

    let mut provenance = resolved_hidden_provenance(
        input.source_file,
        &configured_hidden,
        tasks.hidden_stems(),
        hidden_provenance,
    );
    provenance.extend(team_provenance);
    Ok(TaskMigrationReport {
        schema_version: TASK_MIGRATION_SCHEMA_VERSION,
        source_release: PREDECESSOR_TASK_RELEASE,
        source_file: input.source_file.to_owned(),
        tasks,
        provenance,
    })
}

fn resolved_hidden_provenance(
    source_file: &str,
    configured: &[String],
    resolved: &[String],
    configured_sources: Vec<Source>,
) -> Vec<MigrationProvenance> {
    let mut rows = Vec::new();
    let mut emitted = BTreeSet::new();
    for builtin in board::BUILTIN_HIDDEN_STEMS {
        let target_index = resolved
            .iter()
            .position(|candidate| candidate == builtin)
            .expect("current policy retains every builtin hidden stem");
        rows.push(MigrationProvenance {
            release: PREDECESSOR_TASK_RELEASE,
            source_kind: MigrationSource::Packaged,
            decision: MigrationDecision::Packaged,
            source_file: source_file.to_owned(),
            team: None,
            field: "tasks.hidden_stems::<builtin>".to_owned(),
            list_index: None,
            target: Some(format!("tasks.hidden_stems[{target_index}]")),
        });
        emitted.insert(builtin.to_owned());
    }
    for (stem, source) in configured.iter().zip(configured_sources) {
        let decision = if emitted.insert(stem.clone()) {
            MigrationDecision::Emitted
        } else {
            MigrationDecision::Deduplicated
        };
        let target_index = resolved
            .iter()
            .position(|candidate| candidate == stem)
            .expect("current policy retains every unique configured hidden stem");
        rows.push(source.provenance(
            decision,
            Some(format!("tasks.hidden_stems[{target_index}]")),
        ));
    }
    rows
}

#[cfg(test)]
mod tests;
