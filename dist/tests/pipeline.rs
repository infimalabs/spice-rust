use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::{fs, io};

use dist::{
    BuildError, BuildStep, Builder, Canvas, CanvasError, FilePublisher, PinError, PinName,
    Pipeline, PublishError, Publisher, RequiredPins, StageError,
};
use wire::manifest::{
    BuildPins, MAC_SYMBOL_ARTIFACT_VERSION, MacSymbolArtifact, ProductVersion, ReleaseTarget,
    TargetPayload,
};
use wire::{Digest, Domain};

const OLD_PAYLOAD: &[u8] = b"old-payload";
const ARCHIVE: &[u8] = b"staged app ZIP";

fn pins() -> BuildPins {
    BuildPins {
        rust: "rustc 1.97.1".to_owned(),
        wasm_target: "wasm32-unknown-unknown".to_owned(),
        trunk: "trunk 0.21.14".to_owned(),
        cargo_mutants: "cargo-mutants 27.1.0".to_owned(),
        proof_container: format!("registry.example/spice-proof@sha256:{}", "0".repeat(64)),
    }
}

fn source_digest() -> Digest {
    Digest::from_str(&"1".repeat(64)).expect("fixture digest is canonical")
}

fn product_version() -> ProductVersion {
    ProductVersion::parse("1.2.3").expect("fixture version is canonical")
}

#[derive(Default)]
struct RecordingBuilder {
    calls: Vec<String>,
    symbols: Option<Vec<u8>>,
    windows_debug: Option<Vec<u8>>,
    linked: Option<&'static [u8]>,
    report: Option<String>,
}

impl Builder for RecordingBuilder {
    fn build_app(&mut self) -> Result<Canvas, BuildError> {
        self.calls.push("app".to_owned());
        Ok(canvas())
    }

    fn build_native(
        &mut self,
        target: ReleaseTarget,
        canvas: &Canvas,
    ) -> Result<Vec<u8>, BuildError> {
        self.calls.push(format!("native:{target}"));
        let linked = self.linked.unwrap_or(ARCHIVE);
        let native = match target {
            ReleaseTarget::Aarch64AppleDarwin => macho(linked),
            ReleaseTarget::X86_64UnknownLinuxMusl => elf(linked),
            ReleaseTarget::X86_64PcWindowsMsvc => pe(linked),
        };
        if target == ReleaseTarget::Aarch64AppleDarwin {
            self.symbols = Some(
                wire::encode(&MacSymbolArtifact {
                    version: MAC_SYMBOL_ARTIFACT_VERSION,
                    uuid: "11111111-2222-3333-4444-555555555555".to_owned(),
                    executable: Digest::compute(Domain::DistArtifact, &native),
                    commit: "1".repeat(40),
                    dirty: false,
                    info: b"fixture plist".to_vec(),
                    dwarf: b"fixture dwarf".to_vec(),
                })
                .expect("symbol fixture encodes"),
            );
        }
        if target == ReleaseTarget::X86_64PcWindowsMsvc {
            self.windows_debug = Some(b"fixture PDB".to_vec());
        }
        self.report = Some(format!("spice 1.2.3 app {}\n", canvas.generation()));
        Ok(native)
    }

    fn report_version(&mut self) -> Result<String, BuildError> {
        self.calls.push("version".to_owned());
        Ok(self.report.clone().expect("native ran first"))
    }

    fn take_symbol_artifact(&mut self) -> Option<Vec<u8>> {
        self.symbols.take()
    }

    fn take_windows_debug_artifact(&mut self) -> Option<Vec<u8>> {
        self.windows_debug.take()
    }
}

fn canvas() -> Canvas {
    Canvas::new(
        Digest::compute(Domain::DistArtifact, b"app generation"),
        ARCHIVE,
    )
}

