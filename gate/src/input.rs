//! One current, tracked input contract for executable gate runs.
//!
//! The repository contributes intent and lower-is-better state through two
//! fixed files. Everything capable of execution remains compiled here: metric
//! names, parser argv, tool probes, deadlines, grace, capture, and default walk
//! exclusions. A tracked byte can narrow jurisdiction or lower a floor, but it
//! cannot name a program or revive an old gate arm.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sill::{Argv, Completion, Deadlines, SpecError, WaitError, spawn};
use wire::{Digest, Domain};

use crate::jurisdiction::{Ignore, IgnoreError, Rule};
use crate::measure::{Bounds, Metric, MetricError, Parser, Policy};
use crate::ratchet::Ratchet;

/// Fixed tracked policy path within a selected repository tree.
pub const POLICY_PATH: &str = "spice.toml";
/// Fixed tracked lower-is-better state path within a selected repository tree.
pub const BASELINE_PATH: &str = "baseline.toml";
/// The one baseline schema this build executes.
pub const BASELINE_VERSION: u64 = 1;
/// Byte length above which one Rust source line contributes debt.
pub const LONG_LINE_LIMIT: usize = 100;

const AWK: &str = "/usr/bin/awk";
const AWK_PROBE: &str = "BEGIN { print \"spice-gate-awk-v1\" }";
const AWK_PROBE_OUTPUT: &str = "spice-gate-awk-v1";
const DEFAULT_SKIP: &[&str] = &[".git/", "target/"];
const RETIRED_FLOORS: &[&str] = &["rust.kill-rate"];

/// How one lower-is-better floor obtains its current measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloorKind {
    /// An ordinary parser-backed [`Metric`].
    Measurement,
    /// Confirmed survivor debt produced by the mutation battery.
    Mutation,
}

impl FloorKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Measurement => "measurement",
            Self::Mutation => "mutation",
        }
    }
}

/// One compiled lower-is-better name in the current gate set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FloorDeclaration {
    name: String,
    kind: FloorKind,
}

impl FloorDeclaration {
    fn new(name: &str, kind: FloorKind) -> Self {
        Self {
            name: name.to_owned(),
            kind,
        }
    }

    /// Stable floor name used by `baseline.toml`.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Which current execution path measures this floor.
    #[must_use]
    pub const fn kind(&self) -> FloorKind {
        self.kind
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolDeclaration {
    name: String,
    program: String,
    probe_arguments: Vec<String>,
    probe_output: String,
}

impl ToolDeclaration {
    fn awk() -> Self {
        Self {
            name: "awk".to_owned(),
            program: AWK.to_owned(),
            probe_arguments: vec![AWK_PROBE.to_owned()],
            probe_output: AWK_PROBE_OUTPUT.to_owned(),
        }
    }

    fn render(&self, into: &mut String) {
        let _ = writeln!(into, "tool {} program {}", self.name, self.program);
        for argument in &self.probe_arguments {
            let _ = writeln!(into, "tool {} probe-argument {argument}", self.name);
        }
        let _ = writeln!(
            into,
            "tool {} probe-output {}",
            self.name, self.probe_output
        );
    }
}

/// The compiled checks and floor names this build can execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateSet {
    metrics: Vec<Metric>,
    floors: Vec<FloorDeclaration>,
    tools: Vec<ToolDeclaration>,
}

impl GateSet {
    fn current() -> Result<Self, MetricError> {
        let program =
            format!("length($0) > {LONG_LINE_LIMIT} {{ debt += 1 }} END {{ print debt + 0 }}");
        let parser = Parser::new(AWK, [program])?;
        let metric = Metric::new(
            "rust.long-lines",
            "rust",
            format!("source lines longer than {LONG_LINE_LIMIT} bytes"),
            parser,
            "*.rs",
            "",
        )?;

        Ok(Self {
            metrics: vec![metric],
            floors: vec![
                FloorDeclaration::new("rust.long-lines", FloorKind::Measurement),
                FloorDeclaration::new("rust.mutation-survivors", FloorKind::Mutation),
            ],
            tools: vec![ToolDeclaration::awk()],
        })
    }

    /// Current floor declarations in canonical name order.
    #[must_use]
    pub fn floors(&self) -> &[FloorDeclaration] {
        &self.floors
    }

