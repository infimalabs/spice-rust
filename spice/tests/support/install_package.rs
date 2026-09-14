use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use wire::manifest::{
    BuildPins, CANVAS_APP_FILES, CANVAS_APP_HTML, InventoryEntry, MAC_SYMBOL_ARTIFACT_VERSION,
    MacSymbolArtifact, MacSymbolMetadata, PROOF_RECEIPT_VERSION, ProductVersion, ProofEnvironment,
    ProofReceipt, ProofStanding, RELEASE_MANIFEST_VERSION, ReleaseManifest, ReleaseManifestTarget,
    ReleaseTarget, TARGET_PAYLOAD_VERSION, TargetPayload,
};
use wire::{Digest, Domain};

const TARGETS: [ReleaseTarget; 2] = [
    ReleaseTarget::Aarch64AppleDarwin,
    ReleaseTarget::X86_64UnknownLinuxMusl,
];
const MAX_FIXTURE_MEMBER_BYTES: u64 = 512 * 1024 * 1024;
/// Clean commit the fixture claims every target executable was linked from.
pub(crate) const FIXTURE_COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
/// LC_UUID the fixture fabricates when the executable is not a Mach-O image.
pub(crate) const FALLBACK_UUID: [u8; 16] = [
    0x11, 0x11, 0x11, 0x11, 0x22, 0x22, 0x33, 0x33, 0x44, 0x44, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
];
const MH_MAGIC_64: u32 = 0xFEED_FACF;
const LC_UUID: u32 = 0x1B;

pub(crate) struct PackageFixture {
    root: PathBuf,
    app: Vec<(String, Vec<u8>)>,
    symbols: SymbolFixture,
}

/// The symbol artifact the fixture bound to its Darwin executable.
struct SymbolFixture {
    uuid: String,
    artifact: Vec<u8>,
}

impl SymbolFixture {
    fn create(executable: &[u8], native: &Digest) -> Self {
        let raw = macho_uuid(executable).unwrap_or(FALLBACK_UUID);
        let uuid = format_uuid(&raw);
        let info = format!(
            concat!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
                "<plist version=\"1.0\"><dict>",
                "<key>CFBundleIdentifier</key><string>com.apple.xcode.dsym.spice</string>",
                "<key>dSYM_UUID</key><string>{uuid}</string>",
                "</dict></plist>\n"
            ),
            uuid = uuid
        );
        let artifact = MacSymbolArtifact {
            version: MAC_SYMBOL_ARTIFACT_VERSION,
            uuid: uuid.clone(),
            executable: native.clone(),
            commit: FIXTURE_COMMIT.to_owned(),
            dirty: false,
            info: info.into_bytes(),
            dwarf: fabricate_macho(raw),
        };
        let artifact = wire::encode(&artifact).expect("encode package fixture symbol artifact");
        Self { uuid, artifact }
    }

    fn artifact(&self) -> &[u8] {
        &self.artifact
    }

    fn metadata(&self, native: &Digest) -> MacSymbolMetadata {
        MacSymbolMetadata {
            path: "symbols/spice.dSYM.json".to_owned(),
            mode: 0o644,
            bytes: self.artifact.len() as u64,
            digest: Digest::compute(Domain::DistArtifact, &self.artifact),
            uuid: self.uuid.clone(),
            executable: native.clone(),
        }
    }
}

/// The smallest thin 64-bit Mach-O image carrying exactly one LC_UUID.
pub(crate) fn fabricate_macho(uuid: [u8; 16]) -> Vec<u8> {
    let mut image = Vec::with_capacity(56);
    image.extend_from_slice(&MH_MAGIC_64.to_le_bytes());
    image.extend_from_slice(&0x0100_000C_u32.to_le_bytes());
    image.extend_from_slice(&0_u32.to_le_bytes());
    image.extend_from_slice(&2_u32.to_le_bytes());
    image.extend_from_slice(&1_u32.to_le_bytes());
    image.extend_from_slice(&24_u32.to_le_bytes());
    image.extend_from_slice(&0_u32.to_le_bytes());
    image.extend_from_slice(&0_u32.to_le_bytes());
    image.extend_from_slice(&LC_UUID.to_le_bytes());
    image.extend_from_slice(&24_u32.to_le_bytes());
    image.extend_from_slice(&uuid);
    image
}

