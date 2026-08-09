//! Canonical external package index and closed package tree.

use std::fmt;

use serde::Serialize;
use serde::de::DeserializeOwned;
use wire::manifest::{
    ProductVersion, ProofStanding, RELEASE_MANIFEST_VERSION, RELEASE_PACKAGE_VERSION,
    ReleaseManifest, ReleasePackage, ReleasePackageTarget, ReleaseTarget,
};
use wire::{Digest, Domain};

const PACKAGE_NAME: &str = "spice";
const DATA_MODE: u32 = 0o644;
const EXECUTABLE_MODE: u32 = 0o755;
const MANIFEST_PATH: &str = "release/manifest.json";
const NOTES_PATH: &str = "release/notes.md";
const PACKAGE_PATH: &str = "release/package.json";
const TARGETS: [ReleaseTarget; 2] = [
    ReleaseTarget::Aarch64AppleDarwin,
    ReleaseTarget::X86_64UnknownLinuxMusl,
];

/// Exact bytes supplied for one supported target package entry.
#[derive(Clone, Copy, Debug)]
pub struct PackageTargetInput<'a> {
    target: ReleaseTarget,
    executable: &'a [u8],
    payload: &'a [u8],
    proof: &'a [u8],
}

impl<'a> PackageTargetInput<'a> {
    #[must_use]
    pub const fn new(
        target: ReleaseTarget,
        executable: &'a [u8],
        payload: &'a [u8],
        proof: &'a [u8],
    ) -> Self {
        Self {
            target,
            executable,
            payload,
            proof,
        }
    }
}

/// Complete already-sealed input required to form one external package tree.
#[derive(Clone, Debug)]
pub struct PackageInput<'a> {
    version: &'a ProductVersion,
    notes_identity: Digest,
    notes: &'a [u8],
    manifest: &'a [u8],
    targets: Vec<PackageTargetInput<'a>>,
}

impl<'a> PackageInput<'a> {
    #[must_use]
    pub fn new(
        version: &'a ProductVersion,
        notes_identity: Digest,
        notes: &'a [u8],
        manifest: &'a [u8],
        targets: Vec<PackageTargetInput<'a>>,
    ) -> Self {
        Self {
            version,
            notes_identity,
            notes,
            manifest,
            targets,
        }
    }
}

/// One exact regular file in the deterministic package tree.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageMember {
    path: String,
    mode: u32,
    bytes: Vec<u8>,
    digest: Digest,
}

impl PackageMember {
    /// Construct one observed package-tree member.
    ///
    /// The role is derived from the path so callers re-reading a staged tree
    /// cannot choose a digest domain independently from the package grammar.
    #[must_use]
    pub fn new(path: impl Into<String>, mode: u32, bytes: impl Into<Vec<u8>>) -> Self {
        let path = path.into();
        let bytes = bytes.into();
        let digest = Digest::compute(member_domain(&path), &bytes);
        Self {
            path,
            mode,
            bytes,
            digest,
        }
    }

    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    #[must_use]
    pub const fn mode(&self) -> u32 {
        self.mode
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub const fn digest(&self) -> &Digest {
        &self.digest
    }
}

/// One canonical package index and its exact domain identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdmittedPackage {
    document: ReleasePackage,
    bytes: Vec<u8>,
    identity: Digest,
}

impl AdmittedPackage {
    #[must_use]
    pub const fn document(&self) -> &ReleasePackage {
        &self.document
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub const fn identity(&self) -> &Digest {
        &self.identity
    }
}

/// Complete deterministic regular-file closure for one release package.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageTree {
    package: AdmittedPackage,
    members: Vec<PackageMember>,
}

impl PackageTree {
    #[must_use]
    pub const fn package(&self) -> &AdmittedPackage {
        &self.package
    }

    #[must_use]
    pub fn members(&self) -> &[PackageMember] {
        &self.members
    }
}

/// Build and re-admit one exact external package tree.
///
/// # Errors
///
/// Refuses noncanonical or stale evidence, target order or binding drift, a
/// non-clean proof, or executable bytes that differ from the sealed payload.
pub fn build_package(input: PackageInput<'_>) -> Result<PackageTree, PackageError> {
    let tree = assemble_package(input)?;
    admit_package_tree(tree.members())?;
    Ok(tree)
}

