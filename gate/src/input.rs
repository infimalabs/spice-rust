//! One current, tracked input contract for executable gate runs.
//!
//! The repository contributes intent and lower-is-better state through two
//! fixed files. Everything capable of execution remains compiled here: metric
//! names, parser argv, tool probes, deadlines, grace, capture, and default walk
//! exclusions. A tracked byte can narrow jurisdiction or lower a floor, but it
//! cannot name a program or revive an old gate arm.

use std::collections::BTreeSet;
use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use sill::{Argv, Completion, Deadlines, SpecError, WaitError};
use wire::{Digest, Domain, TRACKED_POLICY_FILE};

use crate::camp::{Camp, CampError, SharedBound};
use crate::jurisdiction::{Ignore, IgnoreError};
use crate::kill::{
    CARGO_PROGRAM, Kill, KillBounds, KillError, KillRun, MUTANTS_PROGRAM, MutationBattery,
    MutationTool, NATIVE_BUILD_PROGRAMS, PINNED_CARGO_MUTANTS, RateError, SurvivorRate,
    check_identity,
};
use crate::measure::{Bounds, Metric, MetricError, Parser, Policy, spawn};
#[cfg(not(target_arch = "wasm32"))]
use crate::policy::{PACKAGED_GENERIC_POLICY, PackagePolicy};
use crate::policy::{Policy as RulePolicy, RepoTruthPolicy, parse_tracked_policy};
use crate::ratchet::Ratchet;

/// Fixed tracked lower-is-better state path within a selected repository tree.
pub const BASELINE_PATH: &str = "baseline.toml";
/// The one baseline schema this build executes.
pub const BASELINE_VERSION: u64 = 1;
/// Byte length above which one Rust source line contributes debt.
pub const LONG_LINE_LIMIT: usize = 100;

/// Wall-clock ceiling every process a gate run launches answers to.
///
/// One value governs the version probes, `cargo fmt --check`, every metric
/// parser run, and the app analyses alike, so it has to clear the slowest
/// workload it covers rather than the typical one. That workload is the
/// `app-web-typecheck` arm's `trunk build --release` against a cold
/// `target/xtask-app`. Five cold samples on a machine sharing eleven lanes, at
/// load averages of 18.8 to 24.2, ran 15.0, 17.2, 19.3, 19.8 and 24.9 seconds;
/// the same build warm ran 2.6 to 2.9, and the `cargo check` beside it ran 8.9
/// to 12.0 cold. The distribution is therefore bimodal, and a 30-second ceiling
/// spent 83% of itself on the worst cold sample — which is how a cold cache came
/// to refuse an otherwise clean battery as a timeout, on a tree nothing was
/// wrong with. This is five times that worst sample, so what decides an arm is
/// the arm, and the clock is left to catch only a process that has stopped
/// making progress at all.
const RUNTIME_DEADLINE: Duration = Duration::from_secs(120);

const AWK: &str = "/usr/bin/awk";
const AWK_PROBE: &str = "BEGIN { print \"spice-gate-awk-v1\" }";
const AWK_PROBE_OUTPUT: &str = "spice-gate-awk-v1";
const DEFAULT_SKIP: &[&str] = &[".git/", "target/"];
const RETIRED_FLOORS: &[(&str, u64)] = &[("rust.kill-rate", 1), ("rust.long-lines", 1)];
#[cfg(not(target_arch = "wasm32"))]
const PACKAGED_GENERIC_POLICY_SOURCE: &str = "gate packaged generic policy";

mod declaration;
pub use declaration::*;
mod baseline;
pub use baseline::*;

/// Identity of one compiled tool declaration and the executable bytes found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolIdentity {
    name: String,
    program: PathBuf,
    version: String,
    executable: Digest,
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

    /// Exactly what the declared probe printed, retained unmodified.
    ///
    /// Kept beside the digests because neither of them can be read back into
    /// what the tool said about itself. A probe that prints a version prints one
    /// here; a probe that prints a compiled sentinel prints that, and either way
    /// this is the tool's own answer rather than a name the gate chose for it.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Identity of the executable bytes alone.
    #[must_use]
    pub const fn executable(&self) -> &Digest {
        &self.executable
    }

    /// Identity binding declaration and executable bytes.
    #[must_use]
    pub const fn digest(&self) -> &Digest {
        &self.digest
    }
}