/// Read the sole LC_UUID of a thin 64-bit little-endian Mach-O image.
pub(crate) fn macho_uuid(bytes: &[u8]) -> Option<[u8; 16]> {
    let word = |offset: usize| {
        bytes
            .get(offset..offset + 4)
            .map(|word| u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
    };
    if word(0)? != MH_MAGIC_64 {
        return None;
    }
    let commands = word(16)?;
    let mut offset = 32;
    for _ in 0..commands {
        let command = word(offset)?;
        let length = word(offset + 4)? as usize;
        if command == LC_UUID {
            let mut uuid = [0; 16];
            uuid.copy_from_slice(bytes.get(offset + 8..offset + 24)?);
            return Some(uuid);
        }
        offset += length.max(8);
    }
    None
}

pub(crate) fn format_uuid(raw: &[u8; 16]) -> String {
    let hex = raw
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<String>();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

impl PackageFixture {
    pub(crate) fn create(root: PathBuf, executable: &Path) -> Self {
        fs::create_dir(&root).expect("create package fixture root");
        let executable_path = executable;
        let executable = fs::read(executable_path).expect("read package fixture executable");
        let source = Digest::compute(Domain::DistSourceCommit, FIXTURE_COMMIT.as_bytes());
        let symbols = SymbolFixture::create(
            &executable,
            &Digest::compute(Domain::DistArtifact, &executable),
        );
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
                &symbols,
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
                    target_input(
                        target,
                        executable_path,
                        (&payloads[index], &proofs[index]),
                        &symbols,
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
        Self { root, app, symbols }
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Mach-O UUID the fixture bound its Darwin symbol artifact to.
    pub(crate) fn symbol_uuid(&self) -> &str {
        &self.symbols.uuid
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

/// One package target member set; only the Darwin target carries symbols.
fn target_input(
    target: ReleaseTarget,
    executable: &Path,
    (payload, proof): (&[u8], &[u8]),
    symbols: &SymbolFixture,
) -> proof::PackageTargetInput {
    proof::PackageTargetInput::new(
        target,
        file_member(executable, &format!("targets/{target}/bin/spice"), 0o755),
        inline(&format!("targets/{target}/payload.json"), 0o644, payload),
        inline(&format!("targets/{target}/proof.json"), 0o644, proof),
    )
    .with_symbols((target == ReleaseTarget::Aarch64AppleDarwin).then(|| {
        inline(
            &format!("targets/{target}/symbols/spice.dSYM.json"),
            0o644,
            symbols.artifact(),
        )
    }))
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
    symbols: &SymbolFixture,
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
    let symbols = (target == ReleaseTarget::Aarch64AppleDarwin).then(|| {
        let metadata = symbols.metadata(&native);
        inventory.push(InventoryEntry {
            path: metadata.path.clone(),
            mode: metadata.mode,
            bytes: metadata.bytes,
            digest: metadata.digest.clone(),
        });
        metadata
    });
    inventory.sort_by(|left, right| left.path.cmp(&right.path));
    let manifest_inventory = inventory.clone();
    let payload = TargetPayload {
        version: TARGET_PAYLOAD_VERSION,
        target,
        source: source.clone(),
        app: app_identity.clone(),
        native,
        symbols: symbols.clone(),
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
        payload_bytes: payload.len() as u64,
        inventory: manifest_inventory,
        symbols,
        receipt: Digest::compute(Domain::ProofReceiptV3, &proof),
        receipt_bytes: proof.len() as u64,
        battery: battery.clone(),
        replay,
        environment: environment.clone(),
        standing: ProofStanding::Clean,
    };
    (payload, proof, sealed)
}
