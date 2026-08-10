//! The typed release-one configuration root and its complete leaf registry.
//!
//! Domain crates remain the only authorities for admitting values. This
//! module composes those values and publishes metadata used to route layers,
//! render help, and report provenance without carrying a second value parser.

use std::collections::BTreeSet;

use serde::de::{Deserializer, Error as _};
use serde::{Deserialize, Serialize};
use wire::ConfigSource;

mod load;
mod pre_commit;
mod resolve;
mod task_migration;

pub use load::{
    AcquisitionIssue, AcquisitionKind, CaptureGeneration, LoadError, Loaded, Locations,
    capture_repository, load, load_repository, locations,
};
pub use pre_commit::{PreCommitScopes, PreCommitStep, predecessor_bridge};
pub use resolve::{
    ConcretePath, Document, FlagError, FlagOverride, FlagValue, Inputs, Issue, IssueKind,
    Provenance, ResolveError, Resolved, resolve,
};
pub use task_migration::{
    MigrationDecision as TaskMigrationDecision, MigrationError as TaskMigrationError,
    MigrationErrorKind as TaskMigrationErrorKind, MigrationProvenance as TaskMigrationProvenance,
    MigrationSource as TaskMigrationSource, PREDECESSOR_TASK_RELEASE, PredecessorTaskMigration,
    RAW_OVERRIDE_MAPPINGS, RawOverrideMapping, TASK_MIGRATION_SCHEMA_VERSION, TaskMigrationReport,
    migrate_predecessor_tasks,
};

/// Every release-one configuration domain.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Root {
    tasks: TaskMetadata,
    judge: JudgeConfig,
    policy: WorkspacePolicy,
    taste: taste::Config,
    consent: consent::Config,
    gate: gate::Config,
    board: board::Config,
    lane: lane::Config,
    steer: steer::Config,
    vendor: vendor::Config,
    listen: listen::Config,
    lock: sill::LockConfig,
    canvas: canvas::Config,
    say: canvas::SpeechConfig,
    proof: proof::Config,
}

impl Root {
    /// Shared task scheduler declarations.
    #[must_use]
    pub const fn tasks(&self) -> &TaskMetadata {
        &self.tasks
    }

    /// Whether this repository opts into independent maxim adjudication.
    #[must_use]
    pub const fn judge(&self) -> &JudgeConfig {
        &self.judge
    }

    /// Workspace-owned environment-name policy.
    #[must_use]
    pub const fn policy(&self) -> &WorkspacePolicy {
        &self.policy
    }

    /// Active prose tastes.
    #[must_use]
    pub const fn taste(&self) -> &taste::Config {
        &self.taste
    }

    /// Ordered consent policy.
    #[must_use]
    pub const fn consent(&self) -> &consent::Config {
        &self.consent
    }

    /// Gate walk policy.
    #[must_use]
    pub const fn gate(&self) -> &gate::Config {
        &self.gate
    }

    /// Allocator tuning.
    #[must_use]
    pub const fn board(&self) -> &board::Config {
        &self.board
    }

    /// Lane supervision policy.
    #[must_use]
    pub const fn lane(&self) -> &lane::Config {
        &self.lane
    }

    /// Steering horizons.
    #[must_use]
    pub const fn steer(&self) -> &steer::Config {
        &self.steer
    }

    /// Vendor-specific launch policy.
    #[must_use]
    pub const fn vendor(&self) -> &vendor::Config {
        &self.vendor
    }

    /// Listening horizon.
    #[must_use]
    pub const fn listen(&self) -> &listen::Config {
        &self.listen
    }

    /// Lock exit and capacity policy.
    #[must_use]
    pub const fn lock(&self) -> &sill::LockConfig {
        &self.lock
    }

    /// Canvas packing limits.
    #[must_use]
    pub const fn canvas(&self) -> &canvas::Config {
        &self.canvas
    }

    /// Browser-owned speech playback and its optional local renderer.
    #[must_use]
    pub const fn say(&self) -> &canvas::SpeechConfig {
        &self.say
    }

    /// Proof image policy and machine-local executable locations.
    #[must_use]
    pub const fn proof(&self) -> &proof::Config {
        &self.proof
    }
}