/// A minimal 64-bit Mach-O image whose `__TEXT,__spiceapp` holds `archive`.
fn macho(archive: &[u8]) -> Vec<u8> {
    let name = |text: &str| {
        let mut field = [0_u8; 16];
        field[..text.len()].copy_from_slice(text.as_bytes());
        field
    };
    let offset = 32 + 72 + 80;
    let mut image = Vec::new();
    for word in [0xfeed_facf_u32, 0x0100_000c, 0, 2, 1, 72 + 80, 0, 0] {
        image.extend(word.to_le_bytes());
    }
    image.extend(0x19_u32.to_le_bytes());
    image.extend((72_u32 + 80).to_le_bytes());
    image.extend(name("__TEXT"));
    image.extend([0; 40]);
    image.extend(1_u32.to_le_bytes());
    image.extend(0_u32.to_le_bytes());
    image.extend(name("__spiceapp"));
    image.extend(name("__TEXT"));
    image.extend(0_u64.to_le_bytes());
    image.extend((archive.len() as u64).to_le_bytes());
    image.extend((offset as u32).to_le_bytes());
    image.extend([0; 28]);
    image.extend(archive);
    image
}

/// A minimal 64-bit ELF image whose `spiceapp` section holds `archive`.
fn elf(archive: &[u8]) -> Vec<u8> {
    let names = b"\0.shstrtab\0spiceapp\0";
    let canvas_at = 64 + names.len();
    let table = canvas_at + archive.len();
    let mut image = vec![0_u8; 64];
    image[..6].copy_from_slice(b"\x7fELF\x02\x01");
    image[0x28..0x30].copy_from_slice(&(table as u64).to_le_bytes());
    image[0x3a..0x3c].copy_from_slice(&64_u16.to_le_bytes());
    image[0x3c..0x3e].copy_from_slice(&3_u16.to_le_bytes());
    image[0x3e..0x40].copy_from_slice(&1_u16.to_le_bytes());
    image.extend(names);
    image.extend(archive);
    image.extend([0; 64]);
    for (name, kind, offset, bytes) in [
        (1_u32, 3_u32, 64, names.len()),
        (11, 1, canvas_at, archive.len()),
    ] {
        let mut header = [0_u8; 64];
        header[..4].copy_from_slice(&name.to_le_bytes());
        header[4..8].copy_from_slice(&kind.to_le_bytes());
        header[24..32].copy_from_slice(&(offset as u64).to_le_bytes());
        header[32..40].copy_from_slice(&(bytes as u64).to_le_bytes());
        image.extend(header);
    }
    image
}

/// A minimal PE32+ image whose `spiceapp` section holds `archive`.
fn pe(archive: &[u8]) -> Vec<u8> {
    const PE_HEADER: usize = 0x80;
    const OPTIONAL_HEADER: usize = 0xf0;
    const TABLE: usize = PE_HEADER + 4 + 20 + OPTIONAL_HEADER;
    const RAW: usize = 0x200;

    let mut image = vec![0_u8; RAW];
    image[0..2].copy_from_slice(b"MZ");
    image[0x3c..0x40].copy_from_slice(&(PE_HEADER as u32).to_le_bytes());
    image[PE_HEADER..PE_HEADER + 4].copy_from_slice(b"PE\0\0");
    let coff = PE_HEADER + 4;
    image[coff..coff + 2].copy_from_slice(&0x8664_u16.to_le_bytes());
    image[coff + 2..coff + 4].copy_from_slice(&1_u16.to_le_bytes());
    image[coff + 16..coff + 18].copy_from_slice(&(OPTIONAL_HEADER as u16).to_le_bytes());
    let optional = coff + 20;
    image[optional..optional + 2].copy_from_slice(&0x20b_u16.to_le_bytes());
    image[optional + 108..optional + 112].copy_from_slice(&16_u32.to_le_bytes());
    image[TABLE..TABLE + 8].copy_from_slice(b"spiceapp");
    image[TABLE + 8..TABLE + 12].copy_from_slice(&(archive.len() as u32).to_le_bytes());
    image[TABLE + 16..TABLE + 20].copy_from_slice(&(archive.len() as u32).to_le_bytes());
    image[TABLE + 20..TABLE + 24].copy_from_slice(&(RAW as u32).to_le_bytes());
    image.extend_from_slice(archive);
    image
}

