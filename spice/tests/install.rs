#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use spice::install::{
    ApplianceState, InstallError, InstallStatus, apply, audit_appliance, capture_generation,
    installed_bash_env, installed_binary, installed_git_hooks, installed_skill,
    installed_spice_wrapper, installed_zdotdir, plan, publish_test_historical_receipt, unapply,
};
use spice::repository::Repository;
use wire::manifest::{MacSymbolArtifact, TargetPayload};
use wire::{Digest, Domain};

#[path = "support/git_repository.rs"]
mod git_repository;
#[path = "support/install_package.rs"]
mod install_package_fixture;

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);
const MAX_COMPACT_RECORD_BYTES: u64 = 64 * 1024;
const RETIRED_FOLD_STORE: &str = ".git/.spice-rs/data/spicefolds.sqlite3";

#[test]
fn obsolete_fold_store_is_deleted_without_payload_retention_or_restoration() {
    let scratch = Scratch::new("retire-fold-store");
    let repository = scratch.repository();
    let store = scratch.join(RETIRED_FOLD_STORE);
    let journal = scratch.join(".git/.spice-rs/data/spicejournal.sqlite3");
    let unknown = scratch.join(".git/.spice-rs/data/operator-state");
    fs::create_dir_all(store.parent().unwrap()).unwrap();
    fs::write(&store, b"obsolete fold bytes\n").unwrap();
    fs::set_permissions(&store, fs::Permissions::from_mode(0o640)).unwrap();
    fs::write(&journal, b"journal authority\n").unwrap();
    fs::write(&unknown, b"operator state\n").unwrap();

    let planned = plan(&repository, None).unwrap();
    let retirement = planned
        .retirements()
        .iter()
        .find(|retirement| retirement.path() == ".spice-rs/data/spicefolds.sqlite3")
        .expect("current plan retires the obsolete fold store");
    assert!(retirement.previous().is_none());
    let document: serde_json::Value =
        serde_json::from_slice(&planned.canonical_bytes().unwrap()).unwrap();
    let retirement = document["retirements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|retirement| retirement["path"] == ".spice-rs/data/spicefolds.sqlite3")
        .unwrap();
    assert_eq!(retirement["discard"], true);
    assert!(retirement.get("superseded").is_some());
    for payload in ["previous", "observed", "rollback"] {
        assert!(
            retirement.get(payload).is_none(),
            "discard retained {payload}"
        );
    }
    assert!(planned.canonical_bytes().unwrap().len() < MAX_COMPACT_RECORD_BYTES as usize);

    apply(&repository, None, &planned.digest().unwrap()).unwrap();
    assert!(!store.exists());
    assert_eq!(fs::read(&journal).unwrap(), b"journal authority\n");
    assert_eq!(fs::read(&unknown).unwrap(), b"operator state\n");

    assert_eq!(
        unapply(&repository).unwrap().status(),
        InstallStatus::Uninstalled
    );
    assert!(!store.exists());
    assert_eq!(fs::read(&journal).unwrap(), b"journal authority\n");
    assert_eq!(fs::read(&unknown).unwrap(), b"operator state\n");
}

#[test]
fn completed_fold_retirement_allows_parent_progress_but_refuses_recreation() {
    let scratch = Scratch::new("completed-fold-retirement");
    let repository = scratch.repository();
    let store = scratch.join(RETIRED_FOLD_STORE);
    fs::create_dir_all(store.parent().unwrap()).unwrap();
    fs::write(&store, b"obsolete fold bytes\n").unwrap();
    let source = Path::new(env!("CARGO_BIN_EXE_spice"));
    let planned = plan(&repository, Some(source)).unwrap();
    apply(&repository, Some(source), &planned.digest().unwrap()).unwrap();
    assert!(!store.exists());

    fs::write(
        scratch.join(".git/.spice-rs/data/unrelated-progress"),
        b"unrelated state\n",
    )
    .unwrap();
    let installed = installed_binary(&repository);
    assert_eq!(
        audit_appliance(&repository, &installed).unwrap().state(),
        ApplianceState::Current,
    );

    fs::write(&store, b"reintroduced fold bytes\n").unwrap();
    assert_eq!(
        audit_appliance(&repository, &installed).unwrap().state(),
        ApplianceState::AssetTampered,
    );
}

#[test]
fn obsolete_fold_store_symlink_and_non_regular_path_are_inert() {
    let linked = Scratch::new("linked-fold-store");
    let repository = linked.repository();
    let store = linked.join(RETIRED_FOLD_STORE);
    let outside = linked.join("outside-fold-store");
    fs::create_dir_all(store.parent().unwrap()).unwrap();
    fs::write(&outside, b"outside bytes\n").unwrap();
    symlink(&outside, &store).unwrap();
    let before = linked.snapshot();
    let error = plan(&repository, None).unwrap_err();
    assert!(
        matches!(&error,
            InstallError::PathShape(path) | InstallError::Io { path, .. }
                if path.ends_with(".spice-rs/data/spicefolds.sqlite3")
        ),
        "{error:?}"
    );
    assert_eq!(linked.snapshot(), before);
    assert_eq!(fs::read(&outside).unwrap(), b"outside bytes\n");

    let directory = Scratch::new("directory-fold-store");
    let repository = directory.repository();
    let store = directory.join(RETIRED_FOLD_STORE);
    fs::create_dir_all(&store).unwrap();
    fs::write(store.join("unknown"), b"unknown bytes\n").unwrap();
    let before = directory.snapshot();
    let error = plan(&repository, None).unwrap_err();
    assert!(
        matches!(&error,
            InstallError::PathShape(path) | InstallError::Io { path, .. }
                if path.ends_with(".spice-rs/data/spicefolds.sqlite3")
        ),
        "{error:?}"
    );
    assert_eq!(directory.snapshot(), before);
}

#[test]
fn version_14_receipt_refreshes_into_the_symbol_bearing_schema_before_unapply() {
    let scratch = Scratch::new("version-14-receipt");
    let repository = scratch.repository();
    let store = scratch.join(RETIRED_FOLD_STORE);
    let current = plan(&repository, None).unwrap();
    apply(&repository, None, &current.digest().unwrap()).unwrap();
    publish_test_historical_receipt(&repository, 14).unwrap();
    let legacy: serde_json::Value =
        serde_json::from_slice(&fs::read(scratch.record()).unwrap()).unwrap();
    assert_eq!(legacy["version"], 14);
    assert_eq!(legacy["plan"]["version"], 14);
    assert!(legacy["plan"].get("symbols").is_none());
    let retirement = legacy["plan"]["retirements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|retirement| retirement["path"] == ".spice-rs/data/spicefolds.sqlite3")
        .unwrap();
    assert_eq!(retirement["discard"], true);
    let error = unapply(&repository).unwrap_err();
    assert!(matches!(error, InstallError::InvalidRecord(detail)
            if detail.contains("INSTALL-1kSWFBN0") && detail.contains("before unapply")));

    let planned = plan(&repository, None).unwrap();
    assert_eq!(planned.version(), 15);
    assert!(planned.symbol_provenance().is_none());
    assert!(!planned.has_rollback_archive());
    assert_eq!(planned.retirements().len(), 3);
    apply(&repository, None, &planned.digest().unwrap()).unwrap();
    assert!(!store.exists());
    assert!(fs::metadata(scratch.record()).unwrap().len() < MAX_COMPACT_RECORD_BYTES);

    unapply(&repository).unwrap();
    assert!(!store.exists());
}

#[test]
fn compact_migration_deletes_without_archiving_or_restoring_the_obsolete_fold_store() {
    let scratch = Scratch::new("archived-fold-store");
    let repository = scratch.repository();
    let source = scratch.join("fixture-spice");
    fs::write(&source, b"#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
    let current = plan(&repository, Some(&source)).unwrap();
    apply(&repository, Some(&source), &current.digest().unwrap()).unwrap();
    publish_test_historical_receipt(&repository, 7).unwrap();
    let store = scratch.join(RETIRED_FOLD_STORE);
    fs::create_dir_all(store.parent().unwrap()).unwrap();
    fs::write(&store, b"archived fold bytes\n").unwrap();

    let planned = plan(&repository, Some(&source)).unwrap();
    assert!(planned.has_rollback_archive());
    let retirement = planned
        .retirements()
        .iter()
        .find(|retirement| retirement.path() == ".spice-rs/data/spicefolds.sqlite3")
        .expect("compact migration adds the fold-store retirement");
    assert!(retirement.previous().is_none());
    apply(&repository, Some(&source), &planned.digest().unwrap()).unwrap();
    assert!(!store.exists());

    unapply(&repository).unwrap();
    assert!(!store.exists());
}

const DSYM_INFO: &str = ".git/.spice-install/bin/spice.dSYM/Contents/Info.plist";
const DSYM_DWARF: &str = ".git/.spice-install/bin/spice.dSYM/Contents/Resources/DWARF/spice";
const EXECUTABLE_UUID: [u8; 16] = [
    0xA0, 0xA1, 0xA2, 0xA3, 0xB0, 0xB1, 0xC0, 0xC1, 0xD0, 0xD1, 0xE0, 0xE1, 0xE2, 0xE3, 0xE4, 0xE5,
];
const OTHER_UUID: [u8; 16] = [
    0x0F, 0x0E, 0x0D, 0x0C, 0x0B, 0x0A, 0x09, 0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01, 0x00,
];

#[test]
fn package_install_publishes_the_executable_and_its_dsym_as_one_generation() {
    let scratch = Scratch::new("package-symbols");
    let repository = scratch.repository();
    let (package, darwin, executable) = package_fixture(&scratch, "release-package");

    let planned = plan(&repository, Some(&darwin)).unwrap();
    let provenance = planned
        .symbol_provenance()
        .expect("Darwin plan carries provenance");
    assert_eq!(provenance.uuid(), package.symbol_uuid());
    assert_eq!(
        provenance.uuid(),
        install_package_fixture::format_uuid(&EXECUTABLE_UUID)
    );
    assert_eq!(provenance.commit(), install_package_fixture::FIXTURE_COMMIT);
    assert!(!provenance.dirty());
    assert_eq!(provenance.path(), ".spice-install/bin/spice.dSYM");
    assert_eq!(provenance.executable_sha256(), sha256_hex(&executable));
    let tail = planned
        .operations()
        .iter()
        .rev()
        .take(3)
        .map(|operation| operation.path().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        tail,
        [
            ".spice-install/bin/spice.dSYM/Contents/Resources/DWARF/spice",
            ".spice-install/bin/spice.dSYM/Contents/Info.plist",
            ".spice-install/bin/spice",
        ]
    );
    assert_eq!(planned.retirements().len(), 3);
    let document: serde_json::Value =
        serde_json::from_slice(&planned.canonical_bytes().unwrap()).unwrap();
    assert_eq!(document["symbols"]["uuid"], package.symbol_uuid());
    assert_eq!(document["symbols"]["dirty"], false);

    let receipt = apply(&repository, Some(&darwin), &planned.digest().unwrap()).unwrap();
    assert_eq!(receipt.status(), InstallStatus::Complete);
    assert_eq!(receipt.plan().symbols(), Some(provenance));
    assert_eq!(fs::read(installed_binary(&repository)).unwrap(), executable);
    let info = fs::read(scratch.join(DSYM_INFO)).unwrap();
    assert!(
        String::from_utf8(info)
            .unwrap()
            .contains(package.symbol_uuid())
    );
    assert_eq!(
        fs::read(scratch.join(DSYM_DWARF)).unwrap(),
        install_package_fixture::fabricate_macho(EXECUTABLE_UUID)
    );
    for path in [DSYM_INFO, DSYM_DWARF] {
        let mode = fs::metadata(scratch.join(path))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o644, "{path}");
    }
    let app = repository.common_dir().join(".spice-install/app");
    let generation = fs::read_to_string(app.join("current")).unwrap();
    assert_eq!(
        fs::read(
            app.join("generations")
                .join(generation.trim())
                .join(wire::manifest::CANVAS_APP_HTML)
        )
        .unwrap(),
        package.app_bytes(wire::manifest::CANVAS_APP_HTML)
    );
    assert_eq!(
        audit_appliance(&repository, &darwin).unwrap().state(),
        ApplianceState::Current
    );

    // Tampering with either half of the bundle is drift of the one generation.
    let dwarf = scratch.join(DSYM_DWARF);
    let retained = fs::read(&dwarf).unwrap();
    fs::write(&dwarf, install_package_fixture::fabricate_macho(OTHER_UUID)).unwrap();
    assert_eq!(
        audit_appliance(&repository, &darwin).unwrap().state(),
        ApplianceState::AssetTampered
    );
    assert!(matches!(
        unapply(&repository).unwrap_err(),
        InstallError::Drift { .. }
    ));
    fs::write(&dwarf, retained).unwrap();

    assert_eq!(
        unapply(&repository).unwrap().status(),
        InstallStatus::Uninstalled
    );
    assert!(!installed_binary(&repository).exists());
    assert!(!scratch.join(DSYM_INFO).exists());
    assert!(!scratch.join(DSYM_DWARF).exists());
}

#[test]
fn refreshing_between_symbol_bearing_and_bare_executables_moves_the_bundle_as_one() {
    let scratch = Scratch::new("package-symbol-refresh");
    let repository = scratch.repository();
    let (package, darwin, executable) = package_fixture(&scratch, "release-package");
    let linux = package
        .root()
        .join("targets/x86_64-unknown-linux-musl/bin/spice");
    let bare = plan(&repository, Some(&linux)).unwrap();
    assert!(bare.symbol_provenance().is_none());
    assert_eq!(bare.retirements().len(), 3);
    apply(&repository, Some(&linux), &bare.digest().unwrap()).unwrap();
    assert!(!scratch.join(DSYM_INFO).exists());

    // Adding symbols on refresh is a pure addition.
    assert_eq!(
        audit_appliance(&repository, &darwin).unwrap().state(),
        ApplianceState::AnotherBuild
    );
    let symbolic = plan(&repository, Some(&darwin)).unwrap();
    assert!(symbolic.symbol_provenance().is_some());
    assert_eq!(symbolic.retirements().len(), 3);
    let receipt = apply(&repository, Some(&darwin), &symbolic.digest().unwrap()).unwrap();
    assert_eq!(
        receipt.plan().symbols().map(|symbols| symbols.uuid()),
        Some(package.symbol_uuid())
    );
    assert_eq!(fs::read(installed_binary(&repository)).unwrap(), executable);
    assert!(scratch.join(DSYM_DWARF).is_file());
    assert_eq!(
        audit_appliance(&repository, &darwin).unwrap().state(),
        ApplianceState::Current
    );

    // Dropping symbols discards the retained bundle with the executable it
    // described, and the discard is derived only for that transition.
    assert_eq!(
        audit_appliance(&repository, &linux).unwrap().state(),
        ApplianceState::AnotherBuild
    );
    let dropped = plan(&repository, Some(&linux)).unwrap();
    assert!(dropped.symbol_provenance().is_none());
    let discards = dropped
        .retirements()
        .iter()
        .skip(3)
        .map(|retirement| {
            (
                retirement.path().to_owned(),
                retirement.previous().is_none(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        discards,
        [
            (
                ".spice-install/bin/spice.dSYM/Contents/Info.plist".to_owned(),
                true
            ),
            (
                ".spice-install/bin/spice.dSYM/Contents/Resources/DWARF/spice".to_owned(),
                true
            ),
        ]
    );
    let document: serde_json::Value =
        serde_json::from_slice(&dropped.canonical_bytes().unwrap()).unwrap();
    assert!(document.get("symbols").is_none());
    assert_eq!(document["retirements"][3]["discard"], true);
    let receipt = apply(&repository, Some(&linux), &dropped.digest().unwrap()).unwrap();
    assert!(receipt.plan().symbols().is_none());
    assert!(!scratch.join(DSYM_INFO).exists());
    assert!(!scratch.join(DSYM_DWARF).exists());
    assert!(fs::metadata(scratch.record()).unwrap().len() < MAX_COMPACT_RECORD_BYTES);
    assert_eq!(
        audit_appliance(&repository, &linux).unwrap().state(),
        ApplianceState::Current
    );
    let settled = plan(&repository, Some(&linux)).unwrap();
    assert_eq!(settled.digest().unwrap(), *receipt.digest());
    assert_eq!(settled.retirements().len(), 5);

    // Re-adding symbols over the discarding generation derives no discard,
    // so the retirement never accumulates across generations.
    let restored = plan(&repository, Some(&darwin)).unwrap();
    assert!(restored.symbol_provenance().is_some());
    assert_eq!(restored.retirements().len(), 3);
    apply(&repository, Some(&darwin), &restored.digest().unwrap()).unwrap();
    assert!(scratch.join(DSYM_DWARF).is_file());
    let dropped_again = plan(&repository, Some(&linux)).unwrap();
    assert_eq!(dropped_again.retirements().len(), 5);

    assert_eq!(
        unapply(&repository).unwrap().status(),
        InstallStatus::Uninstalled
    );
    assert!(!installed_binary(&repository).exists());
    // Refreshed generations leave only the directory skeleton behind, exactly
    // as every other refreshed install does; no bundle file survives.
    assert!(!scratch.join(DSYM_INFO).exists());
    assert!(!scratch.join(DSYM_DWARF).exists());
    let mut residue = Vec::new();
    snapshot_walk(
        scratch.path(),
        &scratch.join(".git/.spice-install"),
        &mut residue,
    );
    assert!(
        residue
            .iter()
            .all(|(_, _, node)| matches!(node, Node::Directory)),
        "{residue:?}"
    );
}

#[test]
fn absent_nonregular_mismatched_and_misbound_symbol_inputs_refuse_before_publication() {
    let scratch = Scratch::new("package-symbol-refusals");
    let repository = scratch.repository();
    let (package, darwin, _) = package_fixture(&scratch, "release-package");
    let target = package.root().join("targets/aarch64-apple-darwin");
    let artifact = target.join("symbols/spice.dSYM.json");
    let pristine = fs::read(&artifact).unwrap();
    let refusal = |scratch: &Scratch| {
        let before = scratch.snapshot();
        let error = plan(&repository, Some(&darwin)).unwrap_err();
        assert_eq!(scratch.snapshot(), before);
        assert!(!scratch.record().exists());
        match error {
            InstallError::SymbolsRefused { path, reason } => (path, reason),
            other => panic!("expected a symbol refusal, got {other:?}"),
        }
    };

    fs::remove_file(&artifact).unwrap();
    assert_eq!(refusal(&scratch), (artifact.clone(), "is absent"));
    fs::create_dir(&artifact).unwrap();
    assert_eq!(
        refusal(&scratch),
        (artifact.clone(), "is not a regular file")
    );
    fs::remove_dir(&artifact).unwrap();
    fs::write(target.join("elsewhere.json"), &pristine).unwrap();
    symlink("../elsewhere.json", &artifact).unwrap();
    assert_eq!(
        refusal(&scratch),
        (artifact.clone(), "is not a regular file")
    );
    fs::remove_file(&artifact).unwrap();

    let mut grown = pristine.clone();
    grown.push(b'\n');
    fs::write(&artifact, &grown).unwrap();
    assert_eq!(
        refusal(&scratch),
        (artifact.clone(), "differs from the authenticated payload")
    );
    fs::write(&artifact, &pristine).unwrap();
    fs::set_permissions(&artifact, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        refusal(&scratch),
        (artifact.clone(), "differs from the authenticated payload")
    );
    fs::set_permissions(&artifact, fs::Permissions::from_mode(0o644)).unwrap();

    forge_symbols(&target, &pristine, |forged| {
        forged.uuid = install_package_fixture::format_uuid(&OTHER_UUID);
    });
    assert_eq!(
        refusal(&scratch),
        (artifact.clone(), "UUID differs from the executable")
    );
    forge_symbols(&target, &pristine, |forged| {
        forged.dwarf = install_package_fixture::fabricate_macho(OTHER_UUID);
    });
    assert_eq!(
        refusal(&scratch),
        (artifact.clone(), "DWARF UUID differs from the executable")
    );
    forge_symbols(&target, &pristine, |forged| forged.commit = "f".repeat(40));
    assert_eq!(
        refusal(&scratch),
        (artifact.clone(), "names another commit than the payload")
    );
    forge_symbols(&target, &pristine, |forged| {
        forged.commit = install_package_fixture::FIXTURE_COMMIT.to_uppercase();
    });
    assert_eq!(
        refusal(&scratch),
        (artifact.clone(), "names another commit than the payload")
    );
    forge_symbols(&target, &pristine, |forged| forged.dirty = true);
    let planned = plan(&repository, Some(&darwin)).unwrap();
    assert!(planned.symbol_provenance().unwrap().dirty());
    forge_symbols(&target, &pristine, |forged| forged.info.clear());
    assert_eq!(
        refusal(&scratch),
        (
            artifact.clone(),
            "is not a canonical current symbol artifact"
        )
    );

    // An executable without a Mach-O UUID cannot be bound to any dSYM.
    let script = scratch.join("script-spice");
    fs::write(&script, b"#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let scripted =
        install_package_fixture::PackageFixture::create(scratch.join("script-package"), &script);
    let scripted_darwin = scripted
        .root()
        .join("targets/aarch64-apple-darwin/bin/spice");
    let error = plan(&repository, Some(&scripted_darwin)).unwrap_err();
    assert!(matches!(
        error,
        InstallError::SymbolsRefused { path, reason: "carries no thin 64-bit Mach-O LC_UUID" }
            if path == scripted_darwin
    ));
    assert!(!scratch.record().exists());
}

#[test]
fn records_that_do_not_establish_symbol_provenance_are_refused_inertly() {
    let scratch = Scratch::new("package-symbol-records");
    let repository = scratch.repository();
    let (_, darwin, _) = package_fixture(&scratch, "release-package");
    let planned = plan(&repository, Some(&darwin)).unwrap();
    let receipt = apply(&repository, Some(&darwin), &planned.digest().unwrap()).unwrap();
    let pristine = fs::read(scratch.record()).unwrap();
    let symbols = receipt.plan().symbols().unwrap();
    let sha = format!("\"executable_sha256\":\"{}\"", symbols.executable_sha256());
    let uuid = format!("\"uuid\":\"{}\"", symbols.uuid());
    let commit = format!("\"commit\":\"{}\"", symbols.commit());
    let path = format!("\"path\":\"{}\"", symbols.path());
    let provenance = "install symbol provenance is not canonical";
    let together = "symbol provenance and retained symbol operations must be planned together";
    let closing = "optional executable and symbols do not close the operation inventory";
    let cases = [
        (
            "sha256 too short",
            provenance,
            sha.clone(),
            "\"executable_sha256\":\"abc\"".to_owned(),
        ),
        (
            "sha256 uppercase",
            provenance,
            sha.clone(),
            format!(
                "\"executable_sha256\":\"{}\"",
                symbols.executable_sha256().to_uppercase()
            ),
        ),
        (
            "uuid lowercase",
            provenance,
            uuid.clone(),
            format!("\"uuid\":\"{}\"", symbols.uuid().to_lowercase()),
        ),
        (
            "commit short",
            provenance,
            commit.clone(),
            "\"commit\":\"0123456\"".to_owned(),
        ),
        (
            "commit uppercase",
            provenance,
            commit.clone(),
            format!("\"commit\":\"{}\"", symbols.commit().to_uppercase()),
        ),
        (
            "path moved",
            provenance,
            path.clone(),
            path.replace("spice.dSYM", "other.dSYM"),
        ),
        (
            "provenance missing",
            together,
            format!(",\"symbols\":{{{sha},{uuid},{commit},\"dirty\":false,{path}}}"),
            String::new(),
        ),
        (
            "dwarf operation missing",
            closing,
            "\"symbols-dwarf\"".to_owned(),
            "\"symbols-info\"".to_owned(),
        ),
    ];
    for (label, expected, from, to) in cases {
        fs::write(scratch.record(), tamper_plan(&pristine, &from, &to)).unwrap();
        let before = scratch.snapshot();
        for error in [
            unapply(&repository).unwrap_err(),
            plan(&repository, Some(&darwin)).unwrap_err(),
        ] {
            assert!(
                matches!(&error, InstallError::InvalidRecord(detail) if detail == expected),
                "{label}: {error:?}"
            );
        }
        assert_eq!(scratch.snapshot(), before, "{label}");
    }
    fs::write(scratch.record(), &pristine).unwrap();
    assert_eq!(
        unapply(&repository).unwrap().status(),
        InstallStatus::Uninstalled
    );
}

/// Replace one unique plan fragment and reseal the plan digest so only the
/// plan validators, not the closure check, can name the tampered record.
fn tamper_plan(record: &[u8], from: &str, to: &str) -> Vec<u8> {
    let text = std::str::from_utf8(record).unwrap();
    let plan_start = text.find("\"plan\":").unwrap() + "\"plan\":".len();
    let plan_end = text.find(",\"plan_digest\":\"").unwrap();
    let digest_start = plan_end + ",\"plan_digest\":\"".len();
    let digest_end = digest_start + text[digest_start..].find('"').unwrap();
    let plan = &text[plan_start..plan_end];
    assert_eq!(plan.matches(from).count(), 1, "{from}");
    let tampered = plan.replace(from, to);
    let digest = Digest::compute(Domain::InstallPlan, tampered.as_bytes());
    format!(
        "{}{tampered},\"plan_digest\":\"{digest}{}",
        &text[..plan_start],
        &text[digest_end..]
    )
    .into_bytes()
}

/// Publish a package whose Darwin executable is a minimal Mach-O image.
fn package_fixture(
    scratch: &Scratch,
    label: &str,
) -> (install_package_fixture::PackageFixture, PathBuf, Vec<u8>) {
    let executable = install_package_fixture::fabricate_macho(EXECUTABLE_UUID);
    let source = scratch.join("fixture-spice");
    fs::write(&source, &executable).unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
    let package = install_package_fixture::PackageFixture::create(scratch.join(label), &source);
    let darwin = package
        .root()
        .join("targets/aarch64-apple-darwin/bin/spice");
    (package, darwin, executable)
}

/// Rewrite the Darwin symbol artifact and reseal the payload around it, so
/// every refusal below is about the binding rather than a torn package.
fn forge_symbols(target: &Path, pristine: &[u8], edit: impl FnOnce(&mut MacSymbolArtifact)) {
    let artifact_path = target.join("symbols/spice.dSYM.json");
    let mut artifact: MacSymbolArtifact = wire::decode(pristine).unwrap();
    edit(&mut artifact);
    let bytes = wire::encode(&artifact).unwrap();
    fs::write(&artifact_path, &bytes).unwrap();
    let payload_path = target.join("payload.json");
    let mut payload: TargetPayload = wire::decode(&fs::read(&payload_path).unwrap()).unwrap();
    let metadata = payload.symbols.as_mut().unwrap();
    metadata.bytes = bytes.len() as u64;
    metadata.digest = Digest::compute(Domain::DistArtifact, &bytes);
    metadata.uuid = artifact.uuid.clone();
    let (bytes, digest) = (metadata.bytes, metadata.digest.clone());
    let entry = payload
        .inventory
        .iter_mut()
        .find(|entry| entry.path == "symbols/spice.dSYM.json")
        .unwrap();
    entry.bytes = bytes;
    entry.digest = digest;
    payload.inventory_digest = Digest::compute(
        Domain::DistInventory,
        &wire::encode(&payload.inventory).unwrap(),
    );
    fs::write(&payload_path, wire::encode(&payload).unwrap()).unwrap();
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn installed_spice_is_the_one_repository_local_executable() {
    let scratch = Scratch::new("appliance");
    let repository = Repository::discover(scratch.path()).unwrap();
    let source = Path::new(env!("CARGO_BIN_EXE_spice"));

    let planned = plan(&repository, Some(source)).unwrap();
    let receipt = apply(&repository, Some(source), &planned.digest().unwrap()).unwrap();
    let installed = installed_binary(&repository);

    assert_eq!(receipt.status(), InstallStatus::Complete);
    assert_eq!(
        capture_generation(&repository).unwrap().as_ref(),
        Some(receipt.digest())
    );
    assert_eq!(
        audit_appliance(&repository, &installed).unwrap().state(),
        ApplianceState::Current,
    );
    let help = run(&installed, &["--help"]);
    assert!(
        String::from_utf8(help)
            .unwrap()
            .contains("Repository-local coordination appliance")
    );
    assert_eq!(
        fs::metadata(&installed).unwrap().len(),
        fs::metadata(source).unwrap().len()
    );
    assert!(installed_skill(&repository).is_file());
    let wrapper = installed_spice_wrapper(&repository);
    assert_eq!(
        fs::read(&wrapper).unwrap(),
        steer::hook::STAGES[4].bytes().as_bytes()
    );
    assert_eq!(
        fs::metadata(&wrapper).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert_eq!(
        fs::read(installed_zdotdir(&repository).join(".zshenv")).unwrap(),
        steer::hook::STAGES[2].bytes().as_bytes()
    );
    assert_eq!(
        fs::read(installed_bash_env(&repository)).unwrap(),
        steer::hook::STAGES[3].bytes().as_bytes()
    );
    let receipt = fs::metadata(scratch.join(".git/.spice-rs/install.json")).unwrap();
    assert!(receipt.len() < MAX_COMPACT_RECORD_BYTES);
    assert_eq!(receipt.permissions().mode() & 0o777, 0o600);
    assert!(!scratch.join(".git/.spice/rust/install.json").exists());

    assert_eq!(
        unapply(&repository).unwrap().status(),
        InstallStatus::Uninstalled
    );
    assert!(!installed.exists());
}

#[test]
fn installed_skill_enforces_review_correction_and_pending_finding_policy() {
    let scratch = applied("review-correction-guidance");
    let repository = scratch.repository();
    let skill = fs::read_to_string(installed_skill(&repository)).unwrap();
    assert_eq!(skill, ::std::include_str!("../agent/SKILL.md"));
    let guidance_text = skill
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('`', "");

    for guidance in [
        "only when every change is tiny, deterministic, mechanical, low-risk",
        "separate commit while holding the active review claim",
        "validate the integrated result, then record a clean verdict without --then or --followup",
        "behavior-changing, schema-or-authority, cross-ownership, broad, uncertain, or independently reviewable change remains a finding review",
        "add flow=todo when the work is mechanical and needs no independent review",
        "use flow=todo,review for material correction work",
        "finding verdict creates or links those corrections as dependencies of the reviewed task",
        "releases the review claim, and leaves the original task pending and unassigned in review",
        "a later reviewer must claim the original task and record a clean verdict before it completes",
        "gate::pre-commit-configuration-untrusted and this lane owns an active task claim",
        "spice task require HANDLE --trust-grant DIGEST",
        "repeating its existing --journal-schema SCHEMA operand when present",
        "This atomically ices the same claim; retain it and stop that execution path",
        "With no active claim, report the operator action and change no task",
    ] {
        assert!(
            guidance_text.contains(guidance),
            "installed skill omitted {guidance:?}"
        );
    }
    assert!(!guidance_text.contains("when that is more proportionate"));
}

#[test]
fn installed_skill_makes_task_capture_conditional_without_weakening_guidance() {
    let scratch = applied("conditional-guidance-capture");
    let repository = scratch.repository();
    let skill = fs::read_to_string(installed_skill(&repository)).unwrap();
    let guidance = skill.split_whitespace().collect::<Vec<_>>().join(" ");

    for required in [
        "Consider and answer every pending key",
        "Acknowledgment or adjustment of current work may satisfy guidance",
        "create a task only when the operator explicitly requests one or its substance warrants a separately valuable deliverable",
        "Resident-authored taste reminders, command-generated task hints, automatic task amendments, review follow-up notices, and other synthesized guidance do not themselves require or authorize another task",
    ] {
        assert!(
            guidance.contains(required),
            "installed skill omitted conditional guidance {required:?}"
        );
    }
    assert!(
        !guidance.contains("Capture pending steering as"),
        "installed skill still makes every steering key create work"
    );
}

#[test]
fn installed_skill_bounds_the_boarded_red_exception() {
    let scratch = applied("boarded-red-guidance");
    let repository = scratch.repository();
    let skill = fs::read_to_string(installed_skill(&repository)).unwrap();
    let guidance = skill.split_whitespace().collect::<Vec<_>>().join(" ");

    for required in [
        "The normal pre-commit gate is the default constitution",
        "git commit --no-verify` only under that current authorization",
        "exact evidence as ready critical work",
        "synthesized merge remains hook-free",
        "declared cheap integrated-tree admission still applies",
    ] {
        assert!(
            guidance.contains(required),
            "installed skill omitted {required:?}"
        );
    }
}

#[test]
fn linked_worktrees_share_one_common_install_and_receipt() {
    let scratch = Scratch::new("linked");
    let common = scratch.join(".git");
    let linked = scratch.join("linked");
    git_repository::link(scratch.path(), &linked, "linked");
    git_repository::seed_canvas_stage(&linked);
    let first_repository = Repository::discover(scratch.path()).unwrap();
    let linked_repository = Repository::discover(&linked).unwrap();
    let source = Path::new(env!("CARGO_BIN_EXE_spice"));

    let planned = plan(&first_repository, Some(source)).unwrap();
    let installed = apply(&first_repository, Some(source), &planned.digest().unwrap()).unwrap();
    let linked_plan = plan(&linked_repository, Some(source)).unwrap();
    let linked_install = apply(
        &linked_repository,
        Some(source),
        &linked_plan.digest().unwrap(),
    )
    .unwrap();

    assert_eq!(installed.status(), InstallStatus::Complete);
    assert_eq!(linked_install, installed);
    assert_eq!(linked_plan, planned);
    assert_eq!(
        installed_binary(&first_repository),
        installed_binary(&linked_repository)
    );
    assert_eq!(
        installed_git_hooks(&first_repository),
        installed_git_hooks(&linked_repository)
    );
    assert!(installed_binary(&linked_repository).is_file());
    assert!(installed_skill(&linked_repository).is_file());
    assert!(installed_spice_wrapper(&linked_repository).is_file());
    assert!(installed_git_hooks(&linked_repository).is_dir());
    let wrapper = installed_spice_wrapper(&linked_repository);
    let output = Command::new("/bin/sh")
        .args(["-c", "spice --help"])
        .current_dir(&linked)
        .env(
            "PATH",
            wrapper.parent().expect("installed wrapper has a parent"),
        )
        .env(
            steer::hook::EXECUTABLE,
            installed_binary(&linked_repository),
        )
        .output()
        .expect("ambient Spice resolves through the linked-worktree wrapper");
    let diagnostic = format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(output.status.success(), "{diagnostic}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("Repository-local coordination appliance"),
        "{diagnostic}"
    );
    assert!(common.join(".spice-rs/install.json").is_file());
    let app = common.join(".spice-install/app");
    let generation = fs::read_to_string(app.join("current"))
        .unwrap()
        .trim()
        .to_owned();
    for file in wire::manifest::CANVAS_APP_FILES {
        assert!(
            app.join("generations")
                .join(&generation)
                .join(file)
                .is_file()
        );
    }
    let record: serde_json::Value =
        serde_json::from_slice(&fs::read(common.join(".spice-rs/install.json")).unwrap()).unwrap();
    assert!(record["repository"].get("worktree").is_none());
    assert!(record["plan"]["repository"].get("worktree").is_none());
    assert!(!scratch.join(".spice-install").exists());
    assert!(!linked.join(".spice-install").exists());

    assert_eq!(
        unapply(&linked_repository).unwrap().status(),
        InstallStatus::Uninstalled
    );
    assert!(!installed_binary(&first_repository).exists());
    assert!(!app.exists());
    assert!(common.join(".spice-rs/install.json").is_file());
}

#[test]
fn canvas_refresh_carries_immutable_generations_and_unapplies_them_together() {
    let scratch = Scratch::new("canvas-refresh");
    let repository = scratch.repository();
    let binary = Path::new(env!("CARGO_BIN_EXE_spice"));
    let first = plan(&repository, Some(binary)).unwrap();
    apply(&repository, Some(binary), &first.digest().unwrap()).unwrap();

    let stage = scratch.join("dist/stage-app");
    let first_generation = fs::read_to_string(stage.join("current")).unwrap();
    fs::write(
        stage
            .join("generations")
            .join(first_generation.trim())
            .join("style.css"),
        b"same generation, different bytes\n",
    )
    .unwrap();
    assert_eq!(
        audit_appliance(&repository, binary).unwrap().state(),
        ApplianceState::AnotherBuild,
    );
    let same_generation = plan(&repository, Some(binary)).unwrap();
    assert_eq!(same_generation.app_generation(), first.app_generation());
    apply(
        &repository,
        Some(binary),
        &same_generation.digest().unwrap(),
    )
    .unwrap();

    git_repository::seed_canvas_stage_as(scratch.path(), b"refreshed fixture app");
    assert_eq!(
        audit_appliance(&repository, binary).unwrap().state(),
        ApplianceState::AnotherBuild,
    );
    let refreshed = plan(&repository, Some(binary)).unwrap();
    assert_ne!(refreshed.digest().unwrap(), first.digest().unwrap());
    apply(&repository, Some(binary), &refreshed.digest().unwrap()).unwrap();

    let app = repository.common_dir().join(".spice-install/app");
    let generations = fs::read_dir(app.join("generations"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(generations.len(), 2);
    let current = fs::read_to_string(app.join("current")).unwrap();
    assert_eq!(current.trim(), refreshed.app_generation().unwrap().as_str());
    assert_eq!(
        unapply(&repository).unwrap().status(),
        InstallStatus::Uninstalled,
    );
    assert!(!app.join("current").exists());
    for generation in generations {
        for file in wire::manifest::CANVAS_APP_FILES {
            assert!(!generation.join(file).exists());
        }
    }
}

#[test]
fn stable_and_swapped_parent_symlinks_bind_one_physical_destination() {
    let stable = Scratch::new("stable-symlink");
    let repository = stable.repository();
    let target = repository.common_dir().join("appliance-assets");
    fs::create_dir(&target).unwrap();
    symlink(&target, repository.common_dir().join(".spice-install")).unwrap();
    let planned = plan(&repository, None).unwrap();
    apply(&repository, None, &planned.digest().unwrap()).unwrap();
    assert!(target.join("agents/skills/spice/SKILL.md").is_file());
    unapply(&repository).unwrap();
    assert!(!target.join("agents/skills/spice/SKILL.md").exists());

    let swapped = Scratch::new("swapped-symlink");
    let repository = swapped.repository();
    let first = repository.common_dir().join("appliance-assets-a");
    let second = repository.common_dir().join("appliance-assets-b");
    fs::create_dir(&first).unwrap();
    fs::create_dir(&second).unwrap();
    symlink(&first, repository.common_dir().join(".spice-install")).unwrap();
    let stale = plan(&repository, None).unwrap().digest().unwrap();
    fs::remove_file(repository.common_dir().join(".spice-install")).unwrap();
    symlink(&second, repository.common_dir().join(".spice-install")).unwrap();
    let before = swapped.snapshot();
    let error = apply(&repository, None, &stale).unwrap_err();
    assert!(matches!(error, InstallError::DigestMismatch { .. }));
    assert_eq!(swapped.snapshot(), before);
    assert!(!swapped.record().exists());
}

#[test]
fn parent_creation_replacement_and_same_byte_destination_drift_are_inert() {
    let created = Scratch::new("created-parent");
    let repository = created.repository();
    let stale = plan(&repository, None).unwrap().digest().unwrap();
    fs::create_dir(repository.common_dir().join(".spice-install")).unwrap();
    assert_stale_apply_is_inert(&created, &repository, &stale);

    let replaced_parent = Scratch::new("replaced-parent");
    let repository = replaced_parent.repository();
    let parent = repository
        .common_dir()
        .join(".spice-install/agents/skills/spice");
    fs::create_dir_all(&parent).unwrap();
    let stale = plan(&repository, None).unwrap().digest().unwrap();
    fs::rename(
        &parent,
        repository
            .common_dir()
            .join(".spice-install/agents/skills/prior-parent"),
    )
    .unwrap();
    fs::create_dir(&parent).unwrap();
    assert_stale_apply_is_inert(&replaced_parent, &repository, &stale);

    let replaced_file = Scratch::new("replaced-file");
    let repository = replaced_file.repository();
    let skill = repository
        .common_dir()
        .join(".spice-install/agents/skills/spice/SKILL.md");
    fs::create_dir_all(skill.parent().unwrap()).unwrap();
    fs::write(&skill, b"operator skill\n").unwrap();
    let stale = plan(&repository, None).unwrap().digest().unwrap();
    let replacement = replaced_file.join("replacement-skill");
    fs::write(&replacement, fs::read(&skill).unwrap()).unwrap();
    fs::rename(replacement, skill).unwrap();
    assert_stale_apply_is_inert(&replaced_file, &repository, &stale);
}

#[test]
fn copied_corrupt_and_noncanonical_records_are_inert() {
    let source = applied("record-source");
    let copied = Scratch::new("record-copied");
    fs::create_dir_all(copied.record().parent().unwrap()).unwrap();
    fs::copy(source.record(), copied.record()).unwrap();
    assert_bad_record_is_inert(&copied);

    let corrupt = applied("record-corrupt");
    fs::write(corrupt.record(), b"{not-json").unwrap();
    assert_bad_record_is_inert(&corrupt);

    let noncanonical = applied("record-noncanonical");
    let mut bytes = fs::read(noncanonical.record()).unwrap();
    bytes.push(b'\n');
    fs::write(noncanonical.record(), bytes).unwrap();
    assert_bad_record_is_inert(&noncanonical);
}

#[test]
fn arbitrary_duplicate_reordered_and_mismatched_records_are_inert() {
    tamper_record("record-out-root", |record| {
        record["plan"]["operations"][0]["destination"] = "../outside".into();
    });
    tamper_record("record-duplicate", |record| {
        record["destinations"][1] = record["destinations"][0].clone();
    });
    tamper_record("record-reordered", |record| {
        record["plan"]["operations"]
            .as_array_mut()
            .unwrap()
            .swap(0, 1);
    });
    tamper_record("record-plan-mismatch", |record| {
        record["plan_digest"] = record["plan"]["operations"][0]["generated"]["digest"].clone();
    });
    tamper_record("record-reversal", |record| {
        record["reversals"][0] = record["reversals"][1].clone();
    });
    tamper_record("record-old-version", |record| {
        record["version"] = 1.into();
    });
    tamper_record("record-expanded-rollback", |record| {
        record["plan"]["rollback_record"] = serde_json::json!({});
    });
}

#[test]
fn same_byte_installed_file_replacement_unapplies_while_parent_replacement_refuses_inertly() {
    let replaced_file = applied("installed-file-generation");
    let repository = replaced_file.repository();
    let entry = repository
        .common_dir()
        .join(".spice-install/hooks/entry.sh");
    let replacement = replaced_file.join("replacement-entry");
    fs::write(&replacement, fs::read(&entry).unwrap()).unwrap();
    fs::set_permissions(&replacement, fs::Permissions::from_mode(0o755)).unwrap();
    fs::rename(replacement, entry).unwrap();
    assert_eq!(
        unapply(&repository).unwrap().status(),
        InstallStatus::Uninstalled
    );
    assert!(!replaced_file.join(".spice/hooks/entry.sh").exists());

    let replaced_parent = applied("installed-parent-generation");
    let repository = replaced_parent.repository();
    let hooks = repository.common_dir().join(".spice-install/hooks");
    let prior = repository.common_dir().join(".spice-install/prior-hooks");
    fs::rename(&hooks, &prior).unwrap();
    fs::create_dir(&hooks).unwrap();
    fs::hard_link(prior.join("entry.sh"), hooks.join("entry.sh")).unwrap();
    fs::create_dir(hooks.join("static")).unwrap();
    fs::hard_link(prior.join("static/stage.sh"), hooks.join("static/stage.sh")).unwrap();
    let before = replaced_parent.snapshot();
    assert!(matches!(
        unapply(&repository),
        Err(InstallError::ParentChanged(_) | InstallError::Drift { .. })
    ));
    assert_eq!(replaced_parent.snapshot(), before);
}

fn assert_stale_apply_is_inert(scratch: &Scratch, repository: &Repository, stale: &wire::Digest) {
    let before = scratch.snapshot();
    let error = apply(repository, None, stale).unwrap_err();
    assert!(matches!(error, InstallError::DigestMismatch { .. }));
    assert_eq!(scratch.snapshot(), before);
    assert!(!scratch.record().exists());
}

fn applied(label: &str) -> Scratch {
    let scratch = Scratch::new(label);
    let repository = scratch.repository();
    let planned = plan(&repository, None).unwrap();
    apply(&repository, None, &planned.digest().unwrap()).unwrap();
    scratch
}

fn tamper_record(label: &str, edit: impl FnOnce(&mut serde_json::Value)) {
    let scratch = applied(label);
    let mut record: serde_json::Value =
        serde_json::from_slice(&fs::read(scratch.record()).unwrap()).unwrap();
    edit(&mut record);
    fs::write(scratch.record(), serde_json::to_vec(&record).unwrap()).unwrap();
    assert_bad_record_is_inert(&scratch);
}

fn assert_bad_record_is_inert(scratch: &Scratch) {
    let repository = scratch.repository();
    let before = scratch.snapshot();
    assert!(unapply(&repository).is_err());
    assert_eq!(scratch.snapshot(), before);
}

fn run(path: &Path, arguments: &[&str]) -> Vec<u8> {
    let output = Command::new(path).args(arguments).output().unwrap();
    assert!(output.status.success(), "{} failed", path.display());
    assert!(output.stderr.is_empty(), "{} wrote stderr", path.display());
    output.stdout
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        for _ in 0..100 {
            let sequence = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "spice-install-{label}-{}-{sequence}",
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
        panic!("no unique install integration fixture path was available");
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn repository(&self) -> Repository {
        Repository::discover(self.path()).unwrap()
    }

    fn record(&self) -> PathBuf {
        self.join(".git/.spice-rs/install.json")
    }

    fn snapshot(&self) -> Snapshot {
        let mut entries = Vec::new();
        snapshot_walk(self.path(), self.path(), &mut entries);
        Snapshot(entries)
    }

    fn join(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.0.join(relative)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Snapshot(Vec<(PathBuf, u32, Node)>);

#[derive(Clone, Debug, Eq, PartialEq)]
enum Node {
    Directory,
    File(Vec<u8>),
    Symlink(PathBuf),
}

fn snapshot_walk(root: &Path, path: &Path, entries: &mut Vec<(PathBuf, u32, Node)>) {
    let mut children = fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    children.sort();
    for child in children {
        let metadata = fs::symlink_metadata(&child).unwrap();
        let relative = child.strip_prefix(root).unwrap().to_path_buf();
        let mode = metadata.permissions().mode() & 0o777;
        if metadata.file_type().is_symlink() {
            entries.push((
                relative,
                mode,
                Node::Symlink(fs::read_link(&child).unwrap()),
            ));
        } else if metadata.is_dir() {
            entries.push((relative, mode, Node::Directory));
            snapshot_walk(root, &child, entries);
        } else {
            entries.push((relative, mode, Node::File(fs::read(&child).unwrap())));
        }
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
