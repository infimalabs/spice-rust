//! Mutation, reversal, and deterministic race proofs for the install CLI.

use super::*;

#[derive(Clone, Copy)]
enum InstallRaceWinner {
    Apply,
    Unapply,
}

struct RaceEnding {
    receipt: InstallReceipt,
    state: InstallSnapshot,
    loser: Spoken,
}

#[test]
fn competing_install_directions_serialize_when_apply_wins() {
    competing_install_directions_serialize(InstallRaceWinner::Apply);
}

#[test]
fn competing_install_directions_serialize_when_unapply_wins() {
    competing_install_directions_serialize(InstallRaceWinner::Unapply);
}

fn competing_install_directions_serialize(winner: InstallRaceWinner) {
    let label = match winner {
        InstallRaceWinner::Apply => "same-record-apply-wins",
        InstallRaceWinner::Unapply => "same-record-unapply-wins",
    };
    let fixture = Fixture::new(label);
    fixture.assert_small_owned_executable();
    let pristine_config = fs::read(fixture.root.join(".git/config"))
        .expect("read pristine fixture Git configuration");

    let preview_a: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    let receipt_a: InstallReceipt = reply_body(fixture.invoke(&[
        "--json",
        "install",
        &format!("--apply={}", preview_a.digest()),
    ]));
    assert_eq!(receipt_a.status(), InstallStatus::Complete);
    assert_eq!(receipt_a.digest(), preview_a.digest());
    assert_plan_generated(&fixture, preview_a.plan());

    fixture.rewrite_executable(b"fixture install executable version B\n");
    let before_preview = InstallSnapshot::capture(&fixture, preview_a.plan());
    let preview_b: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    assert_ne!(preview_b.digest(), preview_a.digest());
    assert_eq!(
        InstallSnapshot::capture(&fixture, preview_a.plan()),
        before_preview,
        "version-B preview mutated the complete version-A state",
    );

    let common = InstallSnapshot::capture(&fixture, preview_b.plan());
    assert_eq!(
        common, before_preview,
        "version-B preview changed the version-A receipt or ownership surface",
    );
    let mut apply = fixture.spawn_paused(
        &[
            "--json",
            "install",
            &format!("--apply={}", preview_b.digest()),
        ],
        "publish",
        fixture.record(),
        Release::Withheld,
    );
    let mut unapply = fixture.spawn_paused(
        &["--json", "install", "--unapply"],
        "publish",
        fixture.record(),
        Release::Withheld,
    );
    assert_eq!(
        InstallSnapshot::capture(&fixture, preview_b.plan()),
        common,
        "dual-ready directions changed the shared receipt or an owned target",
    );
    assert_plan_generated(&fixture, preview_a.plan());

    let ending = match winner {
        InstallRaceWinner::Apply => {
            apply.release();
            let receipt: InstallReceipt = reply_body(apply.collect());
            let state = InstallSnapshot::capture(&fixture, preview_b.plan());
            unapply.release();
            RaceEnding {
                receipt,
                state,
                loser: unapply.collect(),
            }
        }
        InstallRaceWinner::Unapply => {
            unapply.release();
            let receipt: InstallReceipt = reply_body(unapply.collect());
            let state = InstallSnapshot::capture(&fixture, preview_b.plan());
            apply.release();
            RaceEnding {
                receipt,
                state,
                loser: apply.collect(),
            }
        }
    };

    assert_race_ending(
        &fixture,
        winner,
        &pristine_config,
        &preview_a,
        &preview_b,
        &common,
        &ending,
    );

    let before_final_preview = InstallSnapshot::capture(&fixture, preview_b.plan());
    let preview_after: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    assert_eq!(
        InstallSnapshot::capture(&fixture, preview_b.plan()),
        before_final_preview,
        "post-race preview changed receipt or target generations",
    );
    let repeated: InstallReceipt = match winner {
        InstallRaceWinner::Apply => reply_body(fixture.invoke(&[
            "--json",
            "install",
            &format!("--apply={}", preview_after.digest()),
        ])),
        InstallRaceWinner::Unapply => {
            reply_body(fixture.invoke(&["--json", "install", "--unapply"]))
        }
    };
    assert_eq!(repeated, ending.receipt);
    assert_eq!(
        InstallSnapshot::capture(&fixture, preview_b.plan()),
        before_final_preview,
        "idempotent winning direction changed receipt or target generations",
    );
}

