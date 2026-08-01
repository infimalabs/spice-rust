//! Sealed release admission and deterministic operator-facing notes.

use serde::{Deserialize, Serialize};
use wire::manifest::{
    ProofEnvironment, ProofStanding, RELEASE_MANIFEST_VERSION, ReleaseManifest, ReleaseTarget,
    SealedTarget, TARGET_PAYLOAD_VERSION,
};
use wire::{Digest, Domain};

const GIT_OID_LEN: usize = 40;

/// A canonical sealed manifest admitted with its durable identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdmittedRelease {
    manifest: ReleaseManifest,
    digest: Digest,
}

impl AdmittedRelease {
    /// Decoded manifest after every internal identity binding was checked.
    #[must_use]
    pub const fn manifest(&self) -> &ReleaseManifest {
        &self.manifest
    }

    /// Canonical sealed manifest identity.
    #[must_use]
    pub const fn digest(&self) -> &Digest {
        &self.digest
    }
}

/// Authored explanation and operator guidance kept outside generated identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseNarrative {
    pub manifest: String,
    pub summary: String,
    pub operator_actions: Vec<String>,
    pub fixed_refusals: Vec<String>,
}

/// One commit actually found in the sealed Git range.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CommitSummary {
    pub id: String,
    pub title: String,
}

/// One supported artifact and its proof identities.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TargetNote {
    pub target: ReleaseTarget,
    pub native: Digest,
    pub app: Digest,
    pub receipt: Digest,
}

/// Generated factual notes plus separately authored explanation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReleaseNotes {
    pub release: String,
    pub build: Digest,
    pub source: Digest,
    pub git_from: String,
    pub git_to: String,
    pub previous_journal_schema: u32,
    pub journal_schema: u32,
    pub summary: String,
    pub operator_actions: Vec<String>,
    pub fixed_refusals: Vec<String>,
    pub targets: Vec<TargetNote>,
    pub commits: Vec<CommitSummary>,
}

impl ReleaseNotes {
    /// Stable Markdown rendering for the human CLI form.
    #[must_use]
    pub fn markdown(&self) -> String {
        let mut document = format!(
            "# Spice {}\n\n{}\n\n## Build Identity\n\n- Manifest: `{}`\n- Source: `{}`\n- Git range: `{}`..`{}`\n- Journal schema: {} -> {}\n\n## Supported Targets\n\n",
            self.release,
            self.summary,
            self.build,
            self.source,
            self.git_from,
            self.git_to,
            self.previous_journal_schema,
            self.journal_schema,
        );
        for target in &self.targets {
            document.push_str(&format!(
                "- `{}`: native `{}`, app `{}`, receipt `{}`\n",
                target.target, target.native, target.app, target.receipt
            ));
        }
        section(&mut document, "Operator Actions", &self.operator_actions);
        section(&mut document, "Fixed Refusals", &self.fixed_refusals);
        document.push_str("\n## Git Changes\n\n");
        for commit in &self.commits {
            document.push_str(&format!("- `{}` {}\n", commit.id, commit.title));
        }
        document
    }
}

fn section(document: &mut String, title: &str, values: &[String]) {
    document.push_str(&format!("\n## {title}\n\n"));
    for value in values {
        document.push_str(&format!("- {value}\n"));
    }
}

/// Admit canonical manifest bytes and rederive every embedded identity.
///
/// # Errors
///
/// Refuses encoding drift, unsupported schemas, malformed Git endpoints,
/// missing targets, payload or receipt digest disagreement, non-clean proof,
/// and cross-target source or app disagreement.
pub fn admit_release_manifest(bytes: &[u8]) -> Result<AdmittedRelease, ReleaseError> {
    let manifest: ReleaseManifest =
        wire::decode(bytes).map_err(|error| ReleaseError::Decode(error.to_string()))?;
    let canonical =
        wire::encode(&manifest).map_err(|error| ReleaseError::Encode(error.to_string()))?;
    if canonical != bytes {
        return Err(ReleaseError::Noncanonical);
    }
    if manifest.version != RELEASE_MANIFEST_VERSION {
        return Err(ReleaseError::Version(manifest.version));
    }
    validate_text("release", &manifest.release)?;
    validate_git_oid("git_from", &manifest.git_from)?;
    validate_git_oid("git_to", &manifest.git_to)?;
    if manifest.git_from == manifest.git_to {
        return Err(ReleaseError::EmptyRange);
    }
    if manifest.journal_schema == 0 || manifest.previous_journal_schema > manifest.journal_schema {
        return Err(ReleaseError::SchemaMovement {
            previous: manifest.previous_journal_schema,
            current: manifest.journal_schema,
        });
    }
    verify_targets(&manifest.targets)?;
    Ok(AdmittedRelease {
        digest: Digest::compute(Domain::DistReleaseManifest, &canonical),
        manifest,
    })
}