/// Parsed tracked inputs that contain no observation of external tools.
///
/// This type is suitable for in-process policy and acquisition evaluation. It
/// cannot produce a gate receipt: only [`Contract::load`] returns a complete
/// execution contract with operational tool identities.
#[derive(Debug)]
pub struct ParsedContract {
    gate_set: GateSet,
    policy: Policy,
    rule_policy: RulePolicy,
    #[cfg(not(target_arch = "wasm32"))]
    environment_names: Vec<String>,
    #[cfg(not(target_arch = "wasm32"))]
    tracked_configuration: toml::Value,
    #[cfg(not(target_arch = "wasm32"))]
    shape: ShapePolicy,
    camp_policy: Policy,
    kill_floor: Option<SurvivorRate>,
    baseline: Option<Baseline>,
    ratchet: Option<Ratchet>,
    bounds: Bounds,
    compiled_walk_rules: usize,
}

/// Loaded current execution inputs, ready for an ordinary gate run.
#[derive(Debug)]
pub struct Contract {
    parsed: ParsedContract,
    tools: Vec<ToolIdentity>,
}

impl std::ops::Deref for Contract {
    type Target = ParsedContract;

    fn deref(&self) -> &Self::Target {
        &self.parsed
    }
}

struct PolicyInputs<'a> {
    path: &'a Path,
    tracked_text: &'a str,
    rule_source: &'a str,
    rule_text: &'a str,
}

struct BaselineInputs<'a> {
    path: &'a Path,
    text: Option<&'a str>,
}

impl Contract {
    /// Load the tracked files and preflight every parser tool.
    ///
    /// The mutation executable is declared here but is attested inside a
    /// private selected tree immediately before mutation inventory begins.
    ///
    /// # Errors
    ///
    /// Returns the first unreadable path, malformed field, invalid compiled
    /// declaration, or unavailable tool. No partial contract is returned.
    pub fn load(root: &Path) -> Result<Self, ContractError> {
        let policy_path = root.join(TRACKED_POLICY_FILE);
        let baseline_path = root.join(BASELINE_PATH);
        let policy_text = read_tracked(&policy_path)?;
        let baseline_text = read_tracked(&baseline_path)?;
        let parsed = ParsedContract::from_inputs(
            PolicyInputs {
                path: &policy_path,
                tracked_text: &policy_text,
                rule_source: TRACKED_POLICY_FILE,
                rule_text: &policy_text,
            },
            BaselineInputs {
                path: &baseline_path,
                text: Some(&baseline_text),
            },
        )?;
        let tools = parsed
            .gate_set
            .tools
            .iter()
            .map(|tool| identify_tool(tool, root, &parsed.bounds))
            .collect::<Result<_, _>>()
            .map_err(ContractError::Tool)?;
        Ok(Self { parsed, tools })
    }

    /// Load the generic installed-builtin inputs for one selected repository.
    ///
    /// An absent policy is the typed empty repository layer: compiled package
    /// policy remains enabled and no repository value or declaration is
    /// invented. An absent baseline remains absent until a lower-is-better
    /// surface explicitly asks for it. Present files always take the same
    /// strict parsers as [`Self::load`], so malformed tracked bytes cannot turn
    /// into packaged defaults merely because the caller selected generic mode.
    ///
    /// Compiled legacy parser tools are not identified here. Battery-family
    /// tools are acquired by their selected arm after that arm has established
    /// applicability; the strict source-self-hosting loader above retains the
    /// exhaustive legacy probe.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn load_generic(root: &Path) -> Result<Self, ContractError> {
        let policy_path = root.join(TRACKED_POLICY_FILE);
        let baseline_path = root.join(BASELINE_PATH);
        let policy_text = read_optional_tracked(&policy_path)?;
        let baseline_text = read_optional_tracked(&baseline_path)?;
        let (tracked_policy_text, rule_policy_source, rule_policy_text) =
            match policy_text.as_deref() {
                Some(text) => (text, TRACKED_POLICY_FILE, text),
                None => ("", PACKAGED_GENERIC_POLICY_SOURCE, PACKAGED_GENERIC_POLICY),
            };
        let parsed = ParsedContract::from_inputs(
            PolicyInputs {
                path: &policy_path,
                tracked_text: tracked_policy_text,
                rule_source: rule_policy_source,
                rule_text: rule_policy_text,
            },
            BaselineInputs {
                path: &baseline_path,
                text: baseline_text.as_deref(),
            },
        )?;
        Ok(Self {
            parsed,
            tools: Vec::new(),
        })
    }
}

