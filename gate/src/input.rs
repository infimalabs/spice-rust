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

use serde::Deserialize;
use sill::{Argv, Completion, Deadlines, Environment, SpecError, WaitError, spawn};
use wire::{Digest, Domain};

use crate::camp::{Camp, CampError, SharedBound};
use crate::jurisdiction::{Ignore, IgnoreError};
use crate::kill::{
    Kill, KillBounds, KillError, KillRun, MutationBattery, MutationTool, PINNED_CARGO_MUTANTS,
    RateError, SurvivorRate,
};
use crate::measure::{Bounds, Metric, MetricError, Parser, Policy};
use crate::policy::{Policy as RulePolicy, packaged_rule_policy};
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

include!("input/declaration.rs");
include!("input/baseline.rs");

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

/// The complete mutation execution profile selected by one current contract.
#[derive(Debug)]
pub struct KillProfile {
    runner: Kill,
    floor: SurvivorRate,
}

impl KillProfile {
    /// Direct executables and explicit environment.
    #[must_use]
    pub const fn tool(&self) -> &MutationTool {
        self.runner.tool()
    }

    /// Fixed targets, retries, and seed.
    #[must_use]
    pub const fn battery(&self) -> &MutationBattery {
        self.runner.battery()
    }

    /// Fixed process deadlines, grace, and capture.
    #[must_use]
    pub const fn bounds(&self) -> &KillBounds {
        self.runner.bounds()
    }

    /// Exact committed survivor-rate floor.
    #[must_use]
    pub const fn floor(&self) -> SurvivorRate {
        self.floor
    }
}

/// Loaded current execution inputs, ready for ordinary, Camp, or Kill runs.
#[derive(Debug)]
pub struct Contract {
    gate_set: GateSet,
    policy: Policy,
    rule_policy: RulePolicy,
    camp_policy: Policy,
    kill_profile: KillProfile,
    baseline: Baseline,
    ratchet: Ratchet,
    bounds: Bounds,
    tools: Vec<ToolIdentity>,
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
        let gate_set = GateSet::current().map_err(ContractError::GateSet)?;
        let bounds = current_bounds().map_err(ContractError::Bounds)?;
        let ignore = tracked_ignore(policy_path, policy_text).map_err(ContractError::Policy)?;
        let rule_policy = packaged_rule_policy().map_err(|source| {
            ContractError::Policy(PolicyError::Rules {
                path: policy_path.to_path_buf(),
                source,
            })
        })?;
        let metrics: Vec<Metric> = gate_set
            .metrics
            .iter()
            .map(|declaration| declaration.metric.clone())
            .collect();
        let policy = Policy::new(ignore.clone(), metrics).map_err(ContractError::Metric)?;
        let camp_policy =
            Policy::new(ignore, gate_set.camp_metrics()).map_err(ContractError::Metric)?;
        let baseline = Baseline::parse(baseline_path, baseline_text, &gate_set)
            .map_err(ContractError::Baseline)?;
        let ratchet = baseline.ratchet(&gate_set);
        let kill_profile = KillProfile {
            runner: gate_set.build_kill(),
            floor: baseline.mutation_floor(),
        };
        let tools = gate_set
            .tools
            .iter()
            .map(|tool| identify_tool(tool, root, &bounds))
            .collect::<Result<_, _>>()
            .map_err(ContractError::Tool)?;

        Ok(Self {
            gate_set,
            policy,
            rule_policy,
            camp_policy,
            kill_profile,
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

    /// Compiled named rules used by structural family acquisition.
    #[must_use]
    pub const fn rule_policy(&self) -> &RulePolicy {
        &self.rule_policy
    }

    /// Compiled shared-bound profile.
    #[must_use]
    pub const fn camp(&self) -> &Camp {
        self.gate_set.camp()
    }

    pub(crate) const fn camp_policy(&self) -> &Policy {
        &self.camp_policy
    }

    /// Complete compiled mutation execution profile and committed floor.
    #[must_use]
    pub const fn kill(&self) -> &KillProfile {
        &self.kill_profile
    }

    /// Execute the compiled mutation profile against a private selected tree.
    ///
    /// # Errors
    ///
    /// Returns the first selection, tool, inventory, baseline, or integrity
    /// refusal. The caller tree and tracked baseline are never mutation roots.
    pub fn execute_kill(&self, root: &Path) -> Result<KillRun, KillError> {
        self.kill_profile
            .runner
            .run(root, self.policy.ignore(), Some(self.kill_profile.floor))
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

#[derive(Default, Deserialize)]
#[serde(default)]
struct TrackedPolicy {
    gate: crate::config::Config,
}

fn tracked_ignore(path: &Path, text: &str) -> Result<Ignore, PolicyError> {
    let tracked = toml::from_str::<TrackedPolicy>(text).map_err(|source| PolicyError::Toml {
        path: path.to_path_buf(),
        source,
    })?;
    let defaults = Ignore::parse(&(DEFAULT_SKIP.join("\n") + "\n")).map_err(|source| {
        PolicyError::Selector {
            path: path.to_path_buf(),
            field: "compiled gate.walk.skip".to_owned(),
            source,
        }
    })?;
    let mut rules = defaults.rules().to_vec();
    rules.extend_from_slice(tracked.gate.walk().skip().rules());
    Ok(Ignore::from_rules(rules))
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

include!("input/errors.rs");
include!("input/fixtures.rs");