    /// Canonical compiled declaration, including parser argv and tool probes.
    #[must_use]
    pub fn render(&self) -> String {
        let mut rendered = String::from("gate-set 1\n");
        let _ = writeln!(rendered, "parser-file-limit {}", Parser::FILE_LIMIT);
        for floor in &self.floors {
            let _ = writeln!(rendered, "floor {} {}", floor.name, floor.kind.as_str());
        }
        for tool in &self.tools {
            tool.render(&mut rendered);
        }
        for metric in &self.metrics {
            metric.render(&mut rendered);
        }
        rendered
    }

    /// Identity of the complete compiled set.
    #[must_use]
    pub fn digest(&self) -> Digest {
        Digest::compute(Domain::GateSet, self.render().as_bytes())
    }
}

/// One loaded lower-is-better floor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Floor {
    name: String,
    value: u64,
}

impl Floor {
    /// Stable current gate name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Maximum debt admitted by the tracked state.
    #[must_use]
    pub const fn value(&self) -> u64 {
        self.value
    }
}

/// Complete, canonical `baseline.toml` state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Baseline {
    floors: Vec<Floor>,
}

impl Baseline {
    fn parse(path: &Path, text: &str, gate_set: &GateSet) -> Result<Self, BaselineError> {
        let table = parse_baseline_table(path, text)?;
        validate_baseline_version(path, &table)?;
        let rows = baseline_rows(path, &table)?;
        let floors = parse_floor_rows(path, rows, gate_set)?;
        require_all_floors(path, &floors, gate_set)?;

        let baseline = Self { floors };
        let canonical = baseline.render();
        if canonical != text {
            return Err(BaselineError::NonCanonical {
                path: path.to_path_buf(),
                field: "document".to_owned(),
                canonical,
            });
        }
        Ok(baseline)
    }

    /// Every floor in current gate-name order.
    #[must_use]
    pub fn floors(&self) -> &[Floor] {
        &self.floors
    }

    /// One floor by current gate name.
    #[must_use]
    pub fn floor(&self, name: &str) -> Option<u64> {
        self.floors
            .iter()
            .find(|floor| floor.name == name)
            .map(|floor| floor.value)
    }

    /// The one admitted baseline spelling.
    #[must_use]
    pub fn render(&self) -> String {
        let mut rendered = format!("version = {BASELINE_VERSION}\n");
        for floor in &self.floors {
            let _ = write!(
                rendered,
                "\n[[floor]]\nname = \"{}\"\nvalue = {}\n",
                floor.name, floor.value
            );
        }
        rendered
    }

    /// Identity of the complete versioned state.
    #[must_use]
    pub fn digest(&self) -> Digest {
        Digest::compute(Domain::GateBaseline, self.render().as_bytes())
    }

    fn ratchet(&self, gate_set: &GateSet) -> Ratchet {
        let mut text = String::new();
        for declaration in gate_set
            .floors
            .iter()
            .filter(|floor| matches!(floor.kind, FloorKind::Measurement))
        {
            let value = self
                .floor(declaration.name())
                .expect("a loaded baseline carries every declared floor");
            let _ = writeln!(text, "{} {value}", declaration.name());
        }
        Ratchet::parse(&text).expect("validated baseline names render one ratchet spelling")
    }
}

fn parse_baseline_table(path: &Path, text: &str) -> Result<toml::Table, BaselineError> {
    let table = text
        .parse::<toml::Table>()
        .map_err(|source| BaselineError::Toml {
            path: path.to_path_buf(),
            source,
        })?;
    reject_unknown_keys(path, "", &table, &["version", "floor"]).map_err(BaselineError::Field)?;
    Ok(table)
}

fn validate_baseline_version(path: &Path, table: &toml::Table) -> Result<(), BaselineError> {
    let version = required(path, table, "version").map_err(BaselineError::Field)?;
    let version = version
        .as_integer()
        .and_then(|value| u64::try_from(value).ok())
        .ok_or_else(|| {
            BaselineError::Field(FieldError::type_mismatch(
                path,
                "version",
                "a non-negative integer",
            ))
        })?;
    if version != BASELINE_VERSION {
        return Err(BaselineError::UnsupportedVersion {
            path: path.to_path_buf(),
            field: "version".to_owned(),
            version,
        });
    }
    Ok(())
}

fn baseline_rows<'a>(
    path: &Path,
    table: &'a toml::Table,
) -> Result<&'a [toml::Value], BaselineError> {
    required(path, table, "floor")
        .map_err(BaselineError::Field)?
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| {
            BaselineError::Field(FieldError::type_mismatch(
                path,
                "floor",
                "an array of floor tables",
            ))
        })
}