/// Optional resident maxim-adjudication policy.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct JudgeConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    enabled: Option<bool>,
}

impl JudgeConfig {
    /// Whether resident taste candidates cross the shared judge boundary.
    #[must_use]
    pub const fn enabled(&self) -> Option<bool> {
        self.enabled
    }
}

/// Tracked metadata shared with the task scheduler.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TaskMetadata {
    stems: Vec<String>,
    hidden_stems: Vec<String>,
    project_min_depth: u64,
    project_max_depth: u64,
}

impl TaskMetadata {
    /// Additional public project stems in tracked declaration order.
    #[must_use]
    pub fn stems(&self) -> &[String] {
        &self.stems
    }

    /// Fully resolved hidden roots in declared order, without leading dots.
    #[must_use]
    pub fn hidden_stems(&self) -> &[String] {
        &self.hidden_stems
    }

    /// One immutable classifier for board admission and allocation.
    #[must_use]
    pub fn project_policy(&self) -> board::ProjectPolicy {
        board::ProjectPolicy::new(
            self.stems.clone(),
            self.hidden_stems.clone(),
            self.project_min_depth,
            self.project_max_depth,
        )
        .expect("admitted task metadata always reconstructs its project policy")
    }

    /// Fewest dotted segments admitted for a public project.
    #[must_use]
    pub const fn project_min_depth(&self) -> u64 {
        self.project_min_depth
    }

    /// Most dotted segments admitted for a public project.
    #[must_use]
    pub const fn project_max_depth(&self) -> u64 {
        self.project_max_depth
    }

    fn migrated(hidden_additions: Vec<String>) -> Result<Self, board::ProjectPolicyError> {
        let defaults = Self::default();
        let projects = board::ProjectPolicy::new(
            defaults.stems.clone(),
            hidden_additions,
            defaults.project_min_depth,
            defaults.project_max_depth,
        )?;
        Ok(Self {
            hidden_stems: projects.hidden_stems().to_vec(),
            ..defaults
        })
    }
}

impl Default for TaskMetadata {
    fn default() -> Self {
        Self {
            stems: Vec::new(),
            hidden_stems: board::BUILTIN_HIDDEN_STEMS
                .into_iter()
                .map(str::to_owned)
                .collect(),
            project_min_depth: default_project_min_depth(),
            project_max_depth: default_project_max_depth(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskMetadataInput {
    #[serde(default, deserialize_with = "unique_names")]
    stems: Vec<String>,
    #[serde(default)]
    hidden_stems: Vec<String>,
    #[serde(
        default = "default_project_min_depth",
        deserialize_with = "positive_depth"
    )]
    project_min_depth: u64,
    #[serde(
        default = "default_project_max_depth",
        deserialize_with = "positive_depth"
    )]
    project_max_depth: u64,
}

impl<'de> Deserialize<'de> for TaskMetadata {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let input = TaskMetadataInput::deserialize(deserializer)?;
        if input.project_min_depth > input.project_max_depth {
            return Err(D::Error::custom(format!(
                concat!(
                    "tasks.project_min_depth must be less than or equal to ",
                    "tasks.project_max_depth: {} > {}"
                ),
                input.project_min_depth, input.project_max_depth,
            )));
        }
        let projects = board::ProjectPolicy::new(
            input.stems.clone(),
            input.hidden_stems,
            input.project_min_depth,
            input.project_max_depth,
        )
        .map_err(D::Error::custom)?;
        Ok(Self {
            stems: input.stems,
            hidden_stems: projects.hidden_stems().to_vec(),
            project_min_depth: input.project_min_depth,
            project_max_depth: input.project_max_depth,
        })
    }
}

/// Tracked workspace environment vocabulary used by Gate and Doctor.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct WorkspacePolicy {
    #[serde(deserialize_with = "unique_names")]
    env_names: Vec<String>,
    pre_commit_builtins: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pre_commit: Vec<PreCommitStep>,
    commit_message: gate::CommitMessagePolicy,
    repo_truth: gate::RepoTruthPolicy,
    file_shape: gate::FileShapePolicy,
    languages: gate::LanguagesPolicy,
    package_roots: Vec<String>,
    generated_paths: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name_cluster_threshold: Option<usize>,
    test_paths: Vec<String>,
    package: gate::PackagePolicy,
    admission: Vec<String>,
}

