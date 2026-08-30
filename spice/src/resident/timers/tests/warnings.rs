use super::*;

#[test]
fn renewal_cancels_a_committed_warning_and_restart_rearms_the_new_generation() {
    let fixture = fixture_with_expiry(60);
    let mut timers = Runtime::start(fixture.projects.clone(), steer::Config::default())
        .expect("timer runtime starts");
    let (_, warned) = commit_warning(&fixture, &mut timers);
    let renewed_expiry = shifted(fixture.authority.expiry, 60);

    renew_claim(&fixture, renewed_expiry);
    assert_eq!(
        timers
            .synchronize(&fixture.bus)
            .expect("renewal reconciles stale warning"),
        BTreeSet::from([fixture.authority.lane]),
    );
    assert_warning_canceled(&fixture, &warned);
    assert_eq!(timers.warnings.len(), 1, "the new lease generation arms");

    drop(timers);
    let mut restarted = Runtime::start(fixture.projects.clone(), steer::Config::default())
        .expect("replacement timer runtime starts");
    assert!(
        restarted
            .synchronize(&fixture.bus)
            .expect("restart reconciles durable authority")
            .is_empty(),
        "restart found no stale warning left to repair"
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("fixture async runtime builds");
    let event = runtime
        .block_on(restarted.recv())
        .expect("renewed generation warning fires");
    let Ok(Fired::ClaimWarning(_)) = restarted.consume(&fixture.bus, event) else {
        panic!("renewed warning generation did not commit")
    };
    let pending = pending_warnings(&fixture);
    assert_eq!(pending.len(), 1);
    assert_ne!(pending[0].body().directive, warned.directive);

    drop(restarted);
    fixture.bus.shutdown().expect("fixture bus stops");
}

#[test]
fn committed_warning_is_canceled_after_release_settlement_or_residency_movement() {
    assert_committed_warning_canceled("claim release", free_claim);
    assert_committed_warning_canceled("task settlement", settle_task);
    assert_committed_warning_canceled("resident stop", stop_residency);
    assert_committed_warning_canceled("resident identity movement", |fixture| {
        move_residency(fixture, uuid(24), uuid(25));
    });
}

#[test]
fn committed_stale_warning_is_removed_from_the_materialized_inbox_across_restart() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("fixture clock follows the Unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "spice-warning-inbox-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join(".git")).expect("fixture git directory");
    let worktree = std::fs::canonicalize(&root).expect("fixture worktree canonicalizes");
    let paths = crate::resident::RuntimePaths::discover(&worktree).expect("runtime paths resolve");
    std::fs::create_dir_all(&paths.data).expect("runtime data directory");
    let fixture = fixture_with_expiry_at(60, worktree.to_string_lossy().into_owned());
    let mut timers = Runtime::start(fixture.projects.clone(), steer::Config::default())
        .expect("timer runtime starts");
    let (_, warned) = commit_warning(&fixture, &mut timers);
    let residency = fixture
        .bus
        .residency_head()
        .expect("residency reads")
        .expect("residency exists");
    crate::resident::steering::publish_inboxes(&fixture.bus, &paths, residency.body())
        .expect("committed warning materializes");
    let inbox = steer::inbox::path(&paths.inbox, fixture.authority.lane);
    assert!(
        std::fs::read_to_string(&inbox)
            .expect("materialized warning reads")
            .contains(&warned.key)
    );

    stop_residency(&fixture);
    let refresh = timers
        .synchronize(&fixture.bus)
        .expect("resident stop reconciles stale warning");
    crate::resident::runtime::refresh_warning_inboxes(&fixture.bus, &paths, refresh)
        .expect("stale warning inbox refreshes");
    assert_eq!(
        std::fs::read_to_string(&inbox).expect("refreshed inbox reads"),
        ""
    );

    drop(timers);
    let mut restarted = Runtime::start(fixture.projects.clone(), steer::Config::default())
        .expect("replacement timer runtime starts");
    restarted
        .synchronize(&fixture.bus)
        .expect("restart reconciles durable authority");
    let residency = fixture
        .bus
        .residency_head()
        .expect("restart residency reads")
        .expect("restart residency exists");
    crate::resident::steering::publish_inboxes(&fixture.bus, &paths, residency.body())
        .expect("restart materializes current inbox");
    assert_eq!(
        std::fs::read_to_string(inbox).expect("restart inbox reads"),
        ""
    );

    drop(restarted);
    fixture.bus.shutdown().expect("fixture bus stops");
    std::fs::remove_dir_all(root).expect("warning inbox fixture cleanup");
}

#[test]
fn committed_stale_warning_cancellation_preserves_unrelated_lane_steering() {
    let fixture = fixture_with_expiry(60);
    let mut timers = Runtime::start(fixture.projects.clone(), steer::Config::default())
        .expect("timer runtime starts");
    let (_, warned) = commit_warning(&fixture, &mut timers);
    let unrelated = commit_unrelated_steering(&fixture);

    renew_claim(&fixture, shifted(fixture.authority.expiry, 60));
    assert_eq!(
        timers
            .synchronize(&fixture.bus)
            .expect("renewal reconciles only stale warning"),
        BTreeSet::from([fixture.authority.lane]),
    );
    let pending = pending_warnings(&fixture);
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].body().directive, unrelated);
    let warning = fixture
        .bus
        .read({
            let directive = warned.directive;
            move |read| read.steering_head(directive)
        })
        .expect("warning standing reads")
        .expect("warning standing remains indexed");
    assert_eq!(warning.body().disposition, wire::Disposition::Canceled);

    drop(timers);
    fixture.bus.shutdown().expect("fixture bus stops");
}