fn parse_floor_rows(
    path: &Path,
    rows: &[toml::Value],
    gate_set: &GateSet,
) -> Result<Vec<Floor>, BaselineError> {
    let declared: BTreeSet<&str> = gate_set.floors.iter().map(|floor| floor.name()).collect();
    let mut floors = Vec::with_capacity(rows.len());
    let mut seen = BTreeSet::new();
    let mut previous: Option<String> = None;
    for (index, row) in rows.iter().enumerate() {
        let (floor, name_field) = parse_floor_row(path, index, row)?;
        validate_floor_name(
            path,
            &name_field,
            floor.name(),
            &declared,
            &mut seen,
            previous.as_deref(),
        )?;
        previous = Some(floor.name.clone());
        floors.push(floor);
    }
    Ok(floors)
}

fn parse_floor_row(
    path: &Path,
    index: usize,
    row: &toml::Value,
) -> Result<(Floor, String), BaselineError> {
    let row_field = format!("floor[{index}]");
    let row = row.as_table().ok_or_else(|| {
        BaselineError::Field(FieldError::type_mismatch(path, &row_field, "a floor table"))
    })?;
    reject_unknown_keys(path, &row_field, row, &["name", "value"]).map_err(BaselineError::Field)?;

    let name_field = format!("{row_field}.name");
    let name = required(path, row, &name_field)
        .map_err(BaselineError::Field)?
        .as_str()
        .ok_or_else(|| {
            BaselineError::Field(FieldError::type_mismatch(path, &name_field, "a string"))
        })?;
    let value_field = format!("{row_field}.value");
    let value = required(path, row, &value_field)
        .map_err(BaselineError::Field)?
        .as_integer()
        .and_then(|value| u64::try_from(value).ok())
        .ok_or_else(|| {
            BaselineError::Field(FieldError::type_mismatch(
                path,
                &value_field,
                "a non-negative integer",
            ))
        })?;
    Ok((
        Floor {
            name: name.to_owned(),
            value,
        },
        name_field,
    ))
}

fn validate_floor_name(
    path: &Path,
    field: &str,
    name: &str,
    declared: &BTreeSet<&str>,
    seen: &mut BTreeSet<String>,
    previous: Option<&str>,
) -> Result<(), BaselineError> {
    if !seen.insert(name.to_owned()) {
        return Err(BaselineError::Duplicate {
            path: path.to_path_buf(),
            field: field.to_owned(),
            name: name.to_owned(),
        });
    }
    if RETIRED_FLOORS.contains(&name) {
        return Err(BaselineError::Retired {
            path: path.to_path_buf(),
            field: field.to_owned(),
            name: name.to_owned(),
        });
    }
    if previous.is_some_and(|earlier| earlier > name) {
        return Err(BaselineError::Unsorted {
            path: path.to_path_buf(),
            field: field.to_owned(),
            previous: previous
                .expect("the comparison proved a previous floor")
                .to_owned(),
            name: name.to_owned(),
        });
    }
    if !declared.contains(name) {
        return Err(BaselineError::Unknown {
            path: path.to_path_buf(),
            field: field.to_owned(),
            name: name.to_owned(),
        });
    }
    Ok(())
}

fn require_all_floors(
    path: &Path,
    floors: &[Floor],
    gate_set: &GateSet,
) -> Result<(), BaselineError> {
    let present: BTreeSet<&str> = floors.iter().map(|floor| floor.name()).collect();
    if let Some(missing) = gate_set
        .floors
        .iter()
        .find(|floor| !present.contains(floor.name()))
    {
        return Err(BaselineError::Missing {
            path: path.to_path_buf(),
            field: "floor".to_owned(),
            name: missing.name.clone(),
        });
    }
    Ok(())
}

/// Identity of one compiled tool declaration and the executable bytes found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolIdentity {
    name: String,
    program: PathBuf,
    digest: Digest,
}

impl ToolIdentity {
    /// Stable compiled tool name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The one compiled executable path.
    #[must_use]
    pub fn program(&self) -> &Path {
        &self.program
    }

    /// Identity binding declaration and executable bytes.
    #[must_use]
    pub const fn digest(&self) -> &Digest {
        &self.digest
    }
}

/// Loaded current execution inputs, ready for one gate run.
#[derive(Debug)]
pub struct Contract {
    gate_set: GateSet,
    policy: Policy,
    baseline: Baseline,
    ratchet: Ratchet,
    bounds: Bounds,
    tools: Vec<ToolIdentity>,
}