/// Re-admit a package tree from only its regular-file closure.
///
/// # Errors
///
/// Refuses extra, missing, duplicated, reordered, mis-moded, noncanonical, or
/// digest-mismatched members and any evidence binding drift.
pub fn admit_package_tree(members: &[PackageMember]) -> Result<AdmittedPackage, PackageError> {
    let package_member = exact_member(members, PACKAGE_PATH, DATA_MODE)?;
    let document: ReleasePackage = canonical(package_member.bytes(), "release package")?;
    admit_package_document(&document)?;
    let version = document.product_version.clone();
    let manifest = exact_member(members, MANIFEST_PATH, DATA_MODE)?;
    let notes = exact_member(members, NOTES_PATH, DATA_MODE)?;
    let mut targets = Vec::with_capacity(TARGETS.len());
    for target in TARGETS {
        targets.push(PackageTargetInput::new(
            target,
            exact_member(members, &executable_path(target), EXECUTABLE_MODE)?.bytes(),
            exact_member(members, &payload_path(target), DATA_MODE)?.bytes(),
            exact_member(members, &proof_path(target), DATA_MODE)?.bytes(),
        ));
    }
    let rebuilt = assemble_package(PackageInput::new(
        &version,
        document.notes.clone(),
        notes.bytes(),
        manifest.bytes(),
        targets,
    ))?;
    if rebuilt.members != members || rebuilt.package.document != document {
        return Err(PackageError::Closure);
    }
    Ok(rebuilt.package)
}

fn assemble_package(input: PackageInput<'_>) -> Result<PackageTree, PackageError> {
    let manifest: ReleaseManifest = canonical(input.manifest, "release manifest")?;
    let manifest_identity = Digest::compute(Domain::DistReleaseManifest, input.manifest);
    admit_manifest_shape(&manifest)?;
    if input.targets.len() != TARGETS.len() {
        return Err(PackageError::TargetCount(input.targets.len()));
    }
    let mut package_targets = Vec::with_capacity(TARGETS.len());
    for (index, ((target, expected), sealed)) in input
        .targets
        .iter()
        .zip(TARGETS)
        .zip(&manifest.targets)
        .enumerate()
    {
        if target.target != expected || sealed.target != expected {
            return Err(PackageError::TargetOrder {
                index,
                expected,
                input: target.target,
                manifest: sealed.target,
            });
        }
        let admitted = admit_target(target, sealed, &manifest.source, index)?;
        package_targets.push(ReleasePackageTarget {
            target: expected,
            executable_path: executable_path(expected),
            executable: admitted.executable,
            payload: admitted.payload,
            proof: admitted.proof,
        });
    }
    let document = ReleasePackage {
        schema: RELEASE_PACKAGE_VERSION,
        package: PACKAGE_NAME.to_owned(),
        product_version: input.version.clone(),
        source: manifest.source.clone(),
        manifest: manifest_identity,
        notes: input.notes_identity,
        notes_content: Digest::compute(Domain::DistReleaseNotesContent, input.notes),
        targets: package_targets,
    };
    let bytes = wire::encode(&document).map_err(|error| PackageError::Encode {
        label: "release package",
        detail: error.to_string(),
    })?;
    let package = AdmittedPackage {
        identity: Digest::compute(Domain::DistReleasePackage, &bytes),
        document,
        bytes,
    };
    let mut members = vec![
        member(
            MANIFEST_PATH,
            DATA_MODE,
            input.manifest,
            Domain::DistReleaseManifest,
        ),
        member(
            NOTES_PATH,
            DATA_MODE,
            input.notes,
            Domain::DistReleaseNotesContent,
        ),
        member(
            PACKAGE_PATH,
            DATA_MODE,
            package.bytes(),
            Domain::DistReleasePackage,
        ),
    ];
    for target in &input.targets {
        members.extend([
            member(
                &executable_path(target.target),
                EXECUTABLE_MODE,
                target.executable,
                Domain::DistArtifact,
            ),
            member(
                &payload_path(target.target),
                DATA_MODE,
                target.payload,
                Domain::DistTargetPayload,
            ),
            member(
                &proof_path(target.target),
                DATA_MODE,
                target.proof,
                Domain::ProofReceiptV3,
            ),
        ]);
    }
    members.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(PackageTree { package, members })
}