fn verify_targets(targets: &[SealedTarget]) -> Result<(), ReleaseError> {
    let found = targets.iter().map(|entry| entry.target).collect::<Vec<_>>();
    if found != ReleaseTarget::ALL {
        return Err(ReleaseError::Targets(found));
    }
    for entry in targets {
        verify_target(entry)?;
    }
    let first = &targets[0].payload;
    let second = &targets[1].payload;
    if first.source != second.source
        || first.app != second.app
        || first.embedded_app != second.embedded_app
        || first.pins != second.pins
        || first.pins_digest != second.pins_digest
        || targets[0].receipt.battery != targets[1].receipt.battery
    {
        return Err(ReleaseError::CrossTarget);
    }
    Ok(())
}

fn verify_target(entry: &SealedTarget) -> Result<(), ReleaseError> {
    if entry.target != entry.payload.target || entry.target != entry.receipt.target {
        return Err(ReleaseError::TargetBinding(entry.target));
    }
    if entry.payload.version != TARGET_PAYLOAD_VERSION {
        return Err(ReleaseError::PayloadVersion {
            target: entry.target,
            version: entry.payload.version,
        });
    }
    let payload =
        wire::encode(&entry.payload).map_err(|error| ReleaseError::Encode(error.to_string()))?;
    let payload_digest = Digest::compute(Domain::DistTargetPayload, &payload);
    if payload_digest != entry.payload_digest || entry.receipt.payload != payload_digest {
        return Err(ReleaseError::PayloadBinding(entry.target));
    }
    if entry.receipt.version != wire::manifest::PROOF_RECEIPT_VERSION
        || entry.receipt.standing != ProofStanding::Clean
    {
        return Err(ReleaseError::ReceiptStanding(entry.target));
    }
    let environment_matches = matches!(
        (entry.target, &entry.receipt.environment),
        (
            ReleaseTarget::Aarch64AppleDarwin,
            ProofEnvironment::Native { .. }
        ) | (
            ReleaseTarget::X86_64UnknownLinuxMusl,
            ProofEnvironment::Oci { .. }
        )
    );
    if !environment_matches {
        return Err(ReleaseError::ReceiptEnvironment(entry.target));
    }
    let receipt =
        wire::encode(&entry.receipt).map_err(|error| ReleaseError::Encode(error.to_string()))?;
    if Digest::compute(Domain::ProofReceiptV2, &receipt) != entry.receipt_digest {
        return Err(ReleaseError::ReceiptBinding(entry.target));
    }
    Ok(())
}

/// Compose notes after the rim has materialized the sealed Git range.
///
/// # Errors
///
/// Refuses authored placeholders, empty guidance sections, an empty Git range,
/// or commit rows with malformed identities or titles.
pub fn release_notes(
    release: &AdmittedRelease,
    narrative: &ReleaseNarrative,
    commits: Vec<CommitSummary>,
) -> Result<ReleaseNotes, ReleaseError> {
    validate_narrative(narrative)?;
    if commits.is_empty() {
        return Err(ReleaseError::NoCommits);
    }
    for commit in &commits {
        validate_git_oid("commit", &commit.id)?;
        validate_text("commit title", &commit.title)?;
    }
    let manifest = release.manifest();
    let targets = manifest
        .targets
        .iter()
        .map(|entry| TargetNote {
            target: entry.target,
            native: entry.payload.native.clone(),
            app: entry.payload.app.clone(),
            receipt: entry.receipt_digest.clone(),
        })
        .collect();
    Ok(ReleaseNotes {
        release: manifest.release.clone(),
        build: release.digest().clone(),
        source: manifest.targets[0].payload.source.clone(),
        git_from: manifest.git_from.clone(),
        git_to: manifest.git_to.clone(),
        previous_journal_schema: manifest.previous_journal_schema,
        journal_schema: manifest.journal_schema,
        summary: narrative.summary.clone(),
        operator_actions: narrative.operator_actions.clone(),
        fixed_refusals: narrative.fixed_refusals.clone(),
        targets,
        commits,
    })
}