impl Contract {
    /// Load the two fixed tracked files and preflight every compiled tool.
    ///
    /// # Errors
    ///
    /// Returns the first unreadable path, malformed field, invalid compiled
    /// declaration, or unavailable tool. No partial contract is returned.
    pub fn load(root: &Path) -> Result<Self, ContractError> {
        let policy_path = root.join(POLICY_PATH);
        let baseline_path = root.join(BASELINE_PATH);
        let policy_text = read_tracked(&policy_path)?;
        let baseline_text = read_tracked(&baseline_path)?;
        Self::from_tracked(
            root,
            &policy_path,
            &policy_text,
            &baseline_path,
            &baseline_text,
        )
    }

    /// Build from already selected tracked bytes while retaining their paths in
    /// every refusal.
    pub fn from_tracked(
        root: &Path,
        policy_path: &Path,
        policy_text: &str,
        baseline_path: &Path,
        baseline_text: &str,
    ) -> Result<Self, ContractError> {
        let gate_set = GateSet::current().map_err(ContractError::Metric)?;
        let bounds = current_bounds().map_err(ContractError::Bounds)?;
        let ignore = tracked_ignore(policy_path, policy_text).map_err(ContractError::Policy)?;
        let policy =
            Policy::new(ignore, gate_set.metrics.clone()).map_err(ContractError::Metric)?;
        let baseline = Baseline::parse(baseline_path, baseline_text, &gate_set)
            .map_err(ContractError::Baseline)?;
        let ratchet = baseline.ratchet(&gate_set);
        let tools = gate_set
            .tools
            .iter()
            .map(|tool| identify_tool(tool, root, &bounds))
            .collect::<Result<_, _>>()
            .map_err(ContractError::Tool)?;

        Ok(Self {
            gate_set,
            policy,
            baseline,
            ratchet,
            bounds,
            tools,
        })
    }

    /// Complete compiled gate set.
    #[must_use]
    pub const fn gate_set(&self) -> &GateSet {
        &self.gate_set
    }

    /// Effective compiled policy plus admitted tracked walk intent.
    #[must_use]
    pub const fn policy(&self) -> &Policy {
        &self.policy
    }

    /// Complete versioned floor state.
    #[must_use]
    pub const fn baseline(&self) -> &Baseline {
        &self.baseline
    }

    /// Parser-backed floors for the ordinary run.
    #[must_use]
    pub const fn ratchet(&self) -> &Ratchet {
        &self.ratchet
    }

    /// Shared declared process bounds.
    #[must_use]
    pub const fn bounds(&self) -> &Bounds {
        &self.bounds
    }

    /// Operational identities of every compiled tool.
    #[must_use]
    pub fn tools(&self) -> &[ToolIdentity] {
        &self.tools
    }
}

fn current_bounds() -> Result<Bounds, SpecError> {
    let deadlines = Deadlines::new(
        Duration::from_secs(30),
        Duration::from_secs(1),
        Duration::from_secs(1),
    )?;
    Bounds::new(
        "gate current tools",
        deadlines,
        Duration::from_millis(250),
        64 * 1024,
    )
}

fn read_tracked(path: &Path) -> Result<String, ContractError> {
    fs::read_to_string(path).map_err(|source| ContractError::Read {
        path: path.to_path_buf(),
        source,
    })
}