#[derive(Default)]
struct MemoryPublisher {
    files: BTreeMap<PathBuf, (u32, Vec<u8>)>,
    writes: Vec<PathBuf>,
    fail_at: Option<usize>,
}

impl Publisher for MemoryPublisher {
    fn publish(&mut self, path: &Path, mode: u32, bytes: &[u8]) -> Result<(), PublishError> {
        let attempt = self.writes.len();
        self.writes.push(path.to_path_buf());
        if self.fail_at == Some(attempt) {
            return Err(PublishError::Io {
                path: path.to_path_buf(),
                source: io::Error::other("fixture interruption"),
            });
        }
        self.files
            .insert(path.to_path_buf(), (mode, bytes.to_vec()));
        Ok(())
    }
}

#[test]
fn pin_mismatch_refuses_before_the_first_build_or_write() {
    let required = pins();
    let mut observed = required.clone();
    observed.trunk = "trunk 0.21.13".to_owned();
    let pipeline = Pipeline::new(
        RequiredPins::new(required).expect("pins are valid"),
        product_version(),
    );
    let mut builder = RecordingBuilder::default();
    let mut publisher = MemoryPublisher::default();

    let error = pipeline
        .stage(
            ReleaseTarget::Aarch64AppleDarwin,
            source_digest(),
            &observed,
            &mut builder,
            &mut publisher,
        )
        .expect_err("pin drift must refuse");

    assert!(matches!(
        error,
        StageError::Pin(PinError::Mismatch {
            name: PinName::Trunk,
            ..
        })
    ));
    assert!(builder.calls.is_empty());
    assert!(publisher.writes.is_empty());
}

#[test]
fn build_order_payload_bytes_and_restaging_are_deterministic() {
    let required = pins();
    let pipeline = Pipeline::new(
        RequiredPins::new(required.clone()).expect("pins are valid"),
        product_version(),
    );
    let first = run_stage(&pipeline, &required, ReleaseTarget::Aarch64AppleDarwin);
    let second = run_stage(&pipeline, &required, ReleaseTarget::Aarch64AppleDarwin);

    assert_eq!(first.staged, second.staged);
    assert_eq!(first.publisher.files, second.publisher.files);
    assert_eq!(
        first.builder.calls,
        ["app", "native:aarch64-apple-darwin", "version"]
    );
    assert_eq!(
        first.publisher.writes.last(),
        Some(&PathBuf::from("targets/aarch64-apple-darwin/payload.json"))
    );
    let decoded: TargetPayload =
        wire::decode(&first.staged.payload_bytes).expect("payload is current wire");
    assert_eq!(decoded, first.staged.payload);
    assert_eq!(
        decoded
            .symbols
            .as_ref()
            .expect("Darwin payload carries symbols")
            .executable,
        decoded.native
    );
}