impl ParsedContract {
    /// Parse already selected tracked bytes while retaining their paths in
    /// every refusal.
    ///
    /// This boundary performs no tool execution and therefore does not return
    /// an execution-ready [`Contract`].
    pub fn from_tracked(
        policy_path: &Path,
        policy_text: &str,
        baseline_path: &Path,
        baseline_text: &str,
    ) -> Result<Self, ContractError> {
        Self::from_inputs(
            PolicyInputs {
                path: policy_path,
                tracked_text: policy_text,
                rule_source: TRACKED_POLICY_FILE,
                rule_text: policy_text,
            },
            BaselineInputs {
                path: baseline_path,
                text: Some(baseline_text),
            },
        )
    }

    fn from_inputs(
        policy: PolicyInputs<'_>,
        baseline: BaselineInputs<'_>,
    ) -> Result<Self, ContractError> {
        let gate_set = GateSet::current().map_err(ContractError::GateSet)?;
        let bounds = current_bounds().map_err(ContractError::Bounds)?;
        #[cfg(not(target_arch = "wasm32"))]
        let tracked_configuration = tracked_configuration(policy.path, policy.tracked_text)
            .map_err(ContractError::Policy)?;
        let tracked =
            tracked_policy(policy.path, policy.tracked_text).map_err(ContractError::Policy)?;
        let (ignore, compiled_walk_rules) =
            tracked_ignore(policy.path, &tracked).map_err(ContractError::Policy)?;
        #[cfg(not(target_arch = "wasm32"))]
        let shape = ShapePolicy::from_tracked(&tracked.policy);
        let rule_policy =
            parse_tracked_policy(policy.rule_source, policy.rule_text).map_err(|source| {
                ContractError::Policy(PolicyError::Rules {
                    path: policy.path.to_path_buf(),
                    source,
                })
            })?;
        let metrics: Vec<Metric> = gate_set
            .metrics
            .iter()
            .map(|declaration| declaration.metric.clone())
            .collect();
        let policy =
            Policy::tracked_index(ignore.clone(), metrics).map_err(ContractError::Metric)?;
        let camp_policy =
            Policy::new(ignore, gate_set.camp_metrics()).map_err(ContractError::Metric)?;
        let baseline = baseline
            .text
            .map(|text| Baseline::parse(baseline.path, text, &gate_set))
            .transpose()
            .map_err(ContractError::Baseline)?;
        let ratchet = baseline
            .as_ref()
            .map(|baseline| baseline.ratchet(&gate_set));
        let kill_floor = baseline.as_ref().map(Baseline::mutation_floor);

        Ok(Self {
            gate_set,
            policy,
            rule_policy,
            #[cfg(not(target_arch = "wasm32"))]
            environment_names: tracked.policy.env_names,
            #[cfg(not(target_arch = "wasm32"))]
            tracked_configuration,
            #[cfg(not(target_arch = "wasm32"))]
            shape,
            camp_policy,
            kill_floor,
            baseline,
            ratchet,
            bounds,
            compiled_walk_rules,
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

    /// Parsed tracked layer captured from the selected source.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) const fn tracked_configuration(&self) -> &toml::Value {
        &self.tracked_configuration
    }

    /// Compiled named rules used by structural family acquisition.
    #[must_use]
    pub const fn rule_policy(&self) -> &RulePolicy {
        &self.rule_policy
    }

    /// Exact environment-name vocabulary captured from tracked policy.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn environment_names(&self) -> &[String] {
        &self.environment_names
    }

    /// Admitted tracked intent for the repository-shape provider.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) const fn shape(&self) -> &ShapePolicy {
        &self.shape
    }