fn tracked_ignore(path: &Path, text: &str) -> Result<Ignore, PolicyError> {
    let document = text
        .parse::<toml::Table>()
        .map_err(|source| PolicyError::Toml {
            path: path.to_path_buf(),
            source,
        })?;
    let mut rules: Vec<String> = DEFAULT_SKIP.iter().map(|rule| (*rule).to_owned()).collect();
    let Some(gate) = document.get("gate") else {
        return parse_ignore(path, &rules);
    };
    let gate = gate
        .as_table()
        .ok_or_else(|| PolicyError::Field(FieldError::type_mismatch(path, "gate", "a table")))?;
    reject_unknown_keys(path, "gate", gate, &["walk"]).map_err(PolicyError::Field)?;

    let walk = required(path, gate, "gate.walk").map_err(PolicyError::Field)?;
    let walk = walk.as_table().ok_or_else(|| {
        PolicyError::Field(FieldError::type_mismatch(path, "gate.walk", "a table"))
    })?;
    reject_unknown_keys(path, "gate.walk", walk, &["skip"]).map_err(PolicyError::Field)?;

    let skip = required(path, walk, "gate.walk.skip").map_err(PolicyError::Field)?;
    let skip = skip.as_array().ok_or_else(|| {
        PolicyError::Field(FieldError::type_mismatch(
            path,
            "gate.walk.skip",
            "an array of strings",
        ))
    })?;

    let mut seen: BTreeSet<String> = rules.iter().cloned().collect();
    let mut previous: Option<&str> = None;
    for (index, value) in skip.iter().enumerate() {
        let field = format!("gate.walk.skip[{index}]");
        let value = value.as_str().ok_or_else(|| {
            PolicyError::Field(FieldError::type_mismatch(path, &field, "a string"))
        })?;
        if previous == Some(value) || !seen.insert(value.to_owned()) {
            return Err(PolicyError::Duplicate {
                path: path.to_path_buf(),
                field,
                value: value.to_owned(),
            });
        }
        if previous.is_some_and(|earlier| earlier > value) {
            return Err(PolicyError::Unsorted {
                path: path.to_path_buf(),
                field,
                previous: previous
                    .expect("comparison proved a previous skip")
                    .to_owned(),
                value: value.to_owned(),
            });
        }
        let Some(rule) = Rule::parse(value).map_err(|source| PolicyError::Selector {
            path: path.to_path_buf(),
            field: format!("gate.walk.skip[{index}]"),
            source,
        })?
        else {
            return Err(PolicyError::Value {
                path: path.to_path_buf(),
                field,
                reason: "must name one exclusion rule".to_owned(),
            });
        };
        if rule.negated() {
            return Err(PolicyError::Value {
                path: path.to_path_buf(),
                field,
                reason: "must exclude rather than re-admit".to_owned(),
            });
        }
        previous = Some(value);
        rules.push(value.to_owned());
    }

    parse_ignore(path, &rules)
}

fn parse_ignore(path: &Path, rules: &[String]) -> Result<Ignore, PolicyError> {
    let rendered = rules.join("\n") + "\n";
    Ignore::parse(&rendered).map_err(|source| PolicyError::Selector {
        path: path.to_path_buf(),
        field: "gate.walk.skip".to_owned(),
        source,
    })
}

fn required<'a>(
    path: &Path,
    table: &'a toml::Table,
    field: &str,
) -> Result<&'a toml::Value, FieldError> {
    let key = field.rsplit('.').next().unwrap_or(field);
    table
        .get(key)
        .ok_or_else(|| FieldError::missing(path, field))
}

fn reject_unknown_keys(
    path: &Path,
    prefix: &str,
    table: &toml::Table,
    admitted: &[&str],
) -> Result<(), FieldError> {
    if let Some(key) = table.keys().find(|key| !admitted.contains(&key.as_str())) {
        let field = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        return Err(FieldError::unknown(path, &field));
    }
    Ok(())
}

fn identify_tool(
    declaration: &ToolDeclaration,
    root: &Path,
    bounds: &Bounds,
) -> Result<ToolIdentity, ToolError> {
    let program = Path::new(&declaration.program);
    let bytes = fs::read(program).map_err(|source| ToolError::Read {
        name: declaration.name.clone(),
        program: program.to_path_buf(),
        source,
    })?;
    let binary = Digest::compute(Domain::GateTool, &bytes);
    let argv = Argv::new(
        declaration.program.clone(),
        declaration.probe_arguments.clone(),
    )
    .map_err(|source| ToolError::Spec {
        name: declaration.name.clone(),
        source,
    })?;
    let outcome = spawn(bounds.spec(argv, root))
        .map_err(|source| ToolError::Spawn {
            name: declaration.name.clone(),
            source,
        })?
        .wait()
        .map_err(|source| ToolError::Wait {
            name: declaration.name.clone(),
            source,
        })?;

    let Completion::Exited(status) = outcome.completion() else {
        return Err(ToolError::NonOperational {
            name: declaration.name.clone(),
            evidence: format!("probe completed as {:?}", outcome.completion()),
        });
    };
    if !status.success() {
        return Err(ToolError::NonOperational {
            name: declaration.name.clone(),
            evidence: format!("probe exited {status}"),
        });
    }
    let stdout = outcome.stdout().ok_or_else(|| ToolError::NonOperational {
        name: declaration.name.clone(),
        evidence: "probe returned no stdout".to_owned(),
    })?;
    let expected = format!("{}\n", declaration.probe_output);
    if stdout.truncated() || !stdout.complete() || stdout.bytes() != expected.as_bytes() {
        return Err(ToolError::NonOperational {
            name: declaration.name.clone(),
            evidence: format!(
                "probe stdout was {:?}, truncated={}, complete={}",
                String::from_utf8_lossy(stdout.bytes()),
                stdout.truncated(),
                stdout.complete()
            ),
        });
    }
    if let Some(stderr) = outcome.stderr()
        && (stderr.truncated() || !stderr.complete() || !stderr.bytes().is_empty())
    {
        return Err(ToolError::NonOperational {
            name: declaration.name.clone(),
            evidence: format!(
                "probe stderr was {:?}, truncated={}, complete={}",
                String::from_utf8_lossy(stderr.bytes()),
                stderr.truncated(),
                stderr.complete()
            ),
        });
    }

    let mut identity = String::new();
    declaration.render(&mut identity);
    let _ = writeln!(identity, "tool {} executable {binary}", declaration.name);
    Ok(ToolIdentity {
        name: declaration.name.clone(),
        program: program.to_path_buf(),
        digest: Digest::compute(Domain::GateTool, identity.as_bytes()),
    })
}