#[derive(Debug)]
struct TargetEvidence {
    executable: Digest,
    payload: Digest,
    proof: Digest,
}

fn admit_target(
    input: &PackageTargetInput<'_>,
    sealed: &wire::manifest::ReleaseManifestTarget,
    source: &Digest,
    index: usize,
) -> Result<TargetEvidence, PackageError> {
    let admitted_payload = crate::payload::admit(input.payload).map_err(PackageError::Payload)?;
    let payload = admitted_payload.value();
    let admitted_proof =
        crate::evidence::admit_receipt(input.proof).map_err(PackageError::Proof)?;
    let proof = admitted_proof.value();
    let payload_identity = admitted_payload.digest().clone();
    let proof_identity = admitted_proof.digest().clone();
    let executable = Digest::compute(Domain::DistArtifact, input.executable);
    let executable_inventory = payload
        .inventory
        .iter()
        .find(|entry| entry.path == "bin/spice")
        .ok_or(PackageError::Binding {
            index,
            field: "executable inventory",
        })?;
    let bindings = [
        (payload.target == input.target, "payload target"),
        (payload.source == *source, "payload source"),
        (payload.native == executable, "payload executable"),
        (
            executable_inventory.mode == EXECUTABLE_MODE,
            "executable mode",
        ),
        (
            executable_inventory.digest == executable,
            "inventory executable",
        ),
        (sealed.payload == payload_identity, "manifest payload"),
        (sealed.receipt == proof_identity, "manifest proof"),
        (proof.target == input.target, "proof target"),
        (proof.payload == payload_identity, "proof payload"),
        (proof.battery == sealed.battery, "proof battery"),
        (proof.replay == sealed.replay, "proof replay"),
        (proof.environment == sealed.environment, "proof environment"),
        (proof.standing == ProofStanding::Clean, "proof standing"),
        (sealed.standing == ProofStanding::Clean, "manifest standing"),
    ];
    if let Some((_, field)) = bindings.into_iter().find(|(matches, _)| !matches) {
        return Err(PackageError::Binding { index, field });
    }
    Ok(TargetEvidence {
        executable,
        payload: payload_identity,
        proof: proof_identity,
    })
}

fn admit_manifest_shape(manifest: &ReleaseManifest) -> Result<(), PackageError> {
    if manifest.version != RELEASE_MANIFEST_VERSION {
        return Err(PackageError::ManifestVersion(manifest.version));
    }
    if manifest.targets.len() != TARGETS.len() {
        return Err(PackageError::TargetCount(manifest.targets.len()));
    }
    Ok(())
}

fn admit_package_document(document: &ReleasePackage) -> Result<(), PackageError> {
    if document.schema != RELEASE_PACKAGE_VERSION {
        return Err(PackageError::PackageVersion(document.schema));
    }
    if document.package != PACKAGE_NAME {
        return Err(PackageError::PackageName(document.package.clone()));
    }
    if document.targets.len() != TARGETS.len() {
        return Err(PackageError::TargetCount(document.targets.len()));
    }
    for (index, (entry, expected)) in document.targets.iter().zip(TARGETS).enumerate() {
        if entry.target != expected || entry.executable_path != executable_path(expected) {
            return Err(PackageError::PackageTarget { index });
        }
    }
    Ok(())
}