impl WorkspacePolicy {
    /// Environment names declared by the tracked workspace.
    #[must_use]
    pub fn env_names(&self) -> &[String] {
        &self.env_names
    }

    /// Whether the installed predecessor may run its Python builtin battery.
    #[must_use]
    pub const fn pre_commit_builtins(&self) -> bool {
        self.pre_commit_builtins
    }

    /// Repository-owned commands the predecessor runs before its own commit gate.
    ///
    /// The current Rust gate admits these declarations so both runtimes can
    /// read one tracked document, but it deliberately does not execute them:
    /// the repository's bridge is one predecessor-to-Rust edge, not recursion.
    #[must_use]
    pub fn pre_commit(&self) -> &[PreCommitStep] {
        &self.pre_commit
    }

    /// Documents the workspace measures as repository truth.
    ///
    /// Gate compiles this into its rule policy from the same tracked document,
    /// so admitting it here is what lets a repository declare it at all.
    #[must_use]
    pub const fn repo_truth(&self) -> &gate::RepoTruthPolicy {
        &self.repo_truth
    }

    /// Commit trailer policy shared by launch attribution and the native hook.
    #[must_use]
    pub const fn commit_message(&self) -> &gate::CommitMessagePolicy {
        &self.commit_message
    }

    /// Files the workspace measures shape over.
    ///
    /// Gate requires this from the same tracked document, so admitting it here
    /// is what lets the document carrying it load at all.
    #[must_use]
    pub const fn file_shape(&self) -> &gate::FileShapePolicy {
        &self.file_shape
    }

    /// Files the workspace offers each language arm.
    #[must_use]
    pub const fn languages(&self) -> &gate::LanguagesPolicy {
        &self.languages
    }

    /// Roots the repository declares its packages under.
    #[must_use]
    pub fn package_roots(&self) -> &[String] {
        &self.package_roots
    }

    /// Paths the repository declares as generated rather than authored.
    #[must_use]
    pub fn generated_paths(&self) -> &[String] {
        &self.generated_paths
    }

    /// Cluster size at which the repository wants related names reported.
    #[must_use]
    pub const fn name_cluster_threshold(&self) -> Option<usize> {
        self.name_cluster_threshold
    }

    /// Roots the repository declares its tests under.
    #[must_use]
    pub fn test_paths(&self) -> &[String] {
        &self.test_paths
    }

    /// Module-name shape the repository declares.
    #[must_use]
    pub const fn package(&self) -> &gate::PackagePolicy {
        &self.package
    }

    /// Command that admits one integrated tree before it is published.
    ///
    /// Task completion merges the lane onto a baseline it never saw, so the
    /// tree it publishes is an artifact no lane authored and no verifier ran.
    /// This is the cheap load that tree must survive: it answers whether the
    /// repository can be read at all, not whether it passes. A repository
    /// declaring nothing here publishes synthesized trees unadmitted, which is
    /// how a clean textual merge of two valid manifests reached a baseline as a
    /// duplicate table.
    #[must_use]
    pub fn admission(&self) -> &[String] {
        &self.admission
    }
}

const fn default_project_min_depth() -> u64 {
    dialect::PROJECT_MIN_DEPTH as u64
}

const fn default_project_max_depth() -> u64 {
    dialect::PROJECT_MAX_DEPTH as u64
}

fn positive_depth<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    let depth = u64::deserialize(deserializer)?;
    if depth == 0 {
        Err(D::Error::custom("project depth must be positive"))
    } else {
        Ok(depth)
    }
}

fn unique_names<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    let names = Vec::<String>::deserialize(deserializer)?;
    let mut seen = BTreeSet::new();
    for name in &names {
        if name.is_empty() {
            return Err(D::Error::custom("configuration names must be nonempty"));
        }
        if !seen.insert(name) {
            return Err(D::Error::custom(format!(
                "configuration names must be unique; duplicate {name:?}"
            )));
        }
    }
    Ok(names)
}

fn nonempty_string<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    if value.is_empty() {
        Err(D::Error::custom("pre-commit labels must be nonempty"))
    } else {
        Ok(value)
    }
}