fn commit_warning(
    fixture: &Fixture,
    timers: &mut Runtime,
) -> (WarningDelivery, body::ClaimLeaseWarned) {
    assert!(
        timers
            .synchronize(&fixture.bus)
            .expect("due warning arms")
            .is_empty()
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("fixture async runtime builds");
    let event = runtime.block_on(timers.recv()).expect("due warning fires");
    let Ok(Fired::ClaimWarning(delivery)) = timers.consume(&fixture.bus, event) else {
        panic!("due warning did not commit")
    };
    let warned = fixture
        .bus
        .fact_snapshot()
        .expect("warning facts read")
        .facts()
        .iter()
        .filter_map(|envelope| envelope.fact.read::<body::ClaimLeaseWarned>())
        .next_back()
        .expect("warning fact exists");
    (delivery, warned)
}

fn pending_warnings(fixture: &Fixture) -> Vec<journal::JournalHead<wire::SteeringHead>> {
    fixture
        .bus
        .read({
            let lane = fixture.authority.lane;
            move |read| read.pending_steering(Some(lane))
        })
        .expect("warning inbox reads")
}

fn assert_warning_canceled(fixture: &Fixture, warned: &body::ClaimLeaseWarned) {
    assert!(pending_warnings(fixture).is_empty());
    let head = fixture
        .bus
        .read({
            let directive = warned.directive;
            move |read| read.steering_head(directive)
        })
        .expect("warning standing reads")
        .expect("warning standing remains indexed");
    assert_eq!(head.body().disposition, wire::Disposition::Canceled);
    assert!(
        fixture
            .bus
            .fact_snapshot()
            .expect("warning cancellation facts read")
            .facts()
            .iter()
            .any(|envelope| envelope.fact.kind() == wire::FactKind::SteerCanceled)
    );
}

fn assert_committed_warning_canceled(change: &str, mutate: impl FnOnce(&Fixture)) {
    let fixture = fixture_with_expiry(60);
    let mut timers = Runtime::start(fixture.projects.clone(), steer::Config::default())
        .expect("timer runtime starts");
    let (_, warned) = commit_warning(&fixture, &mut timers);

    mutate(&fixture);
    assert_eq!(
        timers
            .synchronize(&fixture.bus)
            .unwrap_or_else(|error| panic!("{change} reconciles stale warning: {error:?}")),
        BTreeSet::from([fixture.authority.lane]),
        "{change} refreshes the warned lane",
    );
    assert_warning_canceled(&fixture, &warned);

    drop(timers);
    fixture.bus.shutdown().expect("fixture bus stops");
}

fn commit_unrelated_steering(fixture: &Fixture) -> CanonicalUuid {
    let at = Instant::new(jiff::Timestamp::now()).expect("fixture clock");
    let directive = uuid(80);
    let body = body::SteerSent {
        directive,
        fanout: None,
        key: "1kJK6N3R".to_owned(),
        lane: fixture.authority.lane,
        text: "preserve unrelated direction".to_owned(),
        control: None,
        attachments: Vec::new(),
        expires: shifted(at, 3600),
    };
    let origin: Origin = format!("resident:{}", fixture.authority.epoch)
        .parse()
        .expect("fixture origin");
    let mut write = JournalWrite::new(Actor::Operator, origin.clone(), at, Producer::CommandDoor);
    let sent = write
        .fact(body.clone(), FactCause::Root)
        .expect("unrelated steering fact builds");
    write.steering_head(sent, move |seq| {
        steer::Ledger::sent_head(&body, origin, at, seq).expect("unrelated steering head builds")
    });
    assert!(matches!(
        fixture
            .bus
            .commit(write)
            .expect("unrelated steering commits"),
        bus::CommitOutcome::Committed(_)
    ));
    directive
}

fn stop_residency(fixture: &Fixture) {
    let observed = fixture
        .bus
        .residency_head()
        .expect("residency reads")
        .expect("residency exists");
    let mut next = observed.body().clone();
    next.state = ResidentState::Dark;
    let epoch = fixture.authority.epoch;
    let mut write = JournalWrite::new(
        Actor::Operator,
        format!("resident:{epoch}").parse().expect("fixture origin"),
        Instant::new(jiff::Timestamp::now()).expect("fixture clock is canonical"),
        Producer::CommandDoor,
    );
    let dark = write
        .fact(body::UpDark { epoch }, FactCause::Root)
        .expect("resident stop fact builds");
    write.residency_head(dark, move |_| next);
    assert!(matches!(
        fixture.bus.commit(write).expect("resident stop commits"),
        bus::CommitOutcome::Committed(_)
    ));
}