fn canonical<T>(bytes: &[u8], label: &'static str) -> Result<T, PackageError>
where
    T: DeserializeOwned + Serialize,
{
    let value = wire::decode(bytes).map_err(|error| PackageError::Decode {
        label,
        detail: error.to_string(),
    })?;
    let canonical = wire::encode(&value).map_err(|error| PackageError::Encode {
        label,
        detail: error.to_string(),
    })?;
    if canonical == bytes {
        Ok(value)
    } else {
        Err(PackageError::NonCanonical(label))
    }
}

fn member(path: &str, mode: u32, bytes: &[u8], domain: Domain) -> PackageMember {
    let member = PackageMember::new(path, mode, bytes);
    debug_assert_eq!(member.digest, Digest::compute(domain, bytes));
    member
}

fn member_domain(path: &str) -> Domain {
    match path {
        MANIFEST_PATH => Domain::DistReleaseManifest,
        NOTES_PATH => Domain::DistReleaseNotesContent,
        PACKAGE_PATH => Domain::DistReleasePackage,
        path if path.ends_with("/payload.json") => Domain::DistTargetPayload,
        path if path.ends_with("/proof.json") => Domain::ProofReceiptV3,
        _ => Domain::DistArtifact,
    }
}

fn exact_member<'a>(
    members: &'a [PackageMember],
    path: &str,
    mode: u32,
) -> Result<&'a PackageMember, PackageError> {
    let mut found = members.iter().filter(|member| member.path == path);
    let member = found
        .next()
        .ok_or_else(|| PackageError::Missing(path.to_owned()))?;
    if found.next().is_some() {
        return Err(PackageError::Duplicate(path.to_owned()));
    }
    if member.mode != mode {
        return Err(PackageError::Mode {
            path: path.to_owned(),
            expected: mode,
            observed: member.mode,
        });
    }
    Ok(member)
}

fn executable_path(target: ReleaseTarget) -> String {
    format!("targets/{target}/bin/spice")
}

fn payload_path(target: ReleaseTarget) -> String {
    format!("targets/{target}/payload.json")
}

fn proof_path(target: ReleaseTarget) -> String {
    format!("targets/{target}/proof.json")
}

/// Why an external release package was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum PackageError {
    Decode {
        label: &'static str,
        detail: String,
    },
    Encode {
        label: &'static str,
        detail: String,
    },
    NonCanonical(&'static str),
    Payload(crate::PayloadError),
    Proof(crate::EvidenceError),
    ManifestVersion(u16),
    PackageVersion(u16),
    PackageName(String),
    TargetCount(usize),
    TargetOrder {
        index: usize,
        expected: ReleaseTarget,
        input: ReleaseTarget,
        manifest: ReleaseTarget,
    },
    Binding {
        index: usize,
        field: &'static str,
    },
    PackageTarget {
        index: usize,
    },
    Missing(String),
    Duplicate(String),
    Mode {
        path: String,
        expected: u32,
        observed: u32,
    },
    Closure,
}

impl fmt::Display for PackageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode { label, detail } => {
                write!(formatter, "{label} does not decode: {detail}")
            }
            Self::Encode { label, detail } => {
                write!(formatter, "{label} does not encode: {detail}")
            }
            Self::NonCanonical(label) => {
                write!(formatter, "{label} is not canonical current wire JSON")
            }
            Self::Payload(error) => write!(formatter, "target payload refuses: {error}"),
            Self::Proof(error) => write!(formatter, "proof receipt refuses: {error}"),
            Self::ManifestVersion(version) => {
                write!(
                    formatter,
                    "release manifest schema {version} is unsupported"
                )
            }
            Self::PackageVersion(version) => {
                write!(formatter, "release package schema {version} is unsupported")
            }
            Self::PackageName(name) => {
                write!(formatter, "release package name is unsupported: {name}")
            }
            Self::TargetCount(count) => {
                write!(
                    formatter,
                    "release package has {count} targets, expected two"
                )
            }
            Self::TargetOrder {
                index,
                expected,
                input,
                manifest,
            } => write!(
                formatter,
                "target {index} order moved: expected {expected}, input {input}, manifest {manifest}"
            ),
            Self::Binding { index, field } => {
                write!(formatter, "release target {index} binding moved at {field}")
            }
            Self::PackageTarget { index } => write!(
                formatter,
                "release package target {index} has a noncanonical target or executable path"
            ),
            Self::Missing(path) => write!(formatter, "release package member is missing: {path}"),
            Self::Duplicate(path) => {
                write!(formatter, "release package member is duplicated: {path}")
            }
            Self::Mode {
                path,
                expected,
                observed,
            } => write!(
                formatter,
                "release package member {path} mode is {observed:o}, expected {expected:o}"
            ),
            Self::Closure => {
                formatter.write_str("release package tree is not the exact canonical closure")
            }
        }
    }
}

impl std::error::Error for PackageError {}