fn nonempty_argv<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    let arguments = Vec::<String>::deserialize(deserializer)?;
    if arguments.is_empty() {
        return Err(D::Error::custom("pre-commit run argv must not be empty"));
    }
    if arguments.iter().any(String::is_empty) {
        return Err(D::Error::custom(
            "pre-commit run argv entries must be nonempty",
        ));
    }
    Ok(arguments)
}

/// Serialized shape of one leaf value.
///
/// Semantic validation still belongs to the domain type named by [`Root`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueKind {
    /// One boolean.
    Boolean,
    /// One string.
    String,
    /// One ordered string list.
    StringList,
    /// One unsigned integer.
    UnsignedInteger,
    /// One finite floating-point number.
    Float,
    /// One duration word.
    Duration,
    /// One ordered duration-word list.
    DurationList,
    /// One consent answer.
    Answer,
    /// One lane life variant.
    Life,
}

impl ValueKind {
    /// Stable metadata spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Boolean => "boolean",
            Self::String => "string",
            Self::StringList => "string-list",
            Self::UnsignedInteger => "unsigned-integer",
            Self::Float => "float",
            Self::Duration => "duration",
            Self::DurationList => "duration-list",
            Self::Answer => "answer",
            Self::Life => "life",
        }
    }
}

/// A compiled default represented as explicit absence or a serde scalar.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "kind", content = "value")]
pub enum Builtin {
    /// An optional fixed leaf has no compiled value.
    Absent,
    /// One string-shaped value.
    String(&'static str),
    /// One ordered string-shaped list.
    Strings(&'static [&'static str]),
    /// One unsigned integer.
    UnsignedInteger(u64),
    /// One finite floating-point number.
    Float(f64),
    /// One boolean.
    Boolean(bool),
}

/// Whether a leaf is tracked policy or an admitted machine-local setting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PolicyClass {
    /// Behavior that is owned by tracked policy except for an explicit flag.
    Policy,
    /// Machine-specific operation that admits a local value.
    MachineLocal,
}

impl PolicyClass {
    /// Stable metadata spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Policy => "policy",
            Self::MachineLocal => "machine-local",
        }
    }
}

/// One command flag that may supply a configuration value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FlagMapping {
    command: &'static [&'static str],
    flag: &'static str,
}

impl FlagMapping {
    const fn new(command: &'static [&'static str], flag: &'static str) -> Self {
        Self { command, flag }
    }

    /// Command path carrying the flag.
    #[must_use]
    pub const fn command(self) -> &'static [&'static str] {
        self.command
    }

    /// Exact long flag spelling.
    #[must_use]
    pub const fn flag(self) -> &'static str {
        self.flag
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TemplateShape {
    Fixed,
    Named,
    Indexed,
}

/// One unambiguous segment of a concrete configuration leaf path.
///
/// Keys remain whole even when a domain admits punctuation in a dynamic name;
/// indexes are separate from keys rather than encoded into display text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathSegment<'a> {
    /// One table or field key.
    Key(&'a str),
    /// One ordered row index.
    Index(usize),
}

/// One fixed or dynamic leaf path template.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PathTemplate {
    rendered: &'static str,
    shape: TemplateShape,
}

impl PathTemplate {
    const fn fixed(rendered: &'static str) -> Self {
        Self {
            rendered,
            shape: TemplateShape::Fixed,
        }
    }

    const fn named(rendered: &'static str) -> Self {
        Self {
            rendered,
            shape: TemplateShape::Named,
        }
    }

    const fn indexed(rendered: &'static str) -> Self {
        Self {
            rendered,
            shape: TemplateShape::Indexed,
        }
    }

