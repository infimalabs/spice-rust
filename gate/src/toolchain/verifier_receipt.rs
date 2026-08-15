//! Canonical cross-process receipts for successful verifier compiler evidence.

use std::ffi::OsString;
use std::os::unix::ffi::OsStrExt as _;
use std::path::Path;

use serde::{Deserialize, Serialize};
use wire::{Digest, Domain};

use super::{
    VerifierCompilerContract, VerifierCompilerEvidence, VerifierCompilerProcessEvidence,
    VerifierCompilerStage, compiler_environment, expected_stages, frame, stage_arguments,
};
use crate::manifest::GateId;
use crate::toolchain::ToolEvidence;

const RECEIPT_VERSION: u16 = 1;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct VerifierCompilerReceiptSet {
    pub(super) version: u16,
    pub(super) evidence: Vec<VerifierCompilerReceipt>,
    pub(super) authority_seal: Digest,
}

impl VerifierCompilerReceiptSet {
    fn mint(evidence: &[VerifierCompilerEvidence], authority: &[u8]) -> Result<Self, String> {
        if authority.is_empty() {
            return Err("verifier compiler receipt authority is empty".to_owned());
        }
        let mut receipt = Self {
            version: RECEIPT_VERSION,
            evidence: evidence
                .iter()
                .map(VerifierCompilerReceipt::from_evidence)
                .collect(),
            authority_seal: Digest::compute(
                Domain::GateReceipt,
                b"unsealed verifier compiler receipt authority",
            ),
        };
        receipt.authority_seal = receipt.compute_authority_seal(authority);
        Ok(receipt)
    }