fn assert_race_ending(
    fixture: &Fixture,
    winner: InstallRaceWinner,
    pristine_config: &[u8],
    preview_a: &InstallPreview,
    preview_b: &InstallPreview,
    common: &InstallSnapshot,
    ending: &RaceEnding,
) {
    assert_ne!(
        common
            .record
            .as_ref()
            .map(|record| (record.device, record.inode)),
        ending
            .state
            .record
            .as_ref()
            .map(|record| (record.device, record.inode)),
        "the winning direction did not advance the shared receipt generation",
    );
    assert_eq!(
        ending.loser.code,
        Some(2),
        "{}",
        spoken_diagnostic(&ending.loser)
    );
    assert!(
        ending.loser.out.is_empty(),
        "{}",
        spoken_diagnostic(&ending.loser)
    );
    assert_refusal(&ending.loser.err, "install::generation-changed");
    assert_eq!(
        InstallSnapshot::capture(fixture, preview_b.plan()),
        ending.state,
        "the stale direction changed the winner's receipt or an owned target",
    );

    match winner {
        InstallRaceWinner::Apply => {
            assert_eq!(ending.receipt.status(), InstallStatus::Complete);
            assert_eq!(ending.receipt.digest(), preview_b.digest());
            assert_plan_generated(fixture, preview_b.plan());
            assert_ne!(ending.state.git_config.bytes, pristine_config);
            assert_eq!(
                git_hook_path(&fixture.root).as_deref(),
                preview_b
                    .plan()
                    .hook_activation()
                    .map(|activation| activation.hooks_path()),
            );
        }
        InstallRaceWinner::Unapply => {
            assert_eq!(ending.receipt.status(), InstallStatus::Uninstalled);
            assert_eq!(ending.receipt.digest(), preview_a.digest());
            assert_plan_restored(&ending.state, preview_a.plan());
            assert_eq!(ending.state.git_config.bytes, pristine_config);
            assert!(git_hook_path(&fixture.root).is_none());
        }
    }
}

fn assert_plan_generated(fixture: &Fixture, plan: &InstallPlanSummary) {
    let snapshot = InstallSnapshot::capture(fixture, plan);
    for operation in plan.operations() {
        let image = target_snapshot(&snapshot, operation.path())
            .as_ref()
            .unwrap_or_else(|| panic!("{} was not generated", operation.path()));
        assert_eq!(image.mode, operation.generated().mode());
        assert_eq!(
            wire::Digest::compute(wire::Domain::InstallArtifact, &image.bytes),
            *operation.generated().digest(),
            "{} does not match its planned bytes",
            operation.path(),
        );
    }
    for retirement in plan.retirements() {
        assert!(
            target_snapshot(&snapshot, retirement.path()).is_none(),
            "retired target {} survived apply",
            retirement.path(),
        );
    }
}

fn assert_plan_restored(snapshot: &InstallSnapshot, plan: &InstallPlanSummary) {
    for operation in plan.operations() {
        assert_restored_image(snapshot, operation.path(), operation.previous());
    }
    for retirement in plan.retirements() {
        assert_restored_image(snapshot, retirement.path(), retirement.previous());
    }
}

fn assert_restored_image(
    snapshot: &InstallSnapshot,
    path: &str,
    previous: Option<&spice::install::GeneratedImage>,
) {
    let observed = target_snapshot(snapshot, path);
    match previous {
        Some(previous) => {
            let image = observed
                .as_ref()
                .unwrap_or_else(|| panic!("{path} was not restored"));
            assert_eq!(image.mode, previous.mode());
            assert_eq!(
                wire::Digest::compute(wire::Domain::InstallArtifact, &image.bytes),
                *previous.digest(),
                "{path} does not match its rollback bytes",
            );
        }
        None => assert!(
            observed.is_none(),
            "introduced target {path} survived unapply"
        ),
    }
}

fn target_snapshot<'a>(snapshot: &'a InstallSnapshot, path: &str) -> &'a Option<FileSnapshot> {
    &snapshot
        .targets
        .iter()
        .find(|(candidate, _)| candidate == path)
        .unwrap_or_else(|| panic!("snapshot omitted {path}"))
        .1
}