fn validate_narrative(narrative: &ReleaseNarrative) -> Result<(), ReleaseError> {
    validate_text("manifest path", &narrative.manifest)?;
    validate_text("summary", &narrative.summary)?;
    if narrative.operator_actions.is_empty() || narrative.fixed_refusals.is_empty() {
        return Err(ReleaseError::EmptyNarrativeSection);
    }
    for action in &narrative.operator_actions {
        validate_text("operator action", action)?;
    }
    for refusal in &narrative.fixed_refusals {
        validate_text("fixed refusal", refusal)?;
    }
    Ok(())
}

fn validate_git_oid(field: &'static str, text: &str) -> Result<(), ReleaseError> {
    if text.len() == GIT_OID_LEN
        && text.bytes().all(|byte| {
            byte.is_ascii_digit() || (byte.is_ascii_lowercase() && byte.is_ascii_hexdigit())
        })
    {
        Ok(())
    } else {
        Err(ReleaseError::GitOid {
            field,
            value: text.to_owned(),
        })
    }
}

fn validate_text(field: &'static str, text: &str) -> Result<(), ReleaseError> {
    let upper = text.to_ascii_uppercase();
    let placeholder = text.trim().is_empty()
        || ["TODO", "TBD", "FIXME", "???", "{{", "}}", "<", ">"]
            .iter()
            .any(|marker| upper.contains(marker));
    if placeholder {
        Err(ReleaseError::Placeholder {
            field,
            value: text.to_owned(),
        })
    } else {
        Ok(())
    }
}

/// Why a sealed release or its generated notes were refused.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ReleaseError {
    #[error("sealed release manifest does not decode: {0}")]
    Decode(String),
    #[error("sealed release manifest cannot be encoded: {0}")]
    Encode(String),
    #[error("sealed release manifest bytes are not canonical")]
    Noncanonical,
    #[error("sealed release manifest version {0} is not current")]
    Version(u16),
    #[error("{field} is not one canonical SHA-1 object id: {value:?}")]
    GitOid { field: &'static str, value: String },
    #[error("sealed release Git range is empty")]
    EmptyRange,
    #[error("sealed release journal schema movement {previous} -> {current} is invalid")]
    SchemaMovement { previous: u32, current: u32 },
    #[error("sealed release targets are not the exact supported set in order: {0:?}")]
    Targets(Vec<ReleaseTarget>),
    #[error("sealed target {0} disagrees with its payload or receipt target")]
    TargetBinding(ReleaseTarget),
    #[error("sealed target {target} payload version {version} is not current")]
    PayloadVersion { target: ReleaseTarget, version: u16 },
    #[error("sealed target {0} payload digest or receipt payload binding disagrees")]
    PayloadBinding(ReleaseTarget),
    #[error("sealed target {0} receipt is not current and clean")]
    ReceiptStanding(ReleaseTarget),
    #[error("sealed target {0} receipt uses the wrong proof environment class")]
    ReceiptEnvironment(ReleaseTarget),
    #[error("sealed target {0} receipt digest disagrees")]
    ReceiptBinding(ReleaseTarget),
    #[error("sealed targets disagree about source, app, embedded app, or build pins")]
    CrossTarget,
    #[error("release narrative {field} contains an unresolved placeholder: {value:?}")]
    Placeholder { field: &'static str, value: String },
    #[error("release narrative must name at least one operator action and fixed refusal")]
    EmptyNarrativeSection,
    #[error("sealed Git range contains no commits")]
    NoCommits,
}