/// One exact tracked field refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldError {
    path: PathBuf,
    field: String,
    reason: String,
}

impl FieldError {
    fn missing(path: &Path, field: &str) -> Self {
        Self {
            path: path.to_path_buf(),
            field: field.to_owned(),
            reason: "is required".to_owned(),
        }
    }

    fn unknown(path: &Path, field: &str) -> Self {
        Self {
            path: path.to_path_buf(),
            field: field.to_owned(),
            reason: "is not an admitted field".to_owned(),
        }
    }

    fn type_mismatch(path: &Path, field: &str, expected: &str) -> Self {
        Self {
            path: path.to_path_buf(),
            field: field.to_owned(),
            reason: format!("must be {expected}"),
        }
    }

    /// Fixed tracked file that was refused.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Exact field path within the tracked document.
    #[must_use]
    pub fn field(&self) -> &str {
        &self.field
    }
}

impl fmt::Display for FieldError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} field {} {}",
            self.path.display(),
            self.field,
            self.reason
        )
    }
}

impl Error for FieldError {}

/// Why tracked gate intent did not produce one walk policy.
#[derive(Debug)]
pub enum PolicyError {
    /// The shared tracked document is not TOML.
    Toml {
        /// Fixed policy path.
        path: PathBuf,
        /// Parser refusal with source position.
        source: toml::de::Error,
    },
    /// A required, typed, or admitted field was violated.
    Field(FieldError),
    /// One skip repeats a compiled or tracked skip.
    Duplicate {
        /// Fixed policy path.
        path: PathBuf,
        /// Exact array element.
        field: String,
        /// Repeated rule.
        value: String,
    },
    /// Tracked skip entries are not in canonical order.
    Unsorted {
        /// Fixed policy path.
        path: PathBuf,
        /// Exact array element.
        field: String,
        /// Rule immediately above it.
        previous: String,
        /// Rule that sorted first.
        value: String,
    },
    /// One value has the right TOML type but no admitted meaning.
    Value {
        /// Fixed policy path.
        path: PathBuf,
        /// Exact field path.
        field: String,
        /// Refusal reason.
        reason: String,
    },
    /// One skip rule is not a current jurisdiction spelling.
    Selector {
        /// Fixed policy path.
        path: PathBuf,
        /// Exact field path.
        field: String,
        /// Jurisdiction refusal.
        source: IgnoreError,
    },
}

impl fmt::Display for PolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Toml { path, source } => {
                write!(formatter, "{} is not valid TOML: {source}", path.display())
            }
            Self::Field(error) => write!(formatter, "{error}"),
            Self::Duplicate { path, field, value } => write!(
                formatter,
                "{} field {field} repeats skip {value:?}",
                path.display()
            ),
            Self::Unsorted {
                path,
                field,
                previous,
                value,
            } => write!(
                formatter,
                "{} field {field} skip {value:?} sorts before {previous:?}",
                path.display()
            ),
            Self::Value {
                path,
                field,
                reason,
            } => write!(formatter, "{} field {field} {reason}", path.display()),
            Self::Selector {
                path,
                field,
                source,
            } => write!(formatter, "{} field {field}: {source}", path.display()),
        }
    }
}

impl Error for PolicyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Toml { source, .. } => Some(source),
            Self::Field(error) => Some(error),
            Self::Selector { source, .. } => Some(source),
            Self::Duplicate { .. } | Self::Unsorted { .. } | Self::Value { .. } => None,
        }
    }
}