fn spoken_diagnostic(spoken: &Spoken) -> String {
    format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&spoken.out),
        String::from_utf8_lossy(&spoken.err),
    )
}

#[test]
fn unapply_restores_exact_prior_images_and_keeps_the_namespaced_receipt() {
    let fixture = Fixture::new("unapply");
    let preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    let operation_count = preview.plan().operation_count();
    let digest = preview.digest();
    let applied = fixture.invoke(&["install", &format!("--apply={digest}")]);
    applied.assert_success();

    let unapplied: InstallReceipt = reply_body(fixture.invoke(&["--json", "install", "--unapply"]));
    assert_eq!(unapplied.status(), InstallStatus::Uninstalled);
    assert_eq!(unapplied.completed(), operation_count);
    assert!(!fixture.installed_binary().exists());
    assert!(fixture.record().is_file());
    assert_eq!(
        fs::read(fixture.root.join(".git/info/exclude")).unwrap(),
        fixture.exclude_bytes
    );
    fixture.assert_state_unchanged();
}

#[test]
fn same_image_atomic_rewrite_still_plans_and_unapplies() {
    let fixture = Fixture::new("same-image-rewrite");
    let preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    let digest = preview.digest().clone();
    fixture
        .invoke(&["install", &format!("--apply={digest}")])
        .assert_success();
    let entry = fixture.root.join(".git/.spice-install/hooks/entry.sh");
    let replacement = fixture.root.join("replacement-entry");
    fs::write(&replacement, fs::read(&entry).unwrap()).expect("write replacement entry hook");
    fs::set_permissions(&replacement, fs::Permissions::from_mode(0o755))
        .expect("set replacement entry hook mode");
    fs::rename(replacement, &entry).expect("atomically rewrite installed entry hook");

    let replanned: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    assert_eq!(replanned.digest(), &digest);
    let unapply = fixture.invoke(&["--json", "install", "--unapply"]);
    let unapplied: InstallReceipt = reply_body(unapply);

    assert_eq!(unapplied.status(), InstallStatus::Uninstalled);
    assert!(!entry.exists());
    fixture.assert_state_unchanged();
}

#[test]
fn drift_refuses_reversal_without_overwriting_the_changed_target() {
    let fixture = Fixture::new("drift");
    let preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    let digest = preview.digest();
    fixture
        .invoke(&["install", &format!("--apply={digest}")])
        .assert_success();
    let entry = fixture.root.join(".git/.spice-install/hooks/entry.sh");
    fs::write(&entry, b"operator drift\n").expect("drift installed entry hook");

    let refused = fixture.invoke(&["--json", "install", "--unapply"]);
    assert_eq!(
        refused.code,
        Some(2),
        "unexpected drift outcome: stdout={} stderr={}",
        String::from_utf8_lossy(&refused.out),
        String::from_utf8_lossy(&refused.err),
    );
    assert!(refused.out.is_empty());
    assert_refusal(&refused.err, "install::drift");
    assert_eq!(fs::read(entry).unwrap(), b"operator drift\n");
    assert!(fixture.record().is_file());
    fixture.assert_state_unchanged();
}

#[test]
fn apply_destination_replacement_at_publication_preserves_both_images() {
    let fixture = Fixture::new("apply-target-race");
    let destination = fixture
        .root
        .join(".git/.spice-install/git-hooks/pre-commit");
    let parked = fixture
        .root
        .join(".git/.spice-install/git-hooks/pre-commit.parked");
    let prior = b"operator-owned pre-commit\n";
    fs::create_dir_all(destination.parent().unwrap()).expect("create prior hook parent");
    fs::write(&destination, prior).expect("write prior hook");
    let preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    let digest = preview.digest();
    let racer = b"racer-owned pre-commit\n";
    let paused = fixture.spawn_paused(
        &["--json", "install", &format!("--apply={digest}")],
        "publish",
        destination.clone(),
        Release::Withheld,
    );

    fs::rename(&destination, &parked).expect("park planned hook destination");
    fs::write(&destination, racer).expect("replace planned hook destination");
    let refused = paused.resume();

    assert_eq!(refused.code, Some(2));
    assert!(refused.out.is_empty());
    assert_refusal(&refused.err, "install::parent-changed");
    assert_eq!(fs::read(&destination).unwrap(), racer);
    assert_eq!(fs::read(&parked).unwrap(), prior);
    fixture.assert_state_unchanged();
}

