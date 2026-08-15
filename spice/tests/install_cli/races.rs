use super::super::*;

#[test]
fn preview_apply_and_unapply_overlap_with_small_fixture_owned_sources() {
    let preview_fixture = Fixture::new("overlap-preview");
    preview_fixture.assert_small_owned_executable();
    let apply_fixture = Fixture::new("overlap-apply");
    apply_fixture.assert_small_owned_executable();
    let apply_plan: InstallPreview = reply_body(apply_fixture.invoke(&["--json", "install"]));
    let unapply_fixture = Fixture::new("overlap-unapply");
    unapply_fixture.assert_small_owned_executable();
    let unapply_plan: InstallPreview = reply_body(unapply_fixture.invoke(&["--json", "install"]));
    let _: InstallReceipt = reply_body(unapply_fixture.invoke(&[
        "--json",
        "install",
        &format!("--apply={}", unapply_plan.digest()),
    ]));

    let overlap = Arc::new(Barrier::new(3));

    let preview_overlap = Arc::clone(&overlap);
    let preview = thread::spawn(move || {
        preview_overlap.wait();
        let _: InstallPreview = reply_body(preview_fixture.invoke(&["--json", "install"]));
    });

    let apply_overlap = Arc::clone(&overlap);
    let apply = thread::spawn(move || {
        apply_overlap.wait();
        let receipt: InstallReceipt = reply_body(apply_fixture.invoke(&[
            "--json",
            "install",
            &format!("--apply={}", apply_plan.digest()),
        ]));
        assert_eq!(receipt.status(), InstallStatus::Complete);
        assert_eq!(
            fs::read(apply_fixture.installed_binary())
                .expect("read overlapped installed executable"),
            fs::read(&apply_fixture.executable).expect("read overlapped fixture executable"),
        );
    });

    let unapply_overlap = Arc::clone(&overlap);
    let unapply = thread::spawn(move || {
        unapply_overlap.wait();
        let receipt: InstallReceipt =
            reply_body(unapply_fixture.invoke(&["--json", "install", "--unapply"]));
        assert_eq!(receipt.status(), InstallStatus::Uninstalled);
        assert!(!unapply_fixture.installed_binary().exists());
    });

    for process in [preview, apply, unapply] {
        process
            .join()
            .expect("overlapped install fixture completes");
    }
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

/// A paused install whose harness never releases it ends on its own bound.
///
/// Nothing writes `release/continue` here, so the only ending reachable is the
/// horizon the wait carries. That makes the fixture cost the whole ten seconds
/// by construction, and that cost is the point: it is what distinguishes a
/// bound that still terminates the event receiver from one merely written down
/// beside an unbounded receive.
#[test]
fn apply_pause_no_harness_releases_faults_when_its_bound_expires() {
    let fixture = Fixture::new("apply-abandoned-pause");
    let preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    let digest = preview.digest();
    let destination = fixture
        .root
        .join(".git/.spice-install/git-hooks/pre-commit");
    let paused = fixture.spawn_paused(
        &["--json", "install", &format!("--apply={digest}")],
        "publish",
        destination.clone(),
        Release::Withheld,
    );

    let expired = paused.abandon();

    assert_eq!(expired.code, Some(1));
    assert!(expired.out.is_empty());
    let outcome: Outcome<()> = wire::decode(&expired.err).expect("typed rendezvous fault decodes");
    let Outcome::Fault(fault) = outcome else {
        panic!("an unreleased install must fault: {outcome:?}");
    };
    assert_eq!(fault.class, wire::FaultClass::Environment);
    assert_eq!(fault.reason, "install::environment-failed");
    assert_eq!(
        fault.detail,
        format!(
            "failed to wait for install mutation rendezvous at {}: \
             test rendezvous timed out",
            fs::canonicalize(&fixture.root)
                .expect("canonicalize install fixture")
                .join(".git/.spice-install/git-hooks/pre-commit")
                .display()
        ),
    );
    assert!(!destination.exists());
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
fn killed_large_record_publication_is_reported_then_reclaimed_before_resume() {
    let fixture = Fixture::new("crashed-record-publication");
    let prior_hook = fixture
        .root
        .join(".git/.spice-install/git-hooks/pre-commit");
    fs::create_dir_all(prior_hook.parent().unwrap()).expect("create prior hook parent");
    fs::write(&prior_hook, vec![0x5a; MIN_PRIVATE_INSTALL_RECORD + 1])
        .expect("write large prior hook");
    let preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    let digest = preview.digest();
    let paused = fixture.spawn_paused(
        &["--json", "install", &format!("--apply={digest}")],
        "publish",
        fixture.record(),
        Release::Withheld,
    );

    let (pid, killed) = paused.kill();
    assert_eq!(killed.code, None);
    let pid = rustix::process::Pid::from_raw(pid.try_into().unwrap()).unwrap();
    assert_eq!(
        rustix::process::test_kill_process(pid),
        Err(rustix::io::Errno::SRCH),
        "killed installer PID must be definitely absent",
    );
    let temporaries = fixture.record_temps();
    let [temporary] = temporaries.as_slice() else {
        panic!(
            "a killed record publication must leave one temporary: {:?}",
            fixture.record_temps()
        );
    };
    assert!(fs::metadata(temporary).unwrap().len() > MIN_PRIVATE_INSTALL_RECORD as u64);
    assert_eq!(
        fs::metadata(temporary).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let before = file_snapshot(temporary);
    let immediate_preview: InstallPreview = reply_body(fixture.invoke(&["--json", "install"]));
    assert_eq!(immediate_preview.digest(), digest);
    assert_eq!(file_snapshot(temporary), before);
    let immediate = fixture.invoke(&["--json", "doctor"]);
    assert!(matches!(
        installation_observation(&immediate),
        DoctorObservation::NotApplicable { .. }
    ));
    assert_eq!(file_snapshot(temporary), before);

    let old_modified = SystemTime::now()
        .checked_sub(Duration::from_secs(2 * 60 * 60))
        .expect("wall clock admits an old fixture timestamp");
    File::options()
        .write(true)
        .open(temporary)
        .expect("open temporary record for timestamp aging")
        .set_times(fs::FileTimes::new().set_modified(old_modified))
        .expect("age temporary record through filesystem timestamps");
    let aged_snapshot = file_snapshot(temporary);
    let aged = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("wall clock follows Unix epoch")
        .as_secs()
        + 2 * 60 * 60;
    let aged_doctor = fixture.invoke_at(&["--json", "doctor"], aged);
    let DoctorObservation::InputRequired { reason } = installation_observation(&aged_doctor) else {
        panic!("aged crashed publication must require input");
    };
    assert!(reason.contains("1 stale install publication file(s)"));
    assert!(reason.contains("run spice install once"));
    assert!(reason.contains("receipt diagnosis: not-applicable: installation receipt is absent"));
    assert_eq!(file_snapshot(temporary), aged_snapshot);

    let resumed_preview: InstallPreview =
        reply_body(fixture.invoke_at(&["--json", "install"], aged));
    assert!(fixture.record_temps().is_empty());
    let resumed_digest = resumed_preview.digest();
    let receipt: InstallReceipt = reply_body(fixture.invoke_at(
        &["--json", "install", &format!("--apply={resumed_digest}")],
        aged,
    ));
    assert_eq!(receipt.status(), InstallStatus::Complete);

    let text = fixture.invoke_at(&["doctor"], aged);
    text.assert_success();
    assert!(!String::from_utf8_lossy(&text.out).contains("stale install publication"));
    let json = fixture.invoke_at(&["--json", "doctor"], aged);
    assert!(matches!(
        installation_observation(&json),
        DoctorObservation::Ready {}
    ));
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