/// Why the complete tracked baseline was refused.
#[derive(Debug)]
pub enum BaselineError {
    /// The file is not TOML.
    Toml {
        /// Fixed baseline path.
        path: PathBuf,
        /// Parser refusal with source position.
        source: toml::de::Error,
    },
    /// A required, typed, or admitted field was violated.
    Field(FieldError),
    /// The version is not executable by this build.
    UnsupportedVersion {
        /// Fixed baseline path.
        path: PathBuf,
        /// Exact version field.
        field: String,
        /// Version found.
        version: u64,
    },
    /// One current floor is absent.
    Missing {
        /// Fixed baseline path.
        path: PathBuf,
        /// Owning collection field.
        field: String,
        /// Missing current name.
        name: String,
    },
    /// One floor appears more than once.
    Duplicate {
        /// Fixed baseline path.
        path: PathBuf,
        /// Exact repeated name field.
        field: String,
        /// Repeated current name.
        name: String,
    },
    /// Floor rows are not in name order.
    Unsorted {
        /// Fixed baseline path.
        path: PathBuf,
        /// Exact offending name field.
        field: String,
        /// Name immediately above it.
        previous: String,
        /// Name that sorted first.
        name: String,
    },
    /// A removed floor name attempted to select old behavior.
    Retired {
        /// Fixed baseline path.
        path: PathBuf,
        /// Exact offending name field.
        field: String,
        /// Retired name.
        name: String,
    },
    /// A name outside the compiled current set was found.
    Unknown {
        /// Fixed baseline path.
        path: PathBuf,
        /// Exact offending name field.
        field: String,
        /// Unknown name.
        name: String,
    },
    /// The values were meaningful but not in the one admitted byte form.
    NonCanonical {
        /// Fixed baseline path.
        path: PathBuf,
        /// Whole-document field.
        field: String,
        /// Exact admitted replacement.
        canonical: String,
    },
}

impl fmt::Display for BaselineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Toml { path, source } => {
                write!(formatter, "{} is not valid TOML: {source}", path.display())
            }
            Self::Field(error) => write!(formatter, "{error}"),
            Self::UnsupportedVersion {
                path,
                field,
                version,
            } => write!(
                formatter,
                "{} field {field} names unsupported baseline version {version}",
                path.display()
            ),
            Self::Missing { path, field, name } => write!(
                formatter,
                "{} field {field} is missing current floor {name:?}",
                path.display()
            ),
            Self::Duplicate { path, field, name } => write!(
                formatter,
                "{} field {field} repeats floor {name:?}",
                path.display()
            ),
            Self::Unsorted {
                path,
                field,
                previous,
                name,
            } => write!(
                formatter,
                "{} field {field} floor {name:?} sorts before {previous:?}",
                path.display()
            ),
            Self::Retired { path, field, name } => write!(
                formatter,
                "{} field {field} names retired floor {name:?}",
                path.display()
            ),
            Self::Unknown { path, field, name } => write!(
                formatter,
                "{} field {field} names unknown floor {name:?}",
                path.display()
            ),
            Self::NonCanonical {
                path,
                field,
                canonical,
            } => write!(
                formatter,
                "{} field {field} is not canonical; expected {canonical:?}",
                path.display()
            ),
        }
    }
}

impl Error for BaselineError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Toml { source, .. } => Some(source),
            Self::Field(error) => Some(error),
            Self::UnsupportedVersion { .. }
            | Self::Missing { .. }
            | Self::Duplicate { .. }
            | Self::Unsorted { .. }
            | Self::Retired { .. }
            | Self::Unknown { .. }
            | Self::NonCanonical { .. } => None,
        }
    }
}

/// Why one compiled tool could not be identified as operational.
#[derive(Debug)]
pub enum ToolError {
    /// The one compiled executable path was unreadable.
    Read {
        /// Compiled tool name.
        name: String,
        /// Compiled executable path.
        program: PathBuf,
        /// Filesystem refusal.
        source: io::Error,
    },
    /// The probe argv was not a SILL command.
    Spec {
        /// Compiled tool name.
        name: String,
        /// SILL refusal.
        source: SpecError,
    },
    /// The executable could not be started.
    Spawn {
        /// Compiled tool name.
        name: String,
        /// Operating-system refusal.
        source: io::Error,
    },
    /// The bounded probe could not be completed and reaped.
    Wait {
        /// Compiled tool name.
        name: String,
        /// Bounded-wait refusal.
        source: WaitError,
    },
    /// The probe ran but did not answer its compiled contract.
    NonOperational {
        /// Compiled tool name.
        name: String,
        /// Exact observed outcome.
        evidence: String,
    },
}