    /// Stable path spelling, using `<name>` and `[]` for dynamic segments.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.rendered
    }

    /// Whether this template covers one concrete leaf path.
    #[must_use]
    pub fn matches(self, path: &[PathSegment<'_>]) -> bool {
        self.covers(path, true)
    }

    pub(crate) fn accepts_prefix(self, path: &[PathSegment<'_>]) -> bool {
        self.covers(path, false)
    }

    fn covers(self, path: &[PathSegment<'_>], exact: bool) -> bool {
        let mut at = 0;
        for segment in self.rendered.split('.') {
            if at == path.len() {
                return !exact;
            }
            if segment == "<name>" {
                if !matches!(path.get(at), Some(PathSegment::Key(_))) {
                    return false;
                }
                at += 1;
            } else if let Some(key) = segment.strip_suffix("[]") {
                if path.get(at) != Some(&PathSegment::Key(key)) {
                    return false;
                }
                at += 1;
                if at == path.len() {
                    return !exact;
                }
                if !matches!(path.get(at), Some(PathSegment::Index(_))) {
                    return false;
                }
                at += 1;
            } else {
                if path.get(at) != Some(&PathSegment::Key(segment)) {
                    return false;
                }
                at += 1;
            }
        }
        at == path.len()
    }

    /// Whether the template contains a named or indexed row.
    #[must_use]
    pub const fn is_dynamic(self) -> bool {
        !matches!(self.shape, TemplateShape::Fixed)
    }
}

/// Metadata and routing policy for one leaf template.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Descriptor {
    template: PathTemplate,
    kind: ValueKind,
    builtin: Option<Builtin>,
    layers: &'static [ConfigSource],
    class: PolicyClass,
    flag: Option<FlagMapping>,
}

impl Descriptor {
    /// Fixed or dynamic path template.
    #[must_use]
    pub const fn template(self) -> PathTemplate {
        self.template
    }

    /// Serialized value shape.
    #[must_use]
    pub const fn kind(self) -> ValueKind {
        self.kind
    }

    /// Compiled default; `None` is reserved for operator-created dynamic rows.
    #[must_use]
    pub const fn builtin(self) -> Option<Builtin> {
        self.builtin
    }

    /// Admitted authored layers in increasing precedence order.
    #[must_use]
    pub const fn layers(self) -> &'static [ConfigSource] {
        self.layers
    }

    /// Tracked-policy or machine-local classification.
    #[must_use]
    pub const fn policy_class(self) -> PolicyClass {
        self.class
    }

    /// Command flag supplying this leaf, when one exists.
    #[must_use]
    pub const fn flag(self) -> Option<FlagMapping> {
        self.flag
    }
}

const TRACKED: &[ConfigSource] = &[ConfigSource::Tracked];
const TRACKED_LOCAL: &[ConfigSource] = &[ConfigSource::Tracked, ConfigSource::Local];
const TRACKED_FLAG: &[ConfigSource] = &[ConfigSource::Tracked, ConfigSource::Flag];
const LOCAL: &[ConfigSource] = &[ConfigSource::Local];

macro_rules! registry {
    ($(
        $shape:ident $path:literal {
            kind: $kind:ident,
            builtin: $builtin:expr,
            layers: $layers:ident,
            class: $class:ident,
            flag: $flag:expr,
        }
    )+) => {
        const DESCRIPTORS: &[Descriptor] = &[
            $(Descriptor {
                template: PathTemplate::$shape($path),
                kind: ValueKind::$kind,
                builtin: $builtin,
                layers: $layers,
                class: PolicyClass::$class,
                flag: $flag,
            }),+
        ];
    };
}

