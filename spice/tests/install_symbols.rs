//! Installed macOS symbol verification over a package fixture, plus the one
//! macOS seam that resolves a live address through `dsymutil` and `dwarfdump`.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest as _, Sha256};
use spice::install::{
    InstallError, SymbolImage, SymbolVerificationError, apply, plan, verify_symbols,
};
use spice::repository::Repository;

#[path = "support/git_repository.rs"]
mod git_repository;
#[path = "support/install_package.rs"]
mod install_package_fixture;

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);
const DSYM_DWARF: &str = ".spice-install/bin/spice.dSYM/Contents/Resources/DWARF/spice";
const EXECUTABLE_UUID: [u8; 16] = [
    0xe3, 0x00, 0xc9, 0x74, 0x4b, 0xa0, 0x33, 0xdc, 0x9c, 0xf8, 0xa9, 0xb6, 0x39, 0x01, 0x90, 0x81,
];
const OTHER_UUID: [u8; 16] = [
    0x59, 0x19, 0xa3, 0xd6, 0x1f, 0x5d, 0x3a, 0x35, 0x83, 0x42, 0xda, 0xa9, 0x5d, 0x66, 0xf5, 0x6e,
];

#[test]
fn verification_reports_the_agreeing_receipt() {
    let scratch = Scratch::new("verify");
    let repository = scratch.repository();
    assert!(matches!(
        verify_symbols(&repository),
        Err(SymbolVerificationError::Install(
            InstallError::RecordMissing
        ))
    ));
    let (package, darwin, executable) = package_fixture(&scratch);
    let planned = plan(&repository, Some(&darwin)).unwrap();
    apply(&repository, Some(&darwin), &planned.digest().unwrap()).unwrap();

    let verification = verify_symbols(&repository).expect("installed pair verifies");
    assert_eq!(verification.uuid(), package.symbol_uuid());
    assert_eq!(
        verification.uuid(),
        install_package_fixture::format_uuid(&EXECUTABLE_UUID)
    );
    assert_eq!(
        verification.executable_sha256(),
        format!("{:x}", Sha256::digest(&executable))
    );
    assert_eq!(
        verification.commit(),
        install_package_fixture::FIXTURE_COMMIT
    );
    assert!(!verification.dirty());
    assert_eq!(verification.executable(), ".spice-install/bin/spice");
    assert_eq!(verification.dwarf(), DSYM_DWARF);
    assert_eq!(
        verification.dwarf_path(&repository),
        repository.common_dir().join(DSYM_DWARF)
    );
    let json = serde_json::to_value(&verification).unwrap();
    assert_eq!(json["uuid"], package.symbol_uuid());
    assert_eq!(json.get("text_address"), None);
    let app = repository.common_dir().join(".spice-install/app");
    let generation = fs::read_to_string(app.join("current")).unwrap();
    assert_eq!(
        fs::read(
            app.join("generations")
                .join(generation.trim())
                .join(wire::manifest::CANVAS_APP_HTML)
        )
        .unwrap(),
        package.app_bytes(wire::manifest::CANVAS_APP_HTML),
        "verification leaves the installed generation untouched"
    );
}

#[test]
fn verification_names_each_disagreement_with_the_observed_value() {
    let scratch = Scratch::new("disagree");
    let repository = scratch.repository();
    let (package, darwin, executable) = package_fixture(&scratch);
    let planned = plan(&repository, Some(&darwin)).unwrap();
    apply(&repository, Some(&darwin), &planned.digest().unwrap()).unwrap();

    let dwarf = repository.common_dir().join(DSYM_DWARF);
    let pristine = fs::read(&dwarf).unwrap();
    replace(
        &dwarf,
        &install_package_fixture::fabricate_macho(OTHER_UUID),
    );
    let error = verify_symbols(&repository).expect_err("dSYM UUID mismatch refuses");
    assert!(
        matches!(
            &error,
            SymbolVerificationError::Uuid { image: SymbolImage::Dwarf, path, recorded, observed }
                if path == DSYM_DWARF
                    && recorded == package.symbol_uuid()
                    && observed.as_deref() == Some(install_package_fixture::format_uuid(&OTHER_UUID).as_str())
        ),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        format!(
            "installed dSYM DWARF at {DSYM_DWARF} carries LC_UUID {}; receipt recorded {}",
            install_package_fixture::format_uuid(&OTHER_UUID),
            package.symbol_uuid()
        )
    );
    replace(&dwarf, b"not a Mach-O image");
    let error = verify_symbols(&repository).expect_err("unparseable dSYM refuses");
    assert!(
        matches!(
            &error,
            SymbolVerificationError::Uuid {
                image: SymbolImage::Dwarf,
                observed: None,
                ..
            }
        ),
        "{error:?}"
    );
    replace(&dwarf, &pristine);
    assert!(verify_symbols(&repository).is_ok());

    let binary = repository.common_dir().join(".spice-install/bin/spice");
    let mut tampered = executable.clone();
    tampered.extend_from_slice(b"\0trailing");
    replace(&binary, &tampered);
    let error = verify_symbols(&repository).expect_err("executable edit refuses");
    assert!(
        matches!(
            &error,
            SymbolVerificationError::ExecutableSha256 { expected, observed }
                if *expected == format!("{:x}", Sha256::digest(&executable))
                    && *observed == format!("{:x}", Sha256::digest(&tampered))
        ),
        "{error:?}"
    );
    replace(
        &binary,
        &install_package_fixture::fabricate_macho(OTHER_UUID),
    );
    let error = verify_symbols(&repository).expect_err("executable UUID mismatch refuses");
    assert!(
        matches!(
            &error,
            SymbolVerificationError::Uuid { image: SymbolImage::Executable, path, .. }
                if path == ".spice-install/bin/spice"
        ),
        "{error:?}"
    );
    replace(&binary, &executable);

    let info = repository
        .common_dir()
        .join(".spice-install/bin/spice.dSYM/Contents/Info.plist");
    let pristine_info = fs::read(&info).unwrap();
    replace(&info, b"<plist/>");
    let error = verify_symbols(&repository).expect_err("drifted Info.plist refuses");
    assert!(
        matches!(
            &error,
            SymbolVerificationError::Install(InstallError::Drift { .. })
        ),
        "{error:?}"
    );
    replace(&info, &pristine_info);
    assert!(verify_symbols(&repository).is_ok());
}