    /// Compiled shared-bound profile.
    #[must_use]
    pub const fn camp(&self) -> &Camp {
        self.gate_set.camp()
    }

    pub(crate) const fn camp_policy(&self) -> &Policy {
        &self.camp_policy
    }

    /// Execute the compiled mutation profile against a private selected tree.
    ///
    /// `rim` resolves the declared mutation programs on the running host, and
    /// `compiler_environment` is the environment their processes inherit.
    ///
    /// # Errors
    ///
    /// Returns the first selection, tool, inventory, baseline, or integrity
    /// refusal, including a declared program the rim does not resolve. The
    /// caller tree and tracked baseline are never mutation roots.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn execute_kill(
        &self,
        root: &Path,
        rim: &crate::toolchain::RimCapture,
        compiler_environment: &[(OsString, OsString)],
    ) -> Result<KillRun, KillError> {
        self.execute_kill_with_progress(root, rim, compiler_environment, &mut |_| {})
    }

    /// Execute the mutation profile while observing its canonical campaign.
    ///
    /// # Errors
    ///
    /// Returns the same failures as [`Self::execute_kill`].
    #[cfg(not(target_arch = "wasm32"))]
    pub fn execute_kill_with_progress(
        &self,
        root: &Path,
        rim: &crate::toolchain::RimCapture,
        compiler_environment: &[(OsString, OsString)],
        progress: &mut dyn FnMut(crate::KillProgress<'_>),
    ) -> Result<KillRun, KillError> {
        let floor = self.kill_floor.ok_or_else(|| KillError::MissingFloor {
            path: root.join(BASELINE_PATH),
            name: "rust.mutation-survivors",
        })?;
        self.gate_set
            .resolve_kill(rim, compiler_environment)?
            .run_with_progress(root, self.policy.ignore(), Some(floor), progress)
    }

    /// Complete versioned floor state.
    #[must_use]
    pub const fn baseline(&self) -> &Baseline {
        self.baseline
            .as_ref()
            .expect("a strict tracked contract carries baseline.toml")
    }

    /// Parser-backed floors for the ordinary run.
    #[must_use]
    pub const fn ratchet(&self) -> &Ratchet {
        self.ratchet
            .as_ref()
            .expect("a strict tracked contract carries baseline.toml")
    }

    /// Shared declared process bounds.
    #[must_use]
    pub const fn bounds(&self) -> &Bounds {
        &self.bounds
    }

    /// Number of leading [`policy`](Self::policy) walk rules that are compiled.
    ///
    /// Rules below this index came from `DEFAULT_SKIP`; rules at or above it are
    /// the tracked `gate.walk.skip` entries in declaration order. A deciding
    /// rule index is only attributable to a layer through this boundary.
    #[must_use]
    pub const fn compiled_walk_rules(&self) -> usize {
        self.compiled_walk_rules
    }
}

impl Contract {
    /// Operational identities of every compiled tool.
    #[must_use]
    pub fn tools(&self) -> &[ToolIdentity] {
        &self.tools
    }
}