/// A release already in place is observed before the receiver is armed.
///
/// `release/continue` is written before the install is spawned. The child sees
/// that authoritative marker, connects to the harness's readiness receiver,
/// and never binds a release receiver. The only thing that can produce this
/// ending is the release test before receiver setup.
///
/// A green run here is a run that reached that wait: the readiness message the
/// harness accepts before collecting is sent only by an install that reached
/// the configured pause.
///
/// The fixture adds no horizon cost by the same construction that makes it a
/// guard: the release is already there, so the wait ends on its first test
/// rather than on its bound. Remove that test and this install cannot announce
/// readiness because no harness can release a receiver that was never needed.
#[test]
fn apply_pause_released_before_receiver_setup_is_observed() {
    let fixture = Fixture::new("apply-prelanded-pause");
    let preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    let operation_count = preview.plan().operation_count();
    let digest = preview.digest();
    let destination = fixture
        .root
        .join(".git/.spice-install/git-hooks/pre-commit");
    let paused = fixture.spawn_paused(
        &["--json", "install", &format!("--apply={digest}")],
        "publish",
        destination.clone(),
        Release::Landed,
    );

    let applied: InstallReceipt = reply_body(paused.collect());

    assert_eq!(applied.status(), InstallStatus::Complete);
    assert_eq!(applied.completed(), operation_count);
    assert_eq!(
        fs::read(fixture.installed_binary()).unwrap(),
        fs::read(&fixture.executable).unwrap()
    );
    assert!(fixture.record().is_file());
    fixture.assert_state_unchanged();
}

#[test]
fn apply_record_parent_replacement_at_publication_preserves_both_trees() {
    let fixture = Fixture::new("apply-record-race");
    let preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    let digest = preview.digest();
    let parent = fixture.root.join(".git/.spice-rs");
    let parked = fixture.root.join(".git/.spice-rs.parked");
    let outside = fixture.root.join("outside-record");
    fs::create_dir(&outside).expect("create replacement record tree");
    let paused = fixture.spawn_paused(
        &["--json", "install", &format!("--apply={digest}")],
        "publish",
        fixture.record(),
        Release::Withheld,
    );

    fs::rename(&parent, &parked).expect("park admitted record parent");
    symlink(&outside, &parent).expect("replace admitted record parent");
    let refused = paused.resume();

    assert_eq!(
        refused.code,
        Some(2),
        "unexpected apply record-race outcome: stdout={} stderr={}",
        String::from_utf8_lossy(&refused.out),
        String::from_utf8_lossy(&refused.err),
    );
    assert!(refused.out.is_empty());
    assert_refusal(&refused.err, "install::parent-changed");
    assert!(fs::read_dir(&outside).unwrap().next().is_none());
    fixture.assert_state_unchanged();
}

#[test]
fn unapply_destination_replacement_at_removal_preserves_both_images() {
    let fixture = Fixture::new("unapply-target-race");
    let preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    let digest = preview.digest();
    fixture
        .invoke(&["--json", "install", &format!("--apply={digest}")])
        .assert_success();
    let destination = fixture.installed_binary();
    let parked = destination.with_extension("parked");
    let installed = fs::read(&destination).unwrap();
    let racer = b"racer-owned binary\n";
    let paused = fixture.spawn_paused(
        &["--json", "install", "--unapply"],
        "remove",
        destination.clone(),
        Release::Withheld,
    );

    fs::rename(&destination, &parked).expect("park installed binary");
    fs::write(&destination, racer).expect("replace installed binary");
    let refused = paused.resume();

    assert_eq!(refused.code, Some(2));
    assert!(refused.out.is_empty());
    assert_refusal(&refused.err, "install::parent-changed");
    assert_eq!(fs::read(&destination).unwrap(), racer);
    assert_eq!(fs::read(&parked).unwrap(), installed);
    fixture.assert_state_unchanged();
}