#[test]
fn the_command_replies_with_the_verification_and_refuses_an_unusable_sample() {
    let scratch = Scratch::new("cli");
    let repository = scratch.repository();
    let (package, darwin, _) = package_fixture(&scratch);
    let planned = plan(&repository, Some(&darwin)).unwrap();
    apply(&repository, Some(&darwin), &planned.digest().unwrap()).unwrap();
    let invoke = |arguments: &[&str]| {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_spice"))
            .arg("--json")
            .args(arguments)
            .current_dir(scratch.path())
            .env_remove(steer::hook::INBOX)
            .output()
            .expect("spice install symbols launches");
        // Replies speak on stdout; refusals and faults speak on stderr.
        let spoken = if output.status.success() {
            &output.stdout
        } else {
            &output.stderr
        };
        let body: serde_json::Value = serde_json::from_slice(spoken).unwrap_or_else(|error| {
            panic!(
                "{error}: stdout {:?} stderr {:?}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });
        (output.status.code(), body)
    };

    let (code, body) = invoke(&["install", "symbols"]);
    assert_eq!(code, Some(0), "{body}");
    assert_eq!(body["kind"], "reply");
    assert_eq!(body["body"]["verification"]["uuid"], package.symbol_uuid());
    assert_eq!(body["body"]["verification"]["dwarf"], DSYM_DWARF);
    assert_eq!(body["body"].get("sample"), None);

    let missing = scratch.join("missing-sample.txt");
    let (code, body) = invoke(&["install", "symbols", "--sample", missing.to_str().unwrap()]);
    assert_eq!(code, Some(2), "{body}");
    assert_eq!(body["diagnostic"]["reason"], "install::sample-refused");
    assert!(
        body["diagnostic"]["observed"]
            .as_str()
            .unwrap()
            .starts_with("failed to read sample "),
        "{body}"
    );

    let dwarf = repository.common_dir().join(DSYM_DWARF);
    replace(
        &dwarf,
        &install_package_fixture::fabricate_macho(OTHER_UUID),
    );
    let (code, body) = invoke(&["install", "symbols"]);
    assert_eq!(code, Some(2), "{body}");
    assert_eq!(body["diagnostic"]["reason"], "install::symbols-unverified");
    assert_eq!(
        body["diagnostic"]["observed"],
        format!(
            "installed dSYM DWARF at {DSYM_DWARF} carries LC_UUID {}; receipt recorded {}",
            install_package_fixture::format_uuid(&OTHER_UUID),
            package.symbol_uuid()
        )
    );
}

#[test]
fn a_symbol_free_install_refuses_before_reading_any_image() {
    let scratch = Scratch::new("bare");
    let repository = scratch.repository();
    let source = scratch.join("fixture-spice");
    fs::write(&source, b"#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
    let planned = plan(&repository, Some(&source)).unwrap();
    apply(&repository, Some(&source), &planned.digest().unwrap()).unwrap();
    let error = verify_symbols(&repository).expect_err("bare executable refuses");
    assert!(
        matches!(error, SymbolVerificationError::NoSymbols),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "installed build published no macOS symbols"
    );
}

/// The one macOS seam: a dSYM produced by the system linker tools resolves a
/// live address of this test executable, spelled the way `sample(1)` reports
/// it, into the named Rust function and its source location through the
/// production lookup adapter and resolver.
#[cfg(all(target_arch = "aarch64", target_vendor = "apple", target_os = "macos"))]
#[test]
fn a_sampled_live_address_resolves_through_dsymutil_and_dwarfdump() {
    use spice::install::{DwarfLookup, DwarfdumpLookup, SampleTarget};

    let scratch = Scratch::new("dsym");
    let executable = std::env::current_exe().unwrap();
    let dsym = scratch.join("probe.dSYM");
    let status = std::process::Command::new("dsymutil")
        .arg("-o")
        .arg(&dsym)
        .arg(&executable)
        .status()
        .expect("dsymutil launches");
    assert!(status.success(), "dsymutil exited with {status}");
    let image = executable.file_name().unwrap().to_str().unwrap();
    let dwarf = dsym.join("Contents/Resources/DWARF").join(image);
    let target = SampleTarget::from_image(
        executable.to_str().unwrap(),
        &fs::read(&executable).unwrap(),
        dwarf.clone(),
    );
    let text_address = target.text_address().expect("linked __TEXT address");
    // Image index zero is the main executable: this test binary.
    let slide = unsafe { _dyld_get_image_vmaddr_slide(0) };
    let load_address = text_address.strict_add_signed(i64::try_from(slide).unwrap());
    let runtime = probe_function as *const () as usize as u64;
    let sample = format!(
        "Load Address:    {load_address:#x}\n\nCall graph:\n    5 Thread_1\n\
         \x20   + 5 ???  (in {image})  load address {load_address:#x} + {:#x}  [{runtime:#x}]\n\
         \x20   +   5 ???  (in {image})  load address {load_address:#x} + 0x10  [{:#x}]\n\n\
         Binary Images:\n       {load_address:#x} - {:#x} +{image} (0) <{}> {}\n\n",
        runtime - load_address,
        load_address + 0x10,
        load_address + 0x1000,
        target.uuid(),
        executable.display(),
    );
    // The system shim under /usr/bin resolves the toolchain without HOME or
    // ambient developer variables.
    let environment = sill::Environment::new([("PATH", "/usr/bin:/bin")]).unwrap();
    let lookup = DwarfdumpLookup::new(scratch.path().to_path_buf(), environment);
    let report = target
        .resolve(Path::new("live-sample.txt"), &sample, &lookup)
        .expect("live sample resolves");
    assert_eq!((report.addresses(), report.resolved()), (2, 1));
    let probe = &report.frames()[0];
    assert_eq!(probe.samples(), 5);
    let frame = probe.frame().expect("probe address names its function");
    assert!(
        frame.function().ends_with("::probe_function"),
        "{}",
        frame.function()
    );
    assert!(
        frame
            .declared()
            .file()
            .ends_with("spice/tests/install_symbols.rs"),
        "{}",
        frame.declared().file()
    );
    assert_eq!(frame.declared().line(), PROBE_LINE);
    let line = frame.line().expect("the line table names the probe body");
    assert_eq!(line.file(), "install_symbols.rs");
    assert!(line.line() >= PROBE_LINE, "{}", line.line());
    assert_eq!(report.frames()[1].frame(), None);

    let error = lookup
        .lookup(&scratch.join("missing"), text_address)
        .expect_err("a missing DWARF fails the launch");
    assert!(
        error
            .to_string()
            .starts_with("dwarfdump exited with 1: error: ")
            && error
                .to_string()
                .ends_with("missing: No such file or directory"),
        "{error}"
    );
    assert_eq!(probe_function(), 7);
}

#[cfg(all(target_arch = "aarch64", target_vendor = "apple", target_os = "macos"))]
const PROBE_LINE: u64 = line!() as u64 + 3;
#[cfg(all(target_arch = "aarch64", target_vendor = "apple", target_os = "macos"))]
#[inline(never)]
fn probe_function() -> u32 {
    std::hint::black_box(7)
}

#[cfg(all(target_arch = "aarch64", target_vendor = "apple", target_os = "macos"))]
unsafe extern "C" {
    /// Runtime slide of the image at `image_index`, from the system loader.
    fn _dyld_get_image_vmaddr_slide(image_index: u32) -> isize;
}

fn package_fixture(
    scratch: &Scratch,
) -> (install_package_fixture::PackageFixture, PathBuf, Vec<u8>) {
    let executable = install_package_fixture::fabricate_macho(EXECUTABLE_UUID);
    let source = scratch.join("fixture-spice");
    fs::write(&source, &executable).unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
    let package =
        install_package_fixture::PackageFixture::create(scratch.join("release-package"), &source);
    let darwin = package
        .root()
        .join("targets/aarch64-apple-darwin/bin/spice");
    (package, darwin, executable)
}

/// Replace an installed read-only file in place, keeping its mode.
fn replace(path: &Path, bytes: &[u8]) {
    let mode = fs::metadata(path).unwrap().permissions().mode();
    fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        for _ in 0..100 {
            let sequence = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "spice-install-symbols-{label}-{}-{sequence}",
                std::process::id(),
            ));
            match fs::create_dir(&root) {
                Ok(()) => {
                    git_repository::initialize(&root);
                    fs::create_dir_all(root.join(".git/info")).unwrap();
                    fs::write(root.join(".git/info/exclude"), b"").unwrap();
                    return Self(root);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("create {}: {error}", root.display()),
            }
        }
        panic!("no unique install symbols fixture path was available");
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn repository(&self) -> Repository {
        Repository::discover(self.path()).unwrap()
    }

    fn join(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.0.join(relative)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.0)
            && !std::thread::panicking()
        {
            panic!("remove {}: {error}", self.0.display());
        }
    }
}