    pub(super) fn compute_authority_seal(&self, authority: &[u8]) -> Digest {
        let mut bytes = Vec::new();
        frame(&mut bytes, authority);
        frame(&mut bytes, &self.version.to_be_bytes());
        for evidence in &self.evidence {
            frame(&mut bytes, evidence.seal.as_str().as_bytes());
        }
        Digest::compute(Domain::GateReceipt, &bytes)
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct VerifierCompilerReceipt {
    gate_id: GateId,
    source: Digest,
    manifest: Digest,
    repository: Digest,
    tools: Digest,
    environment: Digest,
    pub(super) processes: Vec<VerifierCompilerProcessReceipt>,
    pub(super) seal: Digest,
}

impl VerifierCompilerReceipt {
    fn from_evidence(evidence: &VerifierCompilerEvidence) -> Self {
        let mut receipt = Self {
            gate_id: evidence.gate_id,
            source: evidence.source.clone(),
            manifest: evidence.manifest.clone(),
            repository: path_identity(&evidence.repository),
            tools: tools_identity(&evidence.tools),
            environment: environment_identity(&evidence.environment),
            processes: evidence
                .processes
                .iter()
                .map(VerifierCompilerProcessReceipt::from_evidence)
                .collect(),
            seal: Digest::compute(Domain::GateReceipt, b"unsealed verifier compiler receipt"),
        };
        receipt.seal = receipt.compute_seal();
        receipt
    }

    fn admit(
        &self,
        contract: &VerifierCompilerContract,
    ) -> Result<VerifierCompilerEvidence, String> {
        if self.gate_id != contract.gate_id
            || self.source != contract.source
            || self.manifest != contract.manifest
            || self.repository != path_identity(&contract.repository)
            || self.tools != tools_identity(&contract.tools)
            || self.environment != environment_identity(&contract.environment)
            || self.seal != self.compute_seal()
        {
            return Err(format!(
                "{} compiler receipt does not match the current source-bound contract",
                contract.gate_id.as_str()
            ));
        }
        let expected = expected_stages(contract.gate_id);
        if self.processes.len() != expected.len() {
            return Err(format!(
                "{} compiler receipt has {} of {} required stages",
                contract.gate_id.as_str(),
                self.processes.len(),
                expected.len()
            ));
        }
        let mut processes = Vec::with_capacity(expected.len());
        for stage in expected {
            let matching = self
                .processes
                .iter()
                .filter(|process| process.stage == *stage)
                .collect::<Vec<_>>();
            if matching.len() != 1 {
                return Err(format!(
                    "{} compiler receipt has no one exact {} stage",
                    contract.gate_id.as_str(),
                    stage.tag()
                ));
            }
            processes.push(matching[0].admit(contract)?);
        }
        contract.complete(&processes)
    }

    pub(super) fn compute_seal(&self) -> Digest {
        let mut bytes = Vec::new();
        frame(&mut bytes, self.gate_id.as_str().as_bytes());
        frame(&mut bytes, self.source.as_str().as_bytes());
        frame(&mut bytes, self.manifest.as_str().as_bytes());
        frame(&mut bytes, self.repository.as_str().as_bytes());
        frame(&mut bytes, self.tools.as_str().as_bytes());
        frame(&mut bytes, self.environment.as_str().as_bytes());
        for process in &self.processes {
            frame(&mut bytes, process.seal.as_str().as_bytes());
        }
        Digest::compute(Domain::GateReceipt, &bytes)
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct VerifierCompilerProcessReceipt {
    stage: VerifierCompilerStage,
    pub(super) targets: Digest,
    arguments: Digest,
    exit_code: i32,
    elapsed_ms: u64,
    pub(super) seal: Digest,
}

impl VerifierCompilerProcessReceipt {
    fn from_evidence(evidence: &VerifierCompilerProcessEvidence) -> Self {
        let mut receipt = Self {
            stage: evidence.stage,
            targets: strings_identity(&evidence.targets),
            arguments: os_strings_identity(&evidence.arguments),
            exit_code: evidence.exit_code,
            elapsed_ms: evidence.elapsed_ms,
            seal: Digest::compute(
                Domain::GateReceipt,
                b"unsealed verifier compiler process receipt",
            ),
        };
        receipt.seal = receipt.compute_seal();
        receipt
    }

    fn admit(
        &self,
        contract: &VerifierCompilerContract,
    ) -> Result<VerifierCompilerProcessEvidence, String> {
        if self.stage.gate_id() != contract.gate_id
            || self.targets != strings_identity(self.stage.targets())
            || self.arguments
                != os_strings_identity(&stage_arguments(self.stage, &contract.repository))
            || self.exit_code != 0
            || self.seal != self.compute_seal()
        {
            return Err(format!(
                "{} compiler receipt has invalid {} process evidence",
                contract.gate_id.as_str(),
                self.stage.tag()
            ));
        }
        Ok(VerifierCompilerProcessEvidence::mint(
            contract,
            self.stage,
            compiler_environment(&contract.environment, &contract.repository),
            stage_arguments(self.stage, &contract.repository),
            self.elapsed_ms,
            Some(self.exit_code),
        ))
    }

    pub(super) fn compute_seal(&self) -> Digest {
        let mut bytes = Vec::new();
        frame(&mut bytes, self.stage.tag().as_bytes());
        frame(&mut bytes, self.targets.as_str().as_bytes());
        frame(&mut bytes, self.arguments.as_str().as_bytes());
        frame(&mut bytes, &self.exit_code.to_be_bytes());
        frame(&mut bytes, &self.elapsed_ms.to_be_bytes());
        Digest::compute(Domain::GateReceipt, &bytes)
    }
}

/// Encode successful compiler authority into one canonical cross-process receipt.
///
/// The document carries only source-bound digests and stage metadata. Ambient
/// environment values and the one-time provenance authority never cross into
/// the artifact.
pub fn encode_verifier_compiler_receipt(
    evidence: &[VerifierCompilerEvidence],
    authority: &[u8],
) -> Result<Vec<u8>, String> {
    wire::encode(&VerifierCompilerReceiptSet::mint(evidence, authority)?)
        .map_err(|error| format!("verifier compiler receipt could not encode: {error}"))
}

/// Admit one canonical compiler receipt against fresh expected contracts.
///
/// Missing, extra, duplicate, stale, noncanonical, mismatched, or authority-
/// mismatched evidence is a refusal. This path never degrades into running a
/// compiler inside the caller.
pub fn admit_verifier_compiler_receipt(
    document: &[u8],
    contracts: &[VerifierCompilerContract],
    authority: &[u8],
) -> Result<Vec<VerifierCompilerEvidence>, String> {
    if authority.is_empty() {
        return Err("verifier compiler receipt authority is empty".to_owned());
    }
    let receipt: VerifierCompilerReceiptSet = wire::decode(document)
        .map_err(|error| format!("verifier compiler receipt could not decode: {error}"))?;
    let canonical = wire::encode(&receipt)
        .map_err(|error| format!("verifier compiler receipt could not encode: {error}"))?;
    if canonical != document {
        return Err("verifier compiler receipt is not canonical".to_owned());
    }
    if receipt.authority_seal != receipt.compute_authority_seal(authority) {
        return Err("verifier compiler receipt has untrusted provenance".to_owned());
    }
    if receipt.version != RECEIPT_VERSION {
        return Err(format!(
            "verifier compiler receipt version {} is unsupported",
            receipt.version
        ));
    }
    if receipt.evidence.len() != contracts.len() {
        return Err(format!(
            "verifier compiler receipt has {} of {} required arm records",
            receipt.evidence.len(),
            contracts.len()
        ));
    }
    contracts
        .iter()
        .map(|contract| {
            let matching = receipt
                .evidence
                .iter()
                .filter(|evidence| evidence.gate_id == contract.gate_id)
                .collect::<Vec<_>>();
            if matching.len() != 1 {
                return Err(format!(
                    "verifier compiler receipt has no one exact {} arm record",
                    contract.gate_id.as_str()
                ));
            }
            matching[0].admit(contract)
        })
        .collect()
}

/// Inspect a canonical source-bound compiler receipt for semantic arm authorities.
///
/// This validates the original worktree provenance, source, manifest, stage
/// inventory, process seals, and clean exits without pretending its old source
/// root is the current one. The returned authority can therefore be compared
/// with a fresh source-independent contract before one arm is reused.
pub fn semantic_verifier_compiler_authorities(
    document: &[u8],
    source: &Digest,
    authority: &[u8],
) -> Result<Vec<(GateId, Digest)>, String> {
    if authority.is_empty() {
        return Err("verifier compiler receipt authority is empty".to_owned());
    }
    let receipt: VerifierCompilerReceiptSet = wire::decode(document)
        .map_err(|error| format!("verifier compiler receipt could not decode: {error}"))?;
    let canonical = wire::encode(&receipt)
        .map_err(|error| format!("verifier compiler receipt could not encode: {error}"))?;
    if canonical != document {
        return Err("verifier compiler receipt is not canonical".to_owned());
    }
    if receipt.version != RECEIPT_VERSION
        || receipt.authority_seal != receipt.compute_authority_seal(authority)
    {
        return Err("verifier compiler receipt has unsupported or untrusted provenance".to_owned());
    }
    let mut authorities = Vec::with_capacity(receipt.evidence.len());
    for evidence in &receipt.evidence {
        if evidence.source != *source
            || evidence.manifest != super::manifest_identity()
            || evidence.seal != evidence.compute_seal()
        {
            return Err(format!(
                "{} compiler receipt has stale or invalid arm evidence",
                evidence.gate_id.as_str(),
            ));
        }
        let expected = expected_stages(evidence.gate_id);
        if evidence.processes.len() != expected.len() {
            return Err(format!(
                "{} compiler receipt has an incomplete stage inventory",
                evidence.gate_id.as_str(),
            ));
        }
        for stage in expected {
            let matching = evidence
                .processes
                .iter()
                .filter(|process| process.stage == *stage)
                .collect::<Vec<_>>();
            if matching.len() != 1
                || matching[0].targets != strings_identity(stage.targets())
                || matching[0].exit_code != 0
                || matching[0].seal != matching[0].compute_seal()
            {
                return Err(format!(
                    "{} compiler receipt has invalid {} process evidence",
                    evidence.gate_id.as_str(),
                    stage.tag(),
                ));
            }
        }
        let mut bytes = Vec::new();
        frame(&mut bytes, b"gate/verifier-compiler-semantic-authority/v1");
        frame(&mut bytes, evidence.gate_id.as_str().as_bytes());
        frame(&mut bytes, evidence.manifest.as_str().as_bytes());
        frame(&mut bytes, evidence.repository.as_str().as_bytes());
        frame(&mut bytes, evidence.tools.as_str().as_bytes());
        frame(&mut bytes, evidence.environment.as_str().as_bytes());
        for stage in expected {
            let process = evidence
                .processes
                .iter()
                .find(|process| process.stage == *stage)
                .expect("complete stage inventory was checked");
            frame(&mut bytes, stage.tag().as_bytes());
            frame(&mut bytes, process.targets.as_str().as_bytes());
            frame(&mut bytes, process.arguments.as_str().as_bytes());
        }
        authorities.push((
            evidence.gate_id,
            Digest::compute(Domain::GateReceipt, &bytes),
        ));
    }
    authorities.sort_by_key(|(gate_id, _)| *gate_id);
    if authorities.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err("verifier compiler receipt repeats a semantic arm".to_owned());
    }
    Ok(authorities)
}

fn tools_identity(tools: &[ToolEvidence]) -> Digest {
    let mut bytes = Vec::new();
    for tool in tools {
        frame(
            &mut bytes,
            &wire::encode(tool).expect("tool evidence is encodable"),
        );
    }
    Digest::compute(Domain::GateReceipt, &bytes)
}

fn environment_identity(environment: &[(OsString, OsString)]) -> Digest {
    let mut bytes = Vec::new();
    for (name, value) in environment {
        frame(&mut bytes, name.as_bytes());
        frame(&mut bytes, value.as_bytes());
    }
    Digest::compute(Domain::GateReceipt, &bytes)
}

fn path_identity(path: &Path) -> Digest {
    Digest::compute(Domain::GateReceipt, path.as_os_str().as_bytes())
}

fn strings_identity<S: AsRef<str>>(values: &[S]) -> Digest {
    let mut bytes = Vec::new();
    for value in values {
        frame(&mut bytes, value.as_ref().as_bytes());
    }
    Digest::compute(Domain::GateReceipt, &bytes)
}

fn os_strings_identity(values: &[OsString]) -> Digest {
    let mut bytes = Vec::new();
    for value in values {
        frame(&mut bytes, value.as_bytes());
    }
    Digest::compute(Domain::GateReceipt, &bytes)
}
