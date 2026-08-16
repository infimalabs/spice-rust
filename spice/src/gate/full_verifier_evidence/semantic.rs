//! Per-arm semantic evidence retained inside one full-verifier bundle.

use std::collections::{BTreeMap, BTreeSet};

use ::gate::{
    BatteryArmResult, BatteryContext, BatteryReceipt, BoundEvidence, GateId, GateSemanticClosure,
    LatchEvidence, StateReceipt, VerifierCompilerEvidence, semantic_verifier_compiler_authorities,
};
use serde::{Deserialize, Serialize};
use wire::{Digest, Domain};

use super::{compiler_authority, merge_compiler_authority};
use crate::gate::PreparedBattery;
use crate::repository::Repository;

const SEMANTIC_EVIDENCE_VERSION: u64 = 1;
const MERGE_SEMANTIC_EVIDENCE_VERSION: u64 = 1;

/// Canonical per-arm keys and result identities from one complete verifier.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SemanticEvidence {
    version: u64,
    closure: GateSemanticClosure,
    arms: Vec<SemanticArmReceipt>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct SemanticArmReceipt {
    gate_id: GateId,
    authority: Digest,
    result: Digest,
}

impl SemanticEvidence {
    pub(super) fn mint(
        repository: &Repository,
        closure: GateSemanticClosure,
        battery: &BatteryReceipt,
        compiler: &[VerifierCompilerEvidence],
    ) -> Result<Self, String> {
        if closure.source() != battery.tree() {
            return Err("semantic closure names a different full-verifier source".to_owned());
        }
        let compiler = compiler
            .iter()
            .map(|evidence| (evidence.gate_id(), evidence.semantic_authority()))
            .collect::<BTreeMap<_, _>>();
        let arms = battery
            .run()
            .results()
            .iter()
            .map(|result| {
                let gate_id = result.gate_id();
                let source = closure
                    .key(gate_id)
                    .ok_or_else(|| format!("semantic closure omitted {}", gate_id.as_str()))?;
                Ok(SemanticArmReceipt {
                    gate_id,
                    authority: arm_authority(
                        repository,
                        gate_id,
                        source,
                        battery.run().context(),
                        compiler.get(&gate_id),
                    )?,
                    result: result_identity(result)?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let evidence = Self {
            version: SEMANTIC_EVIDENCE_VERSION,
            closure,
            arms,
        };
        evidence.validate_inventory()?;
        Ok(evidence)
    }

    pub(super) fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        wire::encode(self)
    }

    pub(super) fn decode(document: &[u8]) -> Result<Self, String> {
        let evidence: Self = wire::decode(document)
            .map_err(|error| format!("decode semantic verifier evidence: {error}"))?;
        evidence.validate_inventory()?;
        let canonical = evidence
            .encode()
            .map_err(|error| format!("encode semantic verifier evidence: {error}"))?;
        if canonical != document {
            return Err("semantic verifier evidence is not canonical".to_owned());
        }
        Ok(evidence)
    }

    pub(super) fn validate_exact(
        &self,
        repository: &Repository,
        battery: &BatteryReceipt,
        compiler: &[VerifierCompilerEvidence],
        current: &GateSemanticClosure,
    ) -> Result<(), String> {
        if &self.closure != current {
            return Err(
                "semantic verifier closure disagrees with the fresh exact-source capture"
                    .to_owned(),
            );
        }
        let expected = Self::mint(repository, self.closure.clone(), battery, compiler)?;
        if self != &expected {
            return Err(
                "semantic verifier evidence disagrees with its exact battery or compiler receipt"
                    .to_owned(),
            );
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn admit(
        &self,
        repository: &Repository,
        prepared: &mut PreparedBattery<'_>,
        cargo_jobs: usize,
        battery_document: &[u8],
        compiler_document: &[u8],
        state_document: &[u8],
        evidence: Digest,
    ) -> Result<AdmittedSemanticVerifierEvidence, String> {
        self.validate_inventory()?;
        let state = StateReceipt::decode(state_document)
            .map_err(|error| format!("semantic state receipt refused: {error}"))?;
        let battery = prepared
            .admit_battery_receipt_for_semantic_comparison(battery_document)
            .map_err(|error| error.to_string())?;
        if !state.accepted() {
            return Err("semantic evidence does not carry an accepted state transition".to_owned());
        }
        if self.closure.source() != battery.tree() {
            return Err("semantic evidence closure does not match its battery source".to_owned());
        }
        let retained_compiler = semantic_verifier_compiler_authorities(
            compiler_document,
            battery.tree(),
            &compiler_authority(repository),
        )?
        .into_iter()
        .collect::<BTreeMap<_, _>>();
        let current_compiler = prepared
            .semantic_compiler_authorities(cargo_jobs)
            .map_err(|error| error.to_string())?
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        let current_context = prepared.context().map_err(|error| error.to_string())?;
        let current_closure = GateSemanticClosure::capture(
            prepared.source().clone(),
            prepared.root(),
            prepared.paths(),
            prepared.changes(),
            prepared.generated_paths(),
        )?;
        let comparison = self.closure.compare(&current_closure)?;
        let results = battery
            .run()
            .results()
            .iter()
            .map(|result| (result.gate_id(), result))
            .collect::<BTreeMap<_, _>>();
        let receipts = self
            .arms
            .iter()
            .map(|receipt| (receipt.gate_id, receipt))
            .collect::<BTreeMap<_, _>>();
        let mut reusable = Vec::new();
        for gate_id in comparison.reusable() {
            let result = results
                .get(gate_id)
                .ok_or_else(|| format!("semantic battery omitted {}", gate_id.as_str()))?;
            let receipt = receipts
                .get(gate_id)
                .ok_or_else(|| format!("semantic receipt omitted {}", gate_id.as_str()))?;
            if !result.state().admits() || receipt.result != result_identity(result)? {
                return Err(format!(
                    "semantic result authority for {} is invalid",
                    gate_id.as_str(),
                ));
            }
            let retained = arm_authority(
                repository,
                *gate_id,
                self.closure
                    .key(*gate_id)
                    .expect("validated closure covers every arm"),
                battery.run().context(),
                retained_compiler.get(gate_id),
            )?;
            let current = arm_authority(
                repository,
                *gate_id,
                current_closure
                    .key(*gate_id)
                    .expect("validated closure covers every arm"),
                &current_context,
                current_compiler.get(gate_id),
            )?;
            if receipt.authority == retained && receipt.authority == current {
                reusable.push(AdmittedSemanticArm {
                    result: (*result).clone(),
                    authority: receipt.authority.clone(),
                });
            }
        }
        Ok(AdmittedSemanticVerifierEvidence {
            source: current_closure.source().clone(),
            context: current_context,
            evidence,
            reusable,
            invalidated: comparison.invalidated().to_vec(),
            changed_sources: comparison.changed_sources().to_vec(),
            diagnostics: comparison.diagnostics().to_vec(),
        })
    }

    fn validate_inventory(&self) -> Result<(), String> {
        if self.version != SEMANTIC_EVIDENCE_VERSION {
            return Err(format!(
                "unsupported semantic verifier evidence version {}; expected {SEMANTIC_EVIDENCE_VERSION}",
                self.version,
            ));
        }
        let gate_ids = self.arms.iter().map(|arm| arm.gate_id).collect::<Vec<_>>();
        validate_inventory(&self.closure, &gate_ids)
    }
}

/// Canonical repository-portable arm authorities for one admitted parent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MergeSemanticEvidence {
    version: u64,
    repository: Digest,
    worktree: Digest,
    closure: GateSemanticClosure,
    arms: Vec<MergeSemanticArmReceipt>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct MergeSemanticArmReceipt {
    gate_id: GateId,
    authority: Digest,
    merge_authority: Digest,
    result: Digest,
}

impl MergeSemanticEvidence {
    pub(super) fn mint(
        repository: &Repository,
        closure: GateSemanticClosure,
        battery: &BatteryReceipt,
        compiler: &[VerifierCompilerEvidence],
    ) -> Result<Self, String> {
        if closure.source() != battery.tree() {
            return Err("semantic closure names a different full-verifier source".to_owned());
        }
        let repository_authority = repository_authority(repository);
        let compiler = compiler
            .iter()
            .map(|evidence| {
                (
                    evidence.gate_id(),
                    (
                        evidence.semantic_authority(),
                        evidence.merge_semantic_authority(&repository_authority),
                    ),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let arms = battery
            .run()
            .results()
            .iter()
            .map(|result| {
                let gate_id = result.gate_id();
                let source = closure
                    .key(gate_id)
                    .ok_or_else(|| format!("semantic closure omitted {}", gate_id.as_str()))?;
                Ok(MergeSemanticArmReceipt {
                    gate_id,
                    authority: arm_authority(
                        repository,
                        gate_id,
                        source,
                        battery.run().context(),
                        compiler.get(&gate_id).map(|authority| &authority.0),
                    )?,
                    merge_authority: merge_arm_authority(
                        &repository_authority,
                        gate_id,
                        source,
                        battery.run().context(),
                        compiler.get(&gate_id).map(|authority| &authority.1),
                    )?,
                    result: result_identity(result)?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let evidence = Self {
            version: MERGE_SEMANTIC_EVIDENCE_VERSION,
            repository: repository_authority,
            worktree: worktree_authority(repository),
            closure,
            arms,
        };
        evidence.validate_inventory()?;
        Ok(evidence)
    }

    pub(super) fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        wire::encode(self)
    }

    pub(super) fn decode(document: &[u8]) -> Result<Self, String> {
        let evidence: Self = wire::decode(document)
            .map_err(|error| format!("decode merge semantic evidence: {error}"))?;
        evidence.validate_inventory()?;
        let canonical = evidence
            .encode()
            .map_err(|error| format!("encode merge semantic evidence: {error}"))?;
        if canonical != document {
            return Err("merge semantic evidence is not canonical".to_owned());
        }
        Ok(evidence)
    }

    pub(super) const fn source(&self) -> &Digest {
        self.closure.source()
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn admit_merge_parent(
        &self,
        repository: &Repository,
        prepared_parent: &mut PreparedBattery<'_>,
        current_closure: &GateSemanticClosure,
        current_context: &BatteryContext,
        current_compiler: &BTreeMap<GateId, Digest>,
        cargo_jobs: usize,
        battery_document: &[u8],
        compiler_document: &[u8],
        state_document: &[u8],
        evidence: Digest,
    ) -> Result<AdmittedSemanticVerifierEvidence, String> {
        self.validate_inventory()?;
        let repository_authority = repository_authority(repository);
        if self.repository != repository_authority {
            return Err("semantic parent evidence names another repository".to_owned());
        }
        let state = StateReceipt::decode(state_document)
            .map_err(|error| format!("semantic parent state receipt refused: {error}"))?;
        if !state.accepted() {
            return Err("semantic parent evidence lacks an accepted state transition".to_owned());
        }
        let battery = prepared_parent
            .admit_battery_receipt_for_merge_parent_comparison(battery_document)
            .map_err(|error| error.to_string())?;
        if self.closure.source() != battery.tree() {
            return Err("semantic parent closure does not match its battery source".to_owned());
        }
        let retained_compiler = semantic_verifier_compiler_authorities(
            compiler_document,
            battery.tree(),
            &merge_compiler_authority(repository),
        )?
        .into_iter()
        .collect::<BTreeMap<_, _>>();
        let parent_closure = GateSemanticClosure::capture(
            prepared_parent.source().clone(),
            prepared_parent.root(),
            prepared_parent.paths(),
            prepared_parent.changes(),
            prepared_parent.generated_paths(),
        )?;
        if self.closure != parent_closure {
            return Err("semantic parent closure is not exact for its Git parent".to_owned());
        }
        let parent_compiler = prepared_parent
            .merge_semantic_compiler_authorities(cargo_jobs, &repository_authority)
            .map_err(|error| error.to_string())?
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        let parent_context = prepared_parent
            .context()
            .map_err(|error| error.to_string())?;
        let comparison = self.closure.compare(current_closure)?;
        let results = battery
            .run()
            .results()
            .iter()
            .map(|result| (result.gate_id(), result))
            .collect::<BTreeMap<_, _>>();
        let receipts = self
            .arms
            .iter()
            .map(|receipt| (receipt.gate_id, receipt))
            .collect::<BTreeMap<_, _>>();
        let reusable = GateId::BATTERY
            .into_iter()
            .map(|gate_id| {
                self.admit_merge_arm(
                    gate_id,
                    &repository_authority,
                    &battery,
                    &results,
                    &receipts,
                    &retained_compiler,
                    &parent_context,
                    &parent_compiler,
                    &comparison,
                    current_closure,
                    current_context,
                    current_compiler,
                )
            })
            .collect::<Result<Vec<_>, String>>()?
            .into_iter()
            .flatten()
            .collect();
        Ok(AdmittedSemanticVerifierEvidence {
            source: current_closure.source().clone(),
            context: current_context.clone(),
            evidence,
            reusable,
            invalidated: comparison.invalidated().to_vec(),
            changed_sources: comparison.changed_sources().to_vec(),
            diagnostics: comparison.diagnostics().to_vec(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn admit_merge_arm(
        &self,
        gate_id: GateId,
        repository_authority: &Digest,
        battery: &BatteryReceipt,
        results: &BTreeMap<GateId, &BatteryArmResult>,
        receipts: &BTreeMap<GateId, &MergeSemanticArmReceipt>,
        retained_compiler: &BTreeMap<GateId, Digest>,
        parent_context: &BatteryContext,
        parent_compiler: &BTreeMap<GateId, Digest>,
        comparison: &::gate::GateClosureComparison,
        current_closure: &GateSemanticClosure,
        current_context: &BatteryContext,
        current_compiler: &BTreeMap<GateId, Digest>,
    ) -> Result<Option<AdmittedSemanticArm>, String> {
        let result = results
            .get(&gate_id)
            .ok_or_else(|| format!("semantic parent battery omitted {}", gate_id.as_str()))?;
        let receipt = receipts
            .get(&gate_id)
            .ok_or_else(|| format!("semantic parent receipt omitted {}", gate_id.as_str()))?;
        if !result.state().admits() || receipt.result != result_identity(result)? {
            return Err(format!(
                "semantic parent result authority for {} is invalid",
                gate_id.as_str(),
            ));
        }
        let source = self
            .closure
            .key(gate_id)
            .expect("validated closure covers every arm");
        let retained_exact = arm_authority_for(
            &self.worktree,
            gate_id,
            source,
            battery.run().context(),
            retained_compiler.get(&gate_id),
        )?;
        if receipt.authority != retained_exact {
            return Err(format!(
                "semantic parent exact authority for {} is invalid",
                gate_id.as_str(),
            ));
        }
        let retained = merge_arm_authority(
            repository_authority,
            gate_id,
            source,
            parent_context,
            parent_compiler.get(&gate_id),
        )?;
        if receipt.merge_authority != retained {
            return Err(format!(
                "semantic parent authority for {} no longer matches its parent",
                gate_id.as_str(),
            ));
        }
        if comparison.reusable().binary_search(&gate_id).is_err() {
            return Ok(None);
        }
        let current = merge_arm_authority(
            repository_authority,
            gate_id,
            current_closure
                .key(gate_id)
                .expect("validated closure covers every arm"),
            current_context,
            current_compiler.get(&gate_id),
        )?;
        Ok(
            (receipt.merge_authority == current).then(|| AdmittedSemanticArm {
                result: (*result).clone(),
                authority: receipt.merge_authority.clone(),
            }),
        )
    }

    fn validate_inventory(&self) -> Result<(), String> {
        if self.version != MERGE_SEMANTIC_EVIDENCE_VERSION {
            return Err(format!(
                "unsupported merge semantic evidence version {}; expected {MERGE_SEMANTIC_EVIDENCE_VERSION}",
                self.version,
            ));
        }
        let gate_ids = self
            .arms
            .iter()
            .map(|receipt| receipt.gate_id)
            .collect::<Vec<_>>();
        validate_inventory(&self.closure, &gate_ids)
    }
}

fn validate_inventory(closure: &GateSemanticClosure, gate_ids: &[GateId]) -> Result<(), String> {
    let encoded = closure
        .encode()
        .map_err(|error| format!("encode semantic source closure: {error}"))?;
    GateSemanticClosure::decode(&encoded)?;
    if gate_ids != GateId::BATTERY {
        return Err("semantic verifier evidence must cover every canonical arm once".to_owned());
    }
    Ok(())
}

/// Result of probing source-mismatched full-verifier evidence.
pub enum SemanticFullVerifierAdmission {
    /// No bundle can safely prove an arm; ordinary selection and execution remain authoritative.
    Unavailable { detail: String },
    /// At least zero individually compared arms, plus exact invalidation evidence.
    Admitted(Box<AdmittedSemanticVerifierEvidence>),
}

impl SemanticFullVerifierAdmission {
    /// Diagnostic explaining why no semantic evidence was admitted.
    #[must_use]
    pub fn unavailable_detail(&self) -> Option<&str> {
        match self {
            Self::Unavailable { detail } => Some(detail),
            Self::Admitted(_) => None,
        }
    }
}

/// Per-arm green evidence matched to one fresh strict selected tree.
pub struct AdmittedSemanticVerifierEvidence {
    source: Digest,
    context: BatteryContext,
    evidence: Digest,
    reusable: Vec<AdmittedSemanticArm>,
    invalidated: Vec<GateId>,
    changed_sources: Vec<String>,
    diagnostics: Vec<String>,
}

struct AdmittedSemanticArm {
    result: BatteryArmResult,
    authority: Digest,
}

impl AdmittedSemanticVerifierEvidence {
    pub(super) fn compose_merge(parents: Vec<Self>) -> Result<Self, String> {
        if parents.len() != 2 {
            return Err("semantic merge composition requires exactly two parents".to_owned());
        }
        let source = parents[0].source.clone();
        let context = parents[0].context.clone();
        if parents
            .iter()
            .any(|parent| parent.source != source || parent.context != context)
        {
            return Err("semantic merge parents disagree about the current capture".to_owned());
        }
        let mut reusable = Vec::new();
        for gate_id in GateId::BATTERY {
            let candidates = parents
                .iter()
                .filter_map(|parent| {
                    parent
                        .reusable
                        .iter()
                        .find(|arm| arm.result.gate_id() == gate_id)
                })
                .collect::<Vec<_>>();
            let Some(first) = candidates.first() else {
                continue;
            };
            if candidates
                .iter()
                .any(|candidate| candidate.authority != first.authority)
            {
                return Err(format!(
                    "semantic merge parents disagree about reusable {} evidence",
                    gate_id.as_str(),
                ));
            }
            reusable.push(AdmittedSemanticArm {
                result: first.result.clone(),
                authority: first.authority.clone(),
            });
        }
        let reusable_ids = reusable
            .iter()
            .map(|arm| arm.result.gate_id())
            .collect::<BTreeSet<_>>();
        let invalidated = GateId::BATTERY
            .into_iter()
            .filter(|gate_id| !reusable_ids.contains(gate_id))
            .collect();
        let changed_sources = parents
            .iter()
            .flat_map(|parent| parent.changed_sources.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let diagnostics = parents
            .iter()
            .flat_map(|parent| parent.diagnostics.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut evidence_bytes = Vec::new();
        for parent in &parents {
            evidence_bytes.extend_from_slice(parent.evidence.as_str().as_bytes());
        }
        Ok(Self {
            source,
            context,
            evidence: Digest::compute(Domain::GateReceipt, &evidence_bytes),
            reusable,
            invalidated,
            changed_sources,
            diagnostics,
        })
    }

    /// Current selected source independently analyzed by the consumer.
    #[must_use]
    pub const fn source(&self) -> &Digest {
        &self.source
    }

    /// Fresh complete authority context used for every per-arm comparison.
    #[must_use]
    pub const fn context(&self) -> &BatteryContext {
        &self.context
    }

    /// Identity of the canonical containing bundle.
    #[must_use]
    pub const fn digest(&self) -> &Digest {
        &self.evidence
    }

    /// Reusable green result for one selected arm.
    #[must_use]
    pub fn result(&self, gate_id: GateId) -> Option<&BatteryArmResult> {
        self.reusable
            .binary_search_by_key(&gate_id, |arm| arm.result.gate_id())
            .ok()
            .map(|index| &self.reusable[index].result)
    }

    /// Current semantic input-and-contract authority for one reusable arm.
    #[must_use]
    pub fn authority(&self, gate_id: GateId) -> Option<&Digest> {
        self.reusable
            .binary_search_by_key(&gate_id, |arm| arm.result.gate_id())
            .ok()
            .map(|index| &self.reusable[index].authority)
    }

    /// Canonically ordered reusable arm inventory.
    #[must_use]
    pub fn reusable_gate_ids(&self) -> Vec<GateId> {
        self.reusable
            .iter()
            .map(|arm| arm.result.gate_id())
            .collect()
    }

    /// Source-invalidated or deliberately always-executed arms.
    #[must_use]
    pub fn invalidated_gate_ids(&self) -> &[GateId] {
        &self.invalidated
    }

    /// Added, removed, renamed, or byte-changed selected paths.
    #[must_use]
    pub fn changed_sources(&self) -> &[String] {
        &self.changed_sources
    }

    /// Parser, dynamic-edge, unknown-path, or selector-authority refusals.
    #[must_use]
    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }
}

#[derive(Serialize)]
struct ArmAuthority<'a> {
    version: u64,
    gate_id: GateId,
    worktree: Digest,
    source: &'a Digest,
    actor: &'a ::gate::ActorEvidence,
    scope: &'a ::gate::BatteryScope,
    configuration: &'a [::gate::ConfigurationEvidence],
    policy: &'a Digest,
    bounds: Vec<&'a BoundEvidence>,
    latches: Vec<&'a LatchEvidence>,
    tools: Vec<&'a ::gate::BatteryToolEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    compiler: Option<&'a Digest>,
}

#[derive(Serialize)]
struct MergeArmAuthority<'a> {
    version: u64,
    gate_id: GateId,
    repository: &'a Digest,
    source: &'a Digest,
    actor: &'a ::gate::ActorEvidence,
    configuration: &'a [::gate::ConfigurationEvidence],
    policy: &'a Digest,
    bounds: Vec<&'a BoundEvidence>,
    latches: Vec<&'a LatchEvidence>,
    tools: Vec<&'a ::gate::BatteryToolEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    compiler: Option<&'a Digest>,
}

fn arm_authority(
    repository: &Repository,
    gate_id: GateId,
    source: &Digest,
    context: &BatteryContext,
    compiler: Option<&Digest>,
) -> Result<Digest, String> {
    arm_authority_for(
        &worktree_authority(repository),
        gate_id,
        source,
        context,
        compiler,
    )
}

fn arm_authority_for(
    worktree: &Digest,
    gate_id: GateId,
    source: &Digest,
    context: &BatteryContext,
    compiler: Option<&Digest>,
) -> Result<Digest, String> {
    if compiler_gate(gate_id) != compiler.is_some() {
        return Err(format!(
            "semantic {} compiler authority is missing or foreign",
            gate_id.as_str(),
        ));
    }
    let bounds = context
        .bounds()
        .iter()
        .filter(|bound| metric_gate(bound.metric()) == Some(gate_id))
        .collect();
    let latches = context
        .latches()
        .iter()
        .filter(|latch| metric_gate(latch.kind()) == Some(gate_id))
        .collect();
    let authority = ArmAuthority {
        version: SEMANTIC_EVIDENCE_VERSION,
        gate_id,
        worktree: worktree.clone(),
        source,
        actor: context.actor(),
        scope: context.scope(),
        configuration: context.configuration(),
        policy: context.policy(),
        bounds,
        latches,
        tools: gate_tools(gate_id, context.tools()),
        compiler,
    };
    let bytes = wire::encode(&authority)
        .map_err(|error| format!("encode semantic arm authority: {error}"))?;
    Ok(Digest::compute(Domain::GateReceipt, &bytes))
}

fn merge_arm_authority(
    repository: &Digest,
    gate_id: GateId,
    source: &Digest,
    context: &BatteryContext,
    compiler: Option<&Digest>,
) -> Result<Digest, String> {
    if compiler_gate(gate_id) != compiler.is_some() {
        return Err(format!(
            "semantic {} compiler merge authority is missing or foreign",
            gate_id.as_str(),
        ));
    }
    let bounds = context
        .bounds()
        .iter()
        .filter(|bound| metric_gate(bound.metric()) == Some(gate_id))
        .collect();
    let latches = context
        .latches()
        .iter()
        .filter(|latch| metric_gate(latch.kind()) == Some(gate_id))
        .collect();
    let authority = MergeArmAuthority {
        version: MERGE_SEMANTIC_EVIDENCE_VERSION,
        gate_id,
        repository,
        source,
        actor: context.actor(),
        configuration: context.configuration(),
        policy: context.policy(),
        bounds,
        latches,
        tools: gate_tools(gate_id, context.tools()),
        compiler,
    };
    let bytes = wire::encode(&authority)
        .map_err(|error| format!("encode semantic merge arm authority: {error}"))?;
    Ok(Digest::compute(Domain::GateReceipt, &bytes))
}

pub(super) fn repository_authority(repository: &Repository) -> Digest {
    Digest::compute(
        Domain::GateRepositoryAuthority,
        repository.common_dir().as_os_str().as_encoded_bytes(),
    )
}

pub(super) fn worktree_authority(repository: &Repository) -> Digest {
    Digest::compute(
        Domain::GateReceipt,
        repository.git_dir().as_os_str().as_encoded_bytes(),
    )
}

fn result_identity(result: &BatteryArmResult) -> Result<Digest, String> {
    let bytes =
        wire::encode(result).map_err(|error| format!("encode semantic arm result: {error}"))?;
    Ok(Digest::compute(Domain::GateReceipt, &bytes))
}

fn compiler_gate(gate_id: GateId) -> bool {
    matches!(gate_id, GateId::RustTypecheck | GateId::CanvasWebTypecheck)
}

fn metric_gate(metric: &str) -> Option<GateId> {
    match metric {
        "file-loc" | "file-bytes" => Some(GateId::FileShape),
        "complexity-ccn" | "complexity-length" => Some(GateId::Complexity),
        "repo-doc-chars" => Some(GateId::RepoDocs),
        _ => None,
    }
}

fn gate_tools(
    gate_id: GateId,
    tools: &[::gate::BatteryToolEvidence],
) -> Vec<&::gate::BatteryToolEvidence> {
    let Some(declaration) = ::gate::TOOLCHAIN_ARMS
        .iter()
        .find(|declaration| declaration.id() == gate_id)
    else {
        return Vec::new();
    };
    tools
        .iter()
        .filter(|tool| {
            declaration
                .tools()
                .iter()
                .any(|requirement| requirement.name() == tool.name())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arm_tool_authority_contains_only_the_declared_tool_contract() {
        let tools = [
            ::gate::BatteryToolEvidence::identified(
                "cargo",
                "/fixture/cargo",
                "cargo fixture",
                Digest::compute(Domain::GateTool, b"cargo"),
            )
            .expect("cargo evidence"),
            ::gate::BatteryToolEvidence::identified(
                "rustfmt",
                "/fixture/rustfmt",
                "rustfmt fixture",
                Digest::compute(Domain::GateTool, b"rustfmt"),
            )
            .expect("rustfmt evidence"),
            ::gate::BatteryToolEvidence::identified(
                "trunk",
                "/fixture/trunk",
                "trunk fixture",
                Digest::compute(Domain::GateTool, b"trunk"),
            )
            .expect("trunk evidence"),
        ];

        assert_eq!(
            gate_tools(GateId::Formatters, &tools)
                .iter()
                .map(|tool| tool.name())
                .collect::<Vec<_>>(),
            ["rustfmt"],
        );
        assert_eq!(
            gate_tools(GateId::RustTypecheck, &tools)
                .iter()
                .map(|tool| tool.name())
                .collect::<Vec<_>>(),
            ["cargo"],
        );
        assert_eq!(
            gate_tools(GateId::CanvasWebTypecheck, &tools)
                .iter()
                .map(|tool| tool.name())
                .collect::<Vec<_>>(),
            ["cargo", "trunk"],
        );
        assert!(gate_tools(GateId::LocalPaths, &tools).is_empty());
    }
}