impl fmt::Display for ToolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read {
                name,
                program,
                source,
            } => write!(
                formatter,
                "declared tool {name} at {} is unreadable: {source}",
                program.display()
            ),
            Self::Spec { name, source } => write!(formatter, "declared tool {name}: {source}"),
            Self::Spawn { name, source } => {
                write!(formatter, "declared tool {name} could not start: {source}")
            }
            Self::Wait { name, source } => {
                write!(formatter, "declared tool {name} did not complete: {source}")
            }
            Self::NonOperational { name, evidence } => {
                write!(
                    formatter,
                    "declared tool {name} is not operational: {evidence}"
                )
            }
        }
    }
}

impl Error for ToolError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read { source, .. } | Self::Spawn { source, .. } => Some(source),
            Self::Spec { source, .. } => Some(source),
            Self::Wait { source, .. } => Some(source),
            Self::NonOperational { .. } => None,
        }
    }
}

/// Why no complete current execution contract was loaded.
#[derive(Debug)]
pub enum ContractError {
    /// One of the two fixed tracked paths was unreadable.
    Read {
        /// Exact path.
        path: PathBuf,
        /// Filesystem refusal.
        source: io::Error,
    },
    /// Tracked gate intent was refused.
    Policy(PolicyError),
    /// Tracked lower-is-better state was refused.
    Baseline(BaselineError),
    /// A compiled metric declaration was internally invalid.
    Metric(MetricError),
    /// Compiled process bounds were internally invalid.
    Bounds(SpecError),
    /// A compiled tool was absent or non-operational.
    Tool(ToolError),
}

impl fmt::Display for ContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(
                    formatter,
                    "read tracked gate input {}: {source}",
                    path.display()
                )
            }
            Self::Policy(source) => write!(formatter, "gate policy: {source}"),
            Self::Baseline(source) => write!(formatter, "gate baseline: {source}"),
            Self::Metric(source) => write!(formatter, "compiled gate set: {source}"),
            Self::Bounds(source) => write!(formatter, "compiled gate bounds: {source}"),
            Self::Tool(source) => write!(formatter, "gate tool preflight: {source}"),
        }
    }
}

impl Error for ContractError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Policy(source) => Some(source),
            Self::Baseline(source) => Some(source),
            Self::Metric(source) => Some(source),
            Self::Bounds(source) => Some(source),
            Self::Tool(source) => Some(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_non_operational_compiled_tools_are_named_before_execution() {
        let root = workspace_root();
        let bounds = current_bounds().expect("compiled bounds are valid");
        let missing = ToolDeclaration {
            name: "missing".to_owned(),
            program: root.join("does-not-exist").display().to_string(),
            probe_arguments: Vec::new(),
            probe_output: "unused".to_owned(),
        };
        let refused = identify_tool(&missing, &root, &bounds).expect_err("missing tool refuses");
        assert!(matches!(
            refused,
            ToolError::Read { ref name, ref source, .. }
                if name == "missing" && source.kind() == io::ErrorKind::NotFound
        ));

        let non_operational = ToolDeclaration {
            name: "wrong-answer".to_owned(),
            program: "/bin/sh".to_owned(),
            probe_arguments: vec!["-c".to_owned(), "printf wrong".to_owned()],
            probe_output: "right".to_owned(),
        };
        let refused = identify_tool(&non_operational, &root, &bounds)
            .expect_err("a tool that answers another probe is not operational");
        assert!(matches!(
            refused,
            ToolError::NonOperational { ref name, ref evidence }
                if name == "wrong-answer" && evidence.contains("wrong")
        ));
    }

    #[test]
    fn executable_and_bound_declarations_move_their_own_identities() {
        let current = GateSet::current().expect("compiled gate set");
        let mut changed = current.clone();
        changed.tools[0].program = "/bin/awk".to_owned();
        assert_ne!(
            current.digest(),
            changed.digest(),
            "an executable declaration did not move the gate-set identity"
        );

        let bounds = current_bounds().expect("compiled bounds");
        let changed_bounds = Bounds::new(
            "gate current tools",
            bounds.deadlines(),
            Duration::from_millis(251),
            bounds.capture(),
        )
        .expect("changed bounds remain valid");
        assert_ne!(bounds.digest(), changed_bounds.digest());

        let first = identify_tool(&current.tools[0], &workspace_root(), &bounds)
            .expect("current tool answers its probe");
        let second = identify_tool(&current.tools[0], &workspace_root(), &bounds)
            .expect("same current tool answers again");
        assert_eq!(first, second, "unchanged executable bytes moved identity");
    }

    fn workspace_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("gate is a direct workspace member")
            .to_path_buf()
    }
}