pub(crate) fn current_bounds() -> Result<Bounds, SpecError> {
    let deadlines = Deadlines::new(
        RUNTIME_DEADLINE,
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

#[cfg(not(target_arch = "wasm32"))]
fn read_optional_tracked(path: &Path) -> Result<Option<String>, ContractError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ContractError::Read {
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct TrackedPolicy {
    gate: crate::config::Config,
    policy: TrackedPolicyTable,
}

/// The current `[policy]` table consumed by the compiled gate providers.
///
/// The repository-document inventory is admitted on every target because the
/// rule policy it feeds is compiled on every target. The shape fields are read
/// only by the native repository-shape provider.
#[derive(Default, Deserialize)]
#[serde(default)]
struct TrackedPolicyTable {
    repo_truth: RepoTruthPolicy,
    #[cfg(not(target_arch = "wasm32"))]
    env_names: Vec<String>,
    #[cfg(not(target_arch = "wasm32"))]
    package_roots: Vec<String>,
    #[cfg(not(target_arch = "wasm32"))]
    generated_paths: Vec<String>,
    #[cfg(not(target_arch = "wasm32"))]
    name_cluster_threshold: Option<usize>,
    #[cfg(not(target_arch = "wasm32"))]
    test_paths: Vec<String>,
    #[cfg(not(target_arch = "wasm32"))]
    package: PackagePolicy,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShapePolicy {
    package_roots: Vec<String>,
    generated_paths: Vec<String>,
    name_cluster_threshold: usize,
    test_paths: Vec<String>,
    boundary_underscore_pattern: Option<String>,
}

#[cfg(not(target_arch = "wasm32"))]
impl ShapePolicy {
    fn from_tracked(tracked: &TrackedPolicyTable) -> Self {
        let normalize = |values: &[String]| {
            let mut values: Vec<String> = values
                .iter()
                .map(|value| value.trim().trim_matches('/').to_owned())
                .filter(|value| !value.is_empty())
                .collect();
            values.sort();
            values.dedup();
            values
        };
        Self {
            package_roots: normalize(&tracked.package_roots),
            generated_paths: normalize(&tracked.generated_paths),
            name_cluster_threshold: tracked.name_cluster_threshold.unwrap_or(4),
            test_paths: normalize(&tracked.test_paths),
            boundary_underscore_pattern: tracked
                .package
                .boundary_underscore_pattern()
                .map(|pattern| pattern.trim().to_owned()),
        }
    }

    pub(crate) fn package_roots(&self) -> &[String] {
        &self.package_roots
    }

    pub(crate) fn generated_paths(&self) -> &[String] {
        &self.generated_paths
    }

    pub(crate) const fn name_cluster_threshold(&self) -> usize {
        self.name_cluster_threshold
    }

    /// Tracked test-root patterns, empty when the repository declares none.
    ///
    /// Emptiness is not a default: the provider still has its own
    /// remaining derivation to run before it settles on `tests`, and a policy
    /// that answered here would hide the pytest layer that sits between them.
    pub(crate) fn test_paths(&self) -> &[String] {
        &self.test_paths
    }

    /// Tracked module-name shape, present exactly when the repository set one.
    ///
    /// The packaged default is not substituted here. A key that is present and
    /// blank is a different fact than a key nobody wrote, and only the consumer
    /// that compiles the pattern can refuse the first while admitting the
    /// second.
    pub(crate) fn boundary_underscore_pattern(&self) -> Option<&str> {
        self.boundary_underscore_pattern.as_deref()
    }
}

fn tracked_policy(path: &Path, text: &str) -> Result<TrackedPolicy, PolicyError> {
    toml::from_str::<TrackedPolicy>(text).map_err(|source| PolicyError::Toml {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn tracked_configuration(path: &Path, text: &str) -> Result<toml::Value, PolicyError> {
    toml::from_str(text).map_err(|source| PolicyError::Toml {
        path: path.to_path_buf(),
        source,
    })
}

/// Compose the walk policy and report how many of its rules are compiled.
///
/// The count is returned rather than recovered later from `DEFAULT_SKIP`,
/// because this is the only place that knows it: everything downstream sees a
/// single flat rule list, and an index into it means nothing without the point
/// where the compiled defaults stop and `gate.walk.skip` begins. Evidence that
/// names which layer excluded a path is built from exactly that boundary.
fn tracked_ignore(path: &Path, tracked: &TrackedPolicy) -> Result<(Ignore, usize), PolicyError> {
    let defaults = Ignore::parse(&(DEFAULT_SKIP.join("\n") + "\n")).map_err(|source| {
        PolicyError::Selector {
            path: path.to_path_buf(),
            field: "compiled gate.walk.skip".to_owned(),
            source,
        }
    })?;
    let mut rules = defaults.rules().to_vec();
    let compiled = rules.len();
    rules.extend_from_slice(tracked.gate.walk().skip().rules());
    Ok((Ignore::from_rules(rules), compiled))
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
        // The probe's own bytes, admitted above as byte-equal to the declared
        // output, so this is what the tool printed rather than what was expected.
        version: declaration.probe_output.clone(),
        executable: binary,
        digest: Digest::compute(Domain::GateTool, identity.as_bytes()),
    })
}

mod errors;
pub use errors::*;