#[test]
fn unapply_destination_replacement_at_restore_preserves_both_images() {
    let fixture = Fixture::new("unapply-restore-race");
    let destination = fixture
        .root
        .join(".git/.spice-install/git-hooks/pre-commit");
    fs::create_dir_all(destination.parent().unwrap()).expect("create prior hook parent");
    fs::write(&destination, b"operator-owned pre-commit\n").expect("write prior hook");
    let preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    let digest = preview.digest();
    fixture
        .invoke(&["--json", "install", &format!("--apply={digest}")])
        .assert_success();
    let parked = fixture
        .root
        .join(".git/.spice-install/git-hooks/pre-commit.parked");
    let installed = fs::read(&destination).unwrap();
    let racer = b"racer-owned pre-commit\n";
    let paused = fixture.spawn_paused(
        &["--json", "install", "--unapply"],
        "publish",
        destination.clone(),
        Release::Withheld,
    );

    fs::rename(&destination, &parked).expect("park installed hook");
    fs::write(&destination, racer).expect("replace installed hook");
    let refused = paused.resume();

    assert_eq!(refused.code, Some(2));
    assert!(refused.out.is_empty());
    assert_refusal(&refused.err, "install::parent-changed");
    assert_eq!(fs::read(&destination).unwrap(), racer);
    assert_eq!(fs::read(&parked).unwrap(), installed);
    fixture.assert_state_unchanged();
}

#[test]
fn unapply_directory_replacement_at_cleanup_preserves_both_trees() {
    let fixture = Fixture::new("unapply-cleanup-race");
    let preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    let digest = preview.digest();
    fixture
        .invoke(&["--json", "install", &format!("--apply={digest}")])
        .assert_success();
    let destination = fixture.root.join(".git/.spice-install/bin");
    let parked = fixture.root.join(".git/.spice-install/bin.parked");
    let paused = fixture.spawn_paused(
        &["--json", "install", "--unapply"],
        "cleanup",
        destination.clone(),
        Release::Withheld,
    );

    fs::rename(&destination, &parked).expect("park introduced binary directory");
    fs::create_dir(&destination).expect("replace introduced binary directory");
    fs::write(destination.join("racer"), b"outside")
        .expect("populate replacement binary directory");
    let refused = paused.resume();

    assert_eq!(refused.code, Some(2));
    assert!(refused.out.is_empty());
    assert_refusal(&refused.err, "install::parent-changed");
    assert!(fs::read_dir(&parked).unwrap().next().is_none());
    assert_eq!(fs::read(destination.join("racer")).unwrap(), b"outside");
    fixture.assert_state_unchanged();
}

#[test]
fn unapply_record_parent_replacement_at_publication_preserves_both_trees() {
    let fixture = Fixture::new("unapply-record-race");
    let preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    let digest = preview.digest();
    fixture
        .invoke(&["--json", "install", &format!("--apply={digest}")])
        .assert_success();
    let record = fixture.record();
    let record_before = fs::read(&record).unwrap();
    let binary_before = fs::read(fixture.installed_binary()).unwrap();
    let parent = fixture.root.join(".git/.spice-rs");
    let parked = fixture.root.join(".git/.spice-rs.parked");
    let outside = fixture.root.join("outside-record");
    fs::create_dir(&outside).expect("create replacement record tree");
    let paused = fixture.spawn_paused(
        &["--json", "install", "--unapply"],
        "publish",
        record.clone(),
        Release::Withheld,
    );

    fs::rename(&parent, &parked).expect("park admitted record parent");
    symlink(&outside, &parent).expect("replace admitted record parent");
    let refused = paused.resume();

    assert_eq!(
        refused.code,
        Some(2),
        "unexpected unapply record-race outcome: stdout={} stderr={}",
        String::from_utf8_lossy(&refused.out),
        String::from_utf8_lossy(&refused.err),
    );
    assert!(refused.out.is_empty());
    assert_refusal(&refused.err, "install::parent-changed");
    assert_eq!(
        fs::read(parked.join("install.json")).unwrap(),
        record_before
    );
    assert!(fs::read_dir(&outside).unwrap().next().is_none());
    assert_eq!(fs::read(fixture.installed_binary()).unwrap(), binary_before);
    fixture.assert_state_unchanged();
}

pub(super) fn successful_document(spoken: Spoken) -> Value {
    spoken.assert_success();
    serde_json::from_slice(&spoken.out).expect("successful output is JSON")
}

pub(super) fn emitted_digest(document: &Value) -> &str {
    document["digest"]
        .as_str()
        .expect("install preview emits a digest token")
}