registry! {
    fixed "tasks.stems" {
        kind: StringList,
        builtin: Some(Builtin::Strings(&[])),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "tasks.hidden_stems" {
        kind: StringList,
        builtin: Some(Builtin::Strings(&board::BUILTIN_HIDDEN_STEMS)),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "tasks.project_min_depth" {
        kind: UnsignedInteger,
        builtin: Some(Builtin::UnsignedInteger(dialect::PROJECT_MIN_DEPTH as u64)),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "tasks.project_max_depth" {
        kind: UnsignedInteger,
        builtin: Some(Builtin::UnsignedInteger(dialect::PROJECT_MAX_DEPTH as u64)),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "judge.enabled" {
        kind: Boolean,
        builtin: Some(Builtin::Absent),
        layers: TRACKED_LOCAL,
        class: MachineLocal,
        flag: None,
    }
    fixed "policy.env_names" {
        kind: StringList,
        builtin: Some(Builtin::Strings(&[])),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "policy.pre_commit_builtins" {
        kind: Boolean,
        builtin: Some(Builtin::Boolean(false)),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    indexed "policy.pre_commit[].label" {
        kind: String,
        builtin: None,
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    indexed "policy.pre_commit[].run" {
        kind: StringList,
        builtin: None,
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    indexed "policy.pre_commit[].scopes.paths" {
        kind: StringList,
        builtin: None,
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "policy.commit_message.allowed_trailers" {
        kind: StringList,
        builtin: Some(Builtin::Absent),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "policy.commit_message.blocked_trailers" {
        kind: StringList,
        builtin: Some(Builtin::Absent),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "policy.repo_truth.docs" {
        kind: StringList,
        builtin: Some(Builtin::Absent),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "policy.file_shape.source_suffixes" {
        kind: StringList,
        builtin: Some(Builtin::Absent),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "policy.languages.complexity" {
        kind: StringList,
        builtin: Some(Builtin::Absent),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "policy.languages.magic" {
        kind: StringList,
        builtin: Some(Builtin::Absent),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "policy.package_roots" {
        kind: StringList,
        builtin: Some(Builtin::Strings(&[])),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "policy.generated_paths" {
        kind: StringList,
        builtin: Some(Builtin::Strings(&[])),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "policy.name_cluster_threshold" {
        kind: UnsignedInteger,
        builtin: Some(Builtin::Absent),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "policy.test_paths" {
        kind: StringList,
        builtin: Some(Builtin::Strings(&[])),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "policy.package.boundary_underscore_pattern" {
        kind: String,
        builtin: Some(Builtin::Absent),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "policy.admission" {
        kind: StringList,
        builtin: Some(Builtin::Strings(&[])),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    named "taste.<name>.words" {
        kind: StringList,
        builtin: None,
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    named "taste.<name>.message" {
        kind: String,
        builtin: None,
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "consent.posture" {
        kind: Answer,
        builtin: Some(Builtin::String("queue")),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    indexed "consent.rule[].name" {
        kind: String,
        builtin: None,
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    indexed "consent.rule[].tool" {
        kind: String,
        builtin: None,
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    indexed "consent.rule[].answer" {
        kind: Answer,
        builtin: None,
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    indexed "consent.rule[].budget" {
        kind: UnsignedInteger,
        builtin: None,
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "gate.walk.skip" {
        kind: StringList,
        builtin: Some(Builtin::Strings(&[])),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "board.next.band" {
        kind: UnsignedInteger,
        builtin: Some(Builtin::UnsignedInteger(2)),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "lane.life" {
        kind: Life,
        builtin: Some(Builtin::String("Drive")),
        layers: TRACKED_FLAG,
        class: Policy,
        flag: Some(FlagMapping::new(&["lane", "add"], "--life")),
    }
    fixed "lane.lease" {
        kind: Duration,
        builtin: Some(Builtin::String("15m")),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "lane.vendor" {
        kind: String,
        builtin: Some(Builtin::String("claude")),
        layers: TRACKED_FLAG,
        class: Policy,
        flag: Some(FlagMapping::new(&["lane", "add"], "--vendor")),
    }
    fixed "lane.deaths" {
        kind: UnsignedInteger,
        builtin: Some(Builtin::UnsignedInteger(3)),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "lane.young" {
        kind: Duration,
        builtin: Some(Builtin::String("30s")),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "lane.hold" {
        kind: Duration,
        builtin: Some(Builtin::String("30m")),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "steer.expiry" {
        kind: Duration,
        builtin: Some(Builtin::String("24h")),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "steer.resend" {
        kind: DurationList,
        builtin: Some(Builtin::Strings(&["15s", "60s", "300s"])),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "vendor.claude.binary" {
        kind: String,
        builtin: Some(Builtin::String("claude")),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "vendor.claude.model" {
        kind: String,
        builtin: Some(Builtin::String(vendor::DEFAULT_CLAUDE_MODEL)),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "vendor.claude.effort" {
        kind: String,
        builtin: Some(Builtin::String("high")),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "vendor.claude.auto_compact_window_tokens" {
        kind: UnsignedInteger,
        builtin: Some(Builtin::UnsignedInteger(
            vendor::DEFAULT_CLAUDE_AUTO_COMPACT_WINDOW_TOKENS,
        )),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "vendor.codex.binary" {
        kind: String,
        builtin: Some(Builtin::String("codex")),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "vendor.codex.model" {
        kind: String,
        builtin: Some(Builtin::String(vendor::DEFAULT_CODEX_MODEL)),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "vendor.codex.effort" {
        kind: String,
        builtin: Some(Builtin::String("high")),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "vendor.codex.personality" {
        kind: String,
        builtin: Some(Builtin::Absent),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "vendor.codex.fast_mode" {
        kind: Boolean,
        builtin: Some(Builtin::Boolean(false)),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "listen.window" {
        kind: Duration,
        builtin: Some(Builtin::String("90s")),
        layers: TRACKED_LOCAL,
        class: MachineLocal,
        flag: None,
    }
    fixed "lock.busy" {
        kind: UnsignedInteger,
        builtin: Some(Builtin::UnsignedInteger(75)),
        layers: TRACKED_LOCAL,
        class: MachineLocal,
        flag: None,
    }
    fixed "lock.taken" {
        kind: UnsignedInteger,
        builtin: Some(Builtin::UnsignedInteger(76)),
        layers: TRACKED_LOCAL,
        class: MachineLocal,
        flag: None,
    }
    fixed "lock.full" {
        kind: UnsignedInteger,
        builtin: Some(Builtin::UnsignedInteger(77)),
        layers: TRACKED_LOCAL,
        class: MachineLocal,
        flag: None,
    }
    named "lock.<name>.path" {
        kind: String,
        builtin: None,
        layers: TRACKED_LOCAL,
        class: MachineLocal,
        flag: None,
    }
    named "lock.<name>.directory" {
        kind: String,
        builtin: None,
        layers: TRACKED_LOCAL,
        class: MachineLocal,
        flag: None,
    }
    named "lock.<name>.shards" {
        kind: UnsignedInteger,
        builtin: None,
        layers: TRACKED_LOCAL,
        class: MachineLocal,
        flag: None,
    }
    fixed "canvas.pack.rows" {
        kind: UnsignedInteger,
        builtin: Some(Builtin::UnsignedInteger(40)),
        layers: TRACKED_LOCAL,
        class: MachineLocal,
        flag: None,
    }
    fixed "say.backend" {
        kind: String,
        builtin: Some(Builtin::String("system")),
        layers: TRACKED_LOCAL,
        class: MachineLocal,
        flag: None,
    }
    fixed "say.command" {
        kind: StringList,
        builtin: Some(Builtin::Strings(&[])),
        layers: LOCAL,
        class: MachineLocal,
        flag: None,
    }
    fixed "say.voice" {
        kind: String,
        builtin: Some(Builtin::String(canvas::speech::DEFAULT_VOICE)),
        layers: TRACKED_LOCAL,
        class: MachineLocal,
        flag: None,
    }
    fixed "say.content_type" {
        kind: String,
        builtin: Some(Builtin::String(canvas::speech::DEFAULT_CONTENT_TYPE)),
        layers: TRACKED_LOCAL,
        class: MachineLocal,
        flag: None,
    }
    fixed "say.words_per_minute" {
        kind: UnsignedInteger,
        builtin: Some(Builtin::UnsignedInteger(
            canvas::speech::DEFAULT_WORDS_PER_MINUTE as u64,
        )),
        layers: TRACKED_LOCAL,
        class: MachineLocal,
        flag: None,
    }
    fixed "proof.image" {
        kind: String,
        builtin: Some(Builtin::String(proof::DEFAULT_IMAGE)),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "proof.container" {
        kind: String,
        builtin: Some(Builtin::Absent),
        layers: LOCAL,
        class: MachineLocal,
        flag: None,
    }
    fixed "proof.runner" {
        kind: String,
        builtin: Some(Builtin::Absent),
        layers: LOCAL,
        class: MachineLocal,
        flag: None,
    }
}

/// Complete descriptor registry in stable configuration order.
#[must_use]
pub const fn descriptors() -> &'static [Descriptor] {
    DESCRIPTORS
}

/// Find the sole descriptor covering a concrete leaf path.
#[must_use]
pub fn descriptor(path: &[PathSegment<'_>]) -> Option<&'static Descriptor> {
    DESCRIPTORS
        .iter()
        .find(|descriptor| descriptor.template.matches(path))
}
