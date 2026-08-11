use super::*;

#[test]
fn latches_hold_heal_and_redirect_a_peer_only_after_acceptance() {
    let fixture = Fixture::new();
    let repository = production_repository(&fixture.root);
    let document = "app/notes.md";

    stage_document(&repository, document, 24_000);
    let first = crate::BatteryStateRun::capture_at(&repository, 100.0)
        .expect("capture first production state run");
    let first_report = focused_in_state(&repository, "actor-a", first.clone(), GateId::RepoDocs);
    assert!(first_report.admits(), "{}", first_report.result().reason());
    first
        .accept(crate::StateRunScope::Partial)
        .expect("publish the accepted flex-slice claim");
    let claimed = live_ledger(&repository);
    assert!(claimed.latches().is_empty());
    assert_eq!(claimed.claims().len(), 1);
    assert_eq!(claimed.claims()[0].actor(), "actor-a");

    stage_document(&repository, document, 32_000);
    let latch = crate::BatteryStateRun::capture_at(&repository, 200.0)
        .expect("capture peer-redirection run");
    let latched_report = focused_in_state(&repository, "actor-b", latch.clone(), GateId::RepoDocs);
    assert!(
        latched_report.admits(),
        "a peer-held breach is an informational redirect: {}",
        latched_report.result().reason()
    );
    assert_eq!(
        latched_report.context().bounds()[0].latch_transition(),
        "latched"
    );
    latch
        .accept(crate::StateRunScope::Partial)
        .expect("publish accepted latch creation");
    let latched = live_ledger(&repository);
    assert_eq!(latched.latches().len(), 1);
    assert_eq!(latched.latches()[0].key().repo_path(), document);
    assert_eq!(latched.claims()[0].actor(), "actor-a");

    stage_document(&repository, document, 24_000);
    let hold =
        crate::BatteryStateRun::capture_at(&repository, 300.0).expect("capture held-latch run");
    let held_report = focused_in_state(&repository, "actor-b", hold.clone(), GateId::RepoDocs);
    assert!(held_report.admits(), "{}", held_report.result().reason());
    assert_eq!(held_report.context().bounds()[0].latch_transition(), "held");
    hold.accept(crate::StateRunScope::Partial)
        .expect("publish accepted latch hold");
    assert_eq!(live_ledger(&repository).latches().len(), 1);

    stage_document(&repository, document, 19_000);
    let heal = crate::BatteryStateRun::capture_at(&repository, 400.0).expect("capture healing run");
    let healed_report = focused_in_state(&repository, "actor-b", heal.clone(), GateId::RepoDocs);
    assert!(
        healed_report.admits(),
        "{}",
        healed_report.result().reason()
    );
    assert_eq!(
        healed_report.context().bounds()[0].latch_transition(),
        "healed"
    );
    heal.accept(crate::StateRunScope::Partial)
        .expect("publish accepted latch healing");
    assert!(live_ledger(&repository).latches().is_empty());
}

#[test]
fn rejection_and_stale_publication_leave_prior_state_unchanged() {
    let fixture = Fixture::new();
    let repository = production_repository(&fixture.root);
    let (_, common_git) = crate::tree::git_directories(&repository).expect("fixture Git paths");
    let destination = crate::current_state_path(&common_git);
    stage_document(&repository, "app/notes.md", 32_000);

    let rejected =
        crate::BatteryStateRun::capture_at(&repository, 100.0).expect("capture rejected run");
    let report = focused_in_state(
        &repository,
        "unclaimed-actor",
        rejected.clone(),
        GateId::RepoDocs,
    );
    assert!(!report.admits());
    assert!(!destination.exists());
    rejected.reject().expect("reject in-memory proposal");
    assert!(!destination.exists());

    let first = crate::BatteryStateRun::capture_at(&repository, 200.0)
        .expect("capture first concurrent state");
    let second = crate::BatteryStateRun::capture_at(&repository, 200.0)
        .expect("capture second concurrent state");
    let first_report = focused_in_state(&repository, "actor-a", first.clone(), GateId::Staging);
    let second_report = focused_in_state(&repository, "actor-b", second.clone(), GateId::Staging);
    assert!(first_report.admits() && second_report.admits());
    first
        .accept(crate::StateRunScope::Partial)
        .expect("first publication wins");
    let accepted_bytes = fs::read(&destination).expect("accepted current state bytes");
    let error = second
        .accept(crate::StateRunScope::Partial)
        .expect_err("stale production publication must refuse");
    assert!(error.detail().contains("stale"), "{error}");
    assert_eq!(
        fs::read(&destination).expect("state after stale refusal"),
        accepted_bytes
    );
}