#[test]
fn a_final_executable_without_the_staged_archive_or_declared_identity_refuses_before_publication() {
    let required = pins();
    let pipeline = Pipeline::new(
        RequiredPins::new(required.clone()).expect("pins are valid"),
        product_version(),
    );
    for target in [
        ReleaseTarget::Aarch64AppleDarwin,
        ReleaseTarget::X86_64UnknownLinuxMusl,
    ] {
        let mut builder = RecordingBuilder {
            linked: Some(b"another canvas archive"),
            ..RecordingBuilder::default()
        };
        let mut publisher = MemoryPublisher::default();
        let error = pipeline
            .stage(
                target,
                source_digest(),
                &required,
                &mut builder,
                &mut publisher,
            )
            .expect_err("a different linked archive refuses");
        let expected = CanvasError::Mismatch {
            expected: canvas().archive().clone(),
            observed: Digest::compute(Domain::DistArtifact, b"another canvas archive"),
        };
        assert!(
            matches!(&error, StageError::Build { step: BuildStep::Native, source } if source.to_string() == expected.to_string()),
            "{error:?}"
        );
        assert_eq!(
            builder.calls,
            ["app".to_owned(), format!("native:{target}")]
        );
        assert!(publisher.writes.is_empty());
    }

    struct StaleReport(RecordingBuilder);
    impl Builder for StaleReport {
        fn build_app(&mut self) -> Result<Canvas, BuildError> {
            self.0.build_app()
        }
        fn build_native(
            &mut self,
            target: ReleaseTarget,
            canvas: &Canvas,
        ) -> Result<Vec<u8>, BuildError> {
            self.0.build_native(target, canvas)
        }
        fn report_version(&mut self) -> Result<String, BuildError> {
            Ok(format!("spice 1.2.3 app {}\n", "7".repeat(64)))
        }
    }
    let mut builder = StaleReport(RecordingBuilder::default());
    let mut publisher = MemoryPublisher::default();
    let error = pipeline
        .stage(
            ReleaseTarget::X86_64UnknownLinuxMusl,
            source_digest(),
            &required,
            &mut builder,
            &mut publisher,
        )
        .expect_err("a report naming another generation refuses");
    assert!(
        matches!(
            error,
            StageError::Build {
                step: BuildStep::Native,
                ..
            }
        ),
        "{error:?}"
    );
    assert!(publisher.writes.is_empty());

    struct StaleProductVersion(RecordingBuilder);
    impl Builder for StaleProductVersion {
        fn build_app(&mut self) -> Result<Canvas, BuildError> {
            self.0.build_app()
        }
        fn build_native(
            &mut self,
            target: ReleaseTarget,
            canvas: &Canvas,
        ) -> Result<Vec<u8>, BuildError> {
            self.0.build_native(target, canvas)
        }
        fn report_version(&mut self) -> Result<String, BuildError> {
            Ok(format!("spice 9.9.9 app {}\n", canvas().generation()))
        }
    }
    let mut builder = StaleProductVersion(RecordingBuilder::default());
    let mut publisher = MemoryPublisher::default();
    pipeline
        .stage(
            ReleaseTarget::X86_64UnknownLinuxMusl,
            source_digest(),
            &required,
            &mut builder,
            &mut publisher,
        )
        .expect_err("a report naming another product version refuses");
    assert!(publisher.writes.is_empty());
}

#[test]
fn interruption_at_every_publication_edge_preserves_the_old_target_pointer() {
    let required = pins();
    let pipeline = Pipeline::new(
        RequiredPins::new(required.clone()).expect("pins are valid"),
        product_version(),
    );
    let payload_path = PathBuf::from("targets/aarch64-apple-darwin/payload.json");
    for fail_at in 0..=2 {
        let mut publisher = MemoryPublisher {
            files: BTreeMap::from([(payload_path.clone(), (0o644, OLD_PAYLOAD.to_vec()))]),
            writes: Vec::new(),
            fail_at: Some(fail_at),
        };
        let mut builder = RecordingBuilder::default();

        let error = pipeline
            .stage(
                ReleaseTarget::Aarch64AppleDarwin,
                source_digest(),
                &required,
                &mut builder,
                &mut publisher,
            )
            .expect_err("publication interruption must surface");

        assert!(matches!(error, StageError::Publish { .. }));
        assert_eq!(publisher.files[&payload_path].1, OLD_PAYLOAD);
    }
}