#[cfg(test)]
mod tests {
    use wire::manifest::{
        BuildPins, InventoryEntry, PROOF_RECEIPT_VERSION, ProofEnvironment, ProofReceipt,
        ReleaseManifestTarget, TARGET_PAYLOAD_VERSION, TargetPayload,
    };

    use super::*;

    struct Fixture {
        version: ProductVersion,
        notes_identity: Digest,
        notes: Vec<u8>,
        manifest: Vec<u8>,
        executables: Vec<Vec<u8>>,
        payloads: Vec<Vec<u8>>,
        proofs: Vec<Vec<u8>>,
    }

    impl Fixture {
        fn input(&self) -> PackageInput<'_> {
            PackageInput::new(
                &self.version,
                self.notes_identity.clone(),
                &self.notes,
                &self.manifest,
                TARGETS
                    .into_iter()
                    .enumerate()
                    .map(|(index, target)| {
                        PackageTargetInput::new(
                            target,
                            &self.executables[index],
                            &self.payloads[index],
                            &self.proofs[index],
                        )
                    })
                    .collect(),
            )
        }
    }

    #[test]
    fn canonical_package_tree_round_trips_as_one_closed_identity() {
        let fixture = fixture();
        let tree = build_package(fixture.input()).expect("fixture package builds");
        assert_eq!(tree.members().len(), 9);
        assert!(
            tree.members()
                .windows(2)
                .all(|pair| pair[0].path() < pair[1].path())
        );
        let admitted = admit_package_tree(tree.members()).expect("package tree re-admits");
        assert_eq!(admitted, *tree.package());
        assert_eq!(admitted.document().product_version.as_str(), "1.2.3");
        assert_eq!(admitted.document().targets.len(), 2);
    }

    #[test]
    fn every_external_identity_input_moves_or_refuses_the_package() {
        let baseline_fixture = fixture();
        let baseline = build_package(baseline_fixture.input())
            .expect("baseline")
            .package
            .identity;

        let mut notes = fixture();
        notes.notes.push(b'!');
        assert_ne!(
            build_package(notes.input())
                .expect("notes move")
                .package
                .identity,
            baseline
        );

        let mut version = fixture();
        version.version = ProductVersion::parse("1.2.4").expect("version");
        assert_ne!(
            build_package(version.input())
                .expect("version moves")
                .package
                .identity,
            baseline
        );

        let mut executable = fixture();
        executable.executables[0].push(0);
        assert!(matches!(
            build_package(executable.input()),
            Err(PackageError::Binding {
                field: "payload executable",
                ..
            })
        ));
    }

    #[test]
    fn missing_extra_reordered_and_mismoded_members_refuse() {
        let tree = build_package(fixture().input()).expect("fixture package");

        let mut missing = tree.members.clone();
        missing.pop();
        assert!(admit_package_tree(&missing).is_err());

        let mut extra = tree.members.clone();
        extra.push(member("extra", DATA_MODE, b"extra", Domain::DistArtifact));
        assert_eq!(admit_package_tree(&extra), Err(PackageError::Closure));

        let mut reordered = tree.members.clone();
        reordered.swap(0, 1);
        assert_eq!(admit_package_tree(&reordered), Err(PackageError::Closure));

        let mut mismoded = tree.members.clone();
        let executable = mismoded
            .iter_mut()
            .find(|member| member.path.ends_with("/bin/spice"))
            .expect("executable member");
        executable.mode = DATA_MODE;
        assert!(matches!(
            admit_package_tree(&mismoded),
            Err(PackageError::Mode { .. })
        ));

        let mut digest_moved = tree.members.clone();
        digest_moved[0].digest = digest(Domain::DistArtifact, b"moved");
        assert_eq!(
            admit_package_tree(&digest_moved),
            Err(PackageError::Closure)
        );
    }

    #[test]
    fn noncanonical_or_moved_sealed_evidence_refuses_before_packaging() {
        let mut payload = fixture();
        payload.payloads[0].push(b'\n');
        assert_eq!(
            build_package(payload.input()),
            Err(PackageError::Payload(crate::PayloadError::NonCanonical))
        );

        let mut proof = fixture();
        let mut receipt: ProofReceipt = wire::decode(&proof.proofs[0]).expect("receipt");
        receipt.standing = ProofStanding::Refused;
        proof.proofs[0] = wire::encode(&receipt).expect("moved receipt");
        assert!(matches!(
            build_package(proof.input()),
            Err(PackageError::Binding {
                field: "manifest proof",
                ..
            })
        ));
    }

    fn fixture() -> Fixture {
        let source = digest(Domain::DistSourceCommit, b"source");
        let app = digest(Domain::DistArtifact, b"app");
        let embedded = digest(Domain::DistArtifact, b"embedded");
        let battery = digest(Domain::ProofBattery, b"battery");
        let environment = ProofEnvironment::native(digest(Domain::ProofNativeRunner, b"runner"));
        let mut executables = Vec::new();
        let mut payloads = Vec::new();
        let mut proofs = Vec::new();
        let mut manifest_targets = Vec::new();
        for (index, target) in TARGETS.into_iter().enumerate() {
            let executable = format!("executable-{index}").into_bytes();
            let native = Digest::compute(Domain::DistArtifact, &executable);
            let pins = BuildPins {
                rust: "rustc 1.97.1".to_owned(),
                wasm_target: "wasm32-unknown-unknown".to_owned(),
                trunk: "trunk 0.21.14".to_owned(),
                cargo_mutants: "cargo-mutants 27.1.0".to_owned(),
                proof_container: format!("registry.example/proof@sha256:{}", "1".repeat(64)),
            };
            let inventory = vec![
                InventoryEntry {
                    path: "app/app.wasm".to_owned(),
                    mode: DATA_MODE,
                    bytes: 3,
                    digest: app.clone(),
                },
                InventoryEntry {
                    path: "bin/spice".to_owned(),
                    mode: EXECUTABLE_MODE,
                    bytes: executable.len() as u64,
                    digest: native.clone(),
                },
                InventoryEntry {
                    path: "embed/app.bin".to_owned(),
                    mode: DATA_MODE,
                    bytes: 8,
                    digest: embedded.clone(),
                },
            ];
            let payload = TargetPayload {
                version: TARGET_PAYLOAD_VERSION,
                target,
                source: source.clone(),
                app: app.clone(),
                embedded_app: embedded.clone(),
                native: native.clone(),
                pins_digest: Digest::compute(Domain::DistPins, &wire::encode(&pins).expect("pins")),
                inventory_digest: Digest::compute(
                    Domain::DistInventory,
                    &wire::encode(&inventory).expect("inventory"),
                ),
                pins,
                inventory,
            };
            let payload_bytes = wire::encode(&payload).expect("payload");
            let payload_identity = Digest::compute(Domain::DistTargetPayload, &payload_bytes);
            let replay = digest(Domain::ProofReplayV3, format!("replay-{index}").as_bytes());
            let proof = ProofReceipt {
                version: PROOF_RECEIPT_VERSION,
                target,
                payload: payload_identity.clone(),
                battery: battery.clone(),
                replay: replay.clone(),
                environment: environment.clone(),
                standing: ProofStanding::Clean,
            };
            let proof_bytes = wire::encode(&proof).expect("proof");
            let proof_identity = Digest::compute(Domain::ProofReceiptV3, &proof_bytes);
            manifest_targets.push(ReleaseManifestTarget {
                target,
                payload: payload_identity,
                receipt: proof_identity,
                battery: battery.clone(),
                replay,
                environment: environment.clone(),
                standing: ProofStanding::Clean,
            });
            executables.push(executable);
            payloads.push(payload_bytes);
            proofs.push(proof_bytes);
        }
        let manifest = ReleaseManifest {
            version: RELEASE_MANIFEST_VERSION,
            source,
            targets: manifest_targets,
        };
        Fixture {
            version: ProductVersion::parse("1.2.3").expect("version"),
            notes_identity: digest(Domain::DistReleaseNotes, b"notes evidence"),
            notes: b"Curated release notes\n".to_vec(),
            manifest: wire::encode(&manifest).expect("manifest"),
            executables,
            payloads,
            proofs,
        }
    }

    fn digest(domain: Domain, bytes: &[u8]) -> Digest {
        Digest::compute(domain, bytes)
    }
}