#[test]
fn focused_acceptance_preserves_unrelated_live_state() {
    let fixture = Fixture::new();
    let repository = production_repository(&fixture.root);
    let (_, common_git) = crate::tree::git_directories(&repository).expect("fixture Git paths");
    let destination = crate::current_state_path(&common_git);
    let live_paths = crate::tree::tracked_paths(&repository).expect("fixture tracked paths");
    let mut seed = CurrentState::load(&destination).expect("load seed transaction");
    seed.begin_run(&crate::RenameMap::default(), &live_paths, 10.0)
        .expect("begin unrelated seed");
    seed.reconcile([crate::LatchObservation::new(
        crate::LatchEntry::new(
            crate::LatchKind::FileLoc,
            crate::StateKey::path("src/lib.rs").expect("unrelated path key"),
        )
        .expect("unrelated latch entry"),
        crate::BoundRule::new("fixture", "file-loc", 10, 15, false, false)
            .expect("unrelated bound"),
        20,
        ActorInputs::new("src/lib.rs")
            .expect("unrelated actor inputs")
            .with_active_actor("unrelated-actor"),
        0,
    )
    .expect("unrelated observation")])
        .expect("seed unrelated latch");
    seed.claim(
        ["src/lib.rs"],
        &ActorInputs::new("src/lib.rs")
            .expect("unrelated claim inputs")
            .with_active_actor("unrelated-actor"),
    )
    .expect("seed unrelated claim");
    seed.finish()
        .expect("finish unrelated seed")
        .accept()
        .expect("publish unrelated seed");
    let before = fs::read(&destination).expect("seeded state bytes");

    let focused =
        crate::BatteryStateRun::capture_at(&repository, 20.0).expect("capture focused state run");
    let report = focused_in_state(
        &repository,
        "focused-actor",
        focused.clone(),
        GateId::RepoDocs,
    );
    assert!(report.admits(), "{}", report.result().reason());
    focused
        .accept(crate::StateRunScope::Partial)
        .expect("publish focused state run");

    assert_eq!(
        fs::read(&destination).expect("focused state bytes"),
        before,
        "a focused observation must not interpret an unrelated live latch as absent"
    );
}

#[test]
fn exhaustive_acceptance_prunes_removed_latches_and_expired_claims() {
    let fixture = Fixture::new();
    let repository = production_repository(&fixture.root);
    let removed = "src/removed.rs";
    fs::write(repository.join(removed), "pub fn removed() {}\n").expect("write removable source");
    fixture_git(&repository, &["add", removed]);

    let (_, common_git) = crate::tree::git_directories(&repository).expect("fixture Git paths");
    let destination = crate::current_state_path(&common_git);
    let live_paths = crate::tree::tracked_paths(&repository).expect("seed tracked paths");
    let mut seed = CurrentState::load(&destination).expect("load exhaustive seed transaction");
    seed.begin_run(&crate::RenameMap::default(), &live_paths, 10.0)
        .expect("begin exhaustive seed");
    seed.reconcile([crate::LatchObservation::new(
        crate::LatchEntry::new(
            crate::LatchKind::FileLoc,
            crate::StateKey::path(removed).expect("removed path key"),
        )
        .expect("removed latch entry"),
        crate::BoundRule::new("fixture", "file-loc", 10, 15, false, false).expect("removed bound"),
        20,
        ActorInputs::new(removed)
            .expect("removed actor inputs")
            .with_active_actor("seed-actor"),
        0,
    )
    .expect("removed observation")])
        .expect("seed removed latch");
    seed.claim(
        [removed, "app/main.py"],
        &ActorInputs::new(removed)
            .expect("seed claim inputs")
            .with_active_actor("seed-actor"),
    )
    .expect("seed removed and expiring claims");
    seed.finish()
        .expect("finish exhaustive seed")
        .accept()
        .expect("publish exhaustive seed");
    assert_eq!(live_ledger(&repository).latches().len(), 1);
    assert_eq!(live_ledger(&repository).claims().len(), 2);

    fs::remove_file(repository.join(removed)).expect("remove staged source");
    fixture_git(&repository, &["add", "-u", "--", removed]);
    let exhaustive = crate::BatteryStateRun::capture_at(&repository, 30_000.0)
        .expect("capture exhaustive production state run");
    let report = focused_in_state(
        &repository,
        "exhaustive-actor",
        exhaustive.clone(),
        GateId::Staging,
    );
    assert!(report.admits(), "{}", report.result().reason());
    exhaustive
        .accept(crate::StateRunScope::Exhaustive)
        .expect("publish exhaustive pruning");

    let pruned = live_ledger(&repository);
    assert!(pruned.latches().is_empty());
    assert!(pruned.claims().is_empty());
}

fn focused_in_state(
    repository: &Path,
    actor: &str,
    state_run: crate::BatteryStateRun,
    gate_id: GateId,
) -> crate::FocusedArmReport {
    let mut capture = crate::BatteryCapture::capture_arm_in_state(
        repository,
        &crate::Selector::staged(),
        crate::ClaimCapture::Idle,
        acting(actor),
        crate::RimCapture::new(repository, None, ["app/notes.md".to_owned()]).expect("fixture rim"),
        crate::HygieneConfiguration::new(["app/main.py"], ["tests"]).expect("fixture hygiene"),
        gate_id,
        state_run,
    )
    .expect("capture focused production path");
    capture
        .execute_arm_report(gate_id)
        .expect("execute focused production path")
}

fn stage_document(repository: &Path, path: &str, characters: usize) {
    fs::write(repository.join(path), "x".repeat(characters)).expect("write staged document");
    fixture_git(repository, &["add", path]);
}