#[test]
fn target_payload_pointers_and_native_identities_do_not_alias() {
    let required = pins();
    let pipeline = Pipeline::new(
        RequiredPins::new(required.clone()).expect("pins are valid"),
        product_version(),
    );
    let darwin = run_stage(&pipeline, &required, ReleaseTarget::Aarch64AppleDarwin);
    let linux = run_stage(&pipeline, &required, ReleaseTarget::X86_64UnknownLinuxMusl);
    let windows = run_stage(&pipeline, &required, ReleaseTarget::X86_64PcWindowsMsvc);

    assert_ne!(darwin.staged.payload_path, linux.staged.payload_path);
    assert_ne!(linux.staged.payload_path, windows.staged.payload_path);
    assert_ne!(darwin.staged.payload.native, linux.staged.payload.native);
    assert_ne!(linux.staged.payload.native, windows.staged.payload.native);
    assert_ne!(darwin.staged.payload_digest, linux.staged.payload_digest);
    assert_ne!(linux.staged.payload_digest, windows.staged.payload_digest);
    assert_eq!(darwin.staged.payload.app, linux.staged.payload.app);
    assert_eq!(linux.staged.payload.app, windows.staged.payload.app);
    assert_eq!(darwin.staged.payload.canvas, linux.staged.payload.canvas);
    assert_eq!(linux.staged.payload.canvas, windows.staged.payload.canvas);
    assert!(
        windows
            .staged
            .payload
            .inventory
            .iter()
            .any(|entry| { entry.path == "bin/spice.exe" && entry.mode == 0o755 })
    );
    assert!(
        windows
            .staged
            .payload
            .inventory
            .iter()
            .any(|entry| { entry.path == "symbols/spice.pdb" && entry.mode == 0o644 })
    );
}

#[test]
fn proof_container_must_be_bound_by_an_oci_digest() {
    let mut unpinned = pins();
    unpinned.proof_container = "registry.example/spice-proof:latest".to_owned();

    assert!(matches!(
        RequiredPins::new(unpinned),
        Err(PinError::ContainerNotDigestPinned { .. })
    ));

    let mut bare = pins();
    bare.proof_container = format!("sha256:{}", "0".repeat(64));
    RequiredPins::new(bare).expect("the compiled bare digest form is pinned");
}

#[test]
fn the_file_publisher_writes_bytes_and_mode_through_a_nested_relative_path() {
    let fixture = fixture_root();
    let root = fixture.join("stage");
    let mut publisher = FilePublisher::new(&root);

    publisher
        .publish(Path::new("objects/nested/artifact.bin"), 0o640, b"staged")
        .expect("a wholly relative path publishes");

    let published = root.join("objects/nested/artifact.bin");
    assert_eq!(
        fs::read(&published).expect("published bytes"),
        b"staged".to_vec(),
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let permissions = fs::metadata(&published)
            .expect("published metadata")
            .permissions();
        assert_eq!(
            permissions.mode() & 0o777,
            0o640,
            "the requested mode is not the published mode",
        );
    }
    fs::remove_dir_all(fixture).expect("remove fixture");
}

#[test]
fn the_file_publisher_refuses_a_stage_path_before_touching_the_filesystem() {
    let fixture = fixture_root();
    let root = fixture.join("stage");
    let mut publisher = FilePublisher::new(&root);

    // One case per way a path stops being wholly relative: no path at all, a
    // component that climbs out of the root, and a prefix that ignores it.
    for escape in ["", "../escape", "/absolute"] {
        let path = Path::new(escape);
        let error = publisher
            .publish(path, 0o640, b"escaped")
            .expect_err("a path that is not wholly relative is refused");
        assert!(
            matches!(&error, PublishError::InvalidPath(refused) if refused == path),
            "{escape:?} was refused as {error}, not as an invalid stage path",
        );
    }

    assert!(
        !root.exists(),
        "a refused publication still created its stage root",
    );
    fs::remove_dir_all(fixture).expect("remove fixture");
}

fn fixture_root() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "spice-dist-publisher-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&root).expect("fixture directory");
    root
}

struct Run {
    builder: RecordingBuilder,
    publisher: MemoryPublisher,
    staged: dist::StagedTarget,
}

fn run_stage(pipeline: &Pipeline, pins: &BuildPins, target: ReleaseTarget) -> Run {
    let mut builder = RecordingBuilder::default();
    let mut publisher = MemoryPublisher::default();
    let staged = pipeline
        .stage(target, source_digest(), pins, &mut builder, &mut publisher)
        .expect("fixture stage succeeds");
    Run {
        builder,
        publisher,
        staged,
    }
}
