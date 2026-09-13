use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use wire::manifest::{
    BuildPins, CANVAS_APP_FILES, CANVAS_APP_HTML, InventoryEntry, PROOF_RECEIPT_VERSION,
    ProductVersion, ProofEnvironment, ProofReceipt, ProofStanding, RELEASE_MANIFEST_VERSION,
    ReleaseManifest, ReleaseManifestTarget, ReleaseTarget, TARGET_PAYLOAD_VERSION, TargetPayload,
};
use wire::{Digest, Domain};

const TARGETS: [ReleaseTarget; 2] = [
    ReleaseTarget::Aarch64AppleDarwin,
    ReleaseTarget::X86_64UnknownLinuxMusl,
];
const MAX_FIXTURE_MEMBER_BYTES: u64 = 512 * 1024 * 1024;

pub(crate) struct PackageFixture {
    root: PathBuf,
    app: Vec<(String, Vec<u8>)>,
}

impl PackageFixture {
    pub(crate) fn create(root: PathBuf, executable: &Path) -> Self {
        fs::create_dir(&root).expect("create package fixture root");
        let executable_path = executable;
        let executable = fs::read(executable_path).expect("read package fixture executable");
        let source = Digest::compute(Domain::DistSourceCommit, b"package fixture source");
        let app_identity = Digest::compute(Domain::DistArtifact, b"package fixture Canvas");
        let mut app = CANVAS_APP_FILES
            .iter()
            .map(|file| {
                let bytes = if *file == CANVAS_APP_HTML {
                    concat!(
                        "<!doctype html><html><body>",
                        "<script data-canvas-theme>theme();</script>",
                        "<script type=\"module\">boot();</script>",
                        "</body></html>\n"
                    )
                    .as_bytes()
                    .to_vec()
                } else {
                    format!("package Canvas asset {file}\n").into_bytes()
                };
                ((*file).to_owned(), bytes)
            })
            .collect::<Vec<_>>();
        app.sort_by(|left, right| left.0.cmp(&right.0));
        let battery = Digest::compute(Domain::ProofBattery, b"package fixture battery");
        let environment =
            ProofEnvironment::native(Digest::compute(Domain::ProofNativeRunner, b"runner"));
        let mut payloads = Vec::new();
        let mut proofs = Vec::new();
        let mut manifest_targets = Vec::new();
        for target in TARGETS {
            let (payload, proof, sealed) = target_evidence(
                target,
                &executable,
                &source,
                &app_identity,
                &app,
                &battery,
                &environment,
            );
            payloads.push(payload);
            proofs.push(proof);
            manifest_targets.push(sealed);
        }
        let manifest = wire::encode(&ReleaseManifest {
            version: RELEASE_MANIFEST_VERSION,
            source,
            targets: manifest_targets,
        })
        .expect("encode package fixture manifest");
        let current =
            dist::current_product_version().expect("current product version is canonical");
        let major = current
            .as_str()
            .split('.')
            .next()
            .expect("current version has a major component");
        let version = ProductVersion::parse(&format!("1{major}.0.0"))
            .expect("package fixture version advances current major");
        let notes = b"Package fixture release notes\n";
        let tree = proof::build_package(proof::PackageInput::new(
            &version,
            Digest::compute(Domain::DistReleaseNotes, b"package fixture notes"),
            inline("release/notes.md", 0o644, notes),
            inline("release/manifest.json", 0o644, &manifest),
            app.iter()
                .map(|(file, bytes)| {
                    proof::PackageAppInput::new(file, inline(&format!("app/{file}"), 0o644, bytes))
                })
                .collect(),
            TARGETS
                .into_iter()
                .enumerate()
                .map(|(index, target)| {
                    proof::PackageTargetInput::new(
                        target,
                        file_member(
                            executable_path,
                            &format!("targets/{target}/bin/spice"),
                            0o755,
                        ),
                        inline(
                            &format!("targets/{target}/payload.json"),
                            0o644,
                            &payloads[index],
                        ),
                        inline(
                            &format!("targets/{target}/proof.json"),
                            0o644,
                            &proofs[index],
                        ),
                    )
                })
                .collect(),
        ))
        .expect("build canonical package fixture");
        for member in tree.members() {
            let path = root.join(member.path());
            fs::create_dir_all(path.parent().expect("package member has a parent"))
                .expect("create package fixture parent");
            let mut file = fs::File::create(&path).expect("create package fixture member");
            member
                .copy_to(&mut file)
                .expect("write package fixture member");
            fs::set_permissions(&path, fs::Permissions::from_mode(member.mode()))
                .expect("mode package fixture member");
        }
        Self { root, app }
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn app_bytes(&self, file: &str) -> &[u8] {
        self.app
            .iter()
            .find_map(|(candidate, bytes)| (candidate == file).then_some(bytes.as_slice()))
            .expect("package fixture carries every Canvas file")
    }
}

fn inline(path: &str, mode: u32, bytes: impl AsRef<[u8]>) -> proof::PackageMember {
    proof::PackageMember::admit_inline(path, mode, bytes.as_ref(), proof::PACKAGE_DOCUMENT_LIMIT)
        .expect("fixture member admits")
}

fn file_member(source: &Path, path: &str, mode: u32) -> proof::PackageMember {
    proof::PackageMember::admit_file(
        path,
        mode,
        fs::File::open(source).expect("fixture member opens"),
        MAX_FIXTURE_MEMBER_BYTES,
    )
    .expect("fixture member admits")
}

#[allow(clippy::too_many_arguments)]
fn target_evidence(
    target: ReleaseTarget,
    executable: &[u8],
    source: &Digest,
    app_identity: &Digest,
    app: &[(String, Vec<u8>)],
    battery: &Digest,
    environment: &ProofEnvironment,
) -> (Vec<u8>, Vec<u8>, ReleaseManifestTarget) {
    let native = Digest::compute(Domain::DistArtifact, executable);
    let pins = BuildPins {
        rust: "rustc 1.97.1".to_owned(),
        wasm_target: "wasm32-unknown-unknown".to_owned(),
        trunk: "trunk 0.21.0".to_owned(),
        cargo_mutants: "cargo-mutants 27.0.0".to_owned(),
        proof_container: format!("spice/proof@sha256:{}", "1".repeat(64)),
    };
    let mut inventory = app
        .iter()
        .map(|(file, bytes)| InventoryEntry {
            path: format!("app/{file}"),
            mode: 0o644,
            bytes: bytes.len() as u64,
            digest: Digest::compute(Domain::DistArtifact, bytes),
        })
        .collect::<Vec<_>>();
    inventory.push(InventoryEntry {
        path: "bin/spice".to_owned(),
        mode: 0o755,
        bytes: executable.len() as u64,
        digest: native.clone(),
    });
    inventory.sort_by(|left, right| left.path.cmp(&right.path));
    let payload = TargetPayload {
        version: TARGET_PAYLOAD_VERSION,
        target,
        source: source.clone(),
        app: app_identity.clone(),
        native,
        pins_digest: Digest::compute(
            Domain::DistPins,
            &wire::encode(&pins).expect("encode package fixture pins"),
        ),
        inventory_digest: Digest::compute(
            Domain::DistInventory,
            &wire::encode(&inventory).expect("encode package fixture inventory"),
        ),
        pins,
        inventory,
    };
    let payload = wire::encode(&payload).expect("encode package fixture payload");
    let payload_identity = Digest::compute(Domain::DistTargetPayload, &payload);
    let replay = Digest::compute(Domain::ProofReplayV3, target.as_str().as_bytes());
    let proof = ProofReceipt {
        version: PROOF_RECEIPT_VERSION,
        target,
        payload: payload_identity.clone(),
        battery: battery.clone(),
        replay: replay.clone(),
        environment: environment.clone(),
        standing: ProofStanding::Clean,
    };
    let proof = wire::encode(&proof).expect("encode package fixture proof");
    let sealed = ReleaseManifestTarget {
        target,
        payload: payload_identity,
        receipt: Digest::compute(Domain::ProofReceiptV3, &proof),
        battery: battery.clone(),
        replay,
        environment: environment.clone(),
        standing: ProofStanding::Clean,
    };
    (payload, proof, sealed)
}
