#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Instant as Monotonic;

    use sill::{
        Argv, Cancellation, Deadlines, Environment, Output, ProcessGroup, Redaction,
        RestartDisposition, Spec, StdinPolicy, StreamPolicy, WaitPolicy,
    };

    use super::*;

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    fn fixture(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "spice-resident-{name}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn runtime_paths_follow_the_repository_common_directory() {
        let root = fixture("main");
        let nested = root.join("a/b");
        fs::create_dir_all(root.join(".git")).expect("git directory");
        fs::create_dir_all(&nested).expect("nested directory");

        let paths = RuntimePaths::discover(&nested).expect("paths resolve");
        let common = fs::canonicalize(root.join(".git")).expect("canonical common directory");

        assert_eq!(paths.runtime, common.join(".spice-rust"));
        assert_eq!(
            paths.journal,
            common.join(".spice-rust/data/spicejournal.sqlite3")
        );
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn stale_surface_cleanup_is_idempotent_and_leaves_inbox_authority() {
        let root = fixture("stale-surfaces");
        fs::create_dir_all(root.join(".git")).expect("git directory");
        let paths = RuntimePaths::discover(&root).expect("paths resolve");
        fs::create_dir_all(&paths.lane_canvas).expect("lane discovery directory");
        fs::write(&paths.canvas, b"stale operator discovery").expect("operator discovery");
        fs::write(paths.lane_canvas.join("lane"), b"stale lane discovery")
            .expect("lane discovery");
        fs::create_dir_all(&paths.inbox).expect("inbox directory");
        fs::write(paths.inbox.join("lane"), b"durable pending direction").expect("lane inbox");

        paths.clear_stale_surfaces().expect("stale surfaces clear");
        paths
            .clear_stale_surfaces()
            .expect("absent surfaces clear idempotently");

        assert!(!paths.canvas.exists());
        assert!(!paths.lane_canvas.exists());
        assert_eq!(
            fs::read(paths.inbox.join("lane")).expect("inbox remains"),
            b"durable pending direction"
        );
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn linked_worktrees_share_the_git_common_directory() {
        let root = fixture("linked");
        let worktree = root.join("worktree");
        let common = root.join("common.git");
        let git_dir = common.join("worktrees/linked");
        fs::create_dir_all(&worktree).expect("worktree directory");
        fs::create_dir_all(&git_dir).expect("git directory");
        fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", git_dir.display()),
        )
        .expect("git marker");
        fs::write(git_dir.join("commondir"), "../..\n").expect("common marker");

        let paths = RuntimePaths::discover(&worktree).expect("paths resolve");
        let common = fs::canonicalize(&common).expect("canonical common directory");

        assert_eq!(paths.runtime, common.join(".spice-rust"));
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn policy_movement_after_pend_refuses_seating_and_retry_pends_the_new_digest() {
        let (root, paths) = resident_fixture("trust-pend-generation");
        let tracked = root.join("spice.toml");
        fs::write(&tracked, "# tracked policy v1\n").expect("write first policy");
        let repository = Repository::discover(&root).expect("fixture repository resolves");
        let surface =
            crate::trust_command::Surface::load(&repository).expect("first surface loads");
        let first_digest = surface.digest().clone();
        let journal = journal::Journal::open(&paths.journal).expect("fixture journal opens");
        let bus = Bus::start(journal).expect("fixture bus starts");

        let refused =
            commit_startup_with(&bus, &repository, &surface, test_uuid(10), None, || {
                replace_policy(&tracked, "# tracked policy v2\n")
            })
            .expect_err("moved policy must refuse seating");
        assert!(matches!(
            refused,
            Outcome::Refusal(ref refusal) if refusal.reason == "trust::surface-moved"
        ));
        let after_refusal = bus.fact_snapshot().expect("refused startup facts read");
        assert_eq!(trust_pend_digests(&after_refusal), [first_digest]);
        assert!(
            after_refusal
                .facts()
                .iter()
                .all(|fact| fact.fact.kind() != wire::FactKind::UpSeated)
        );

        let current =
            crate::trust_command::Surface::load(&repository).expect("replacement surface loads");
        let current_digest = current.digest().clone();
        assert_ne!(current_digest, surface.digest().clone());
        commit_startup(&bus, &repository, &current, test_uuid(11), None)
            .expect("retry seats current surface");
        let after_retry = bus.fact_snapshot().expect("retry facts read");
        assert_eq!(
            trust_pend_digests(&after_retry),
            [surface.digest().clone(), current_digest]
        );
        assert_eq!(
            after_retry
                .facts()
                .iter()
                .filter(|fact| fact.fact.kind() == wire::FactKind::UpSeated)
                .count(),
            1
        );
        bus.verify().expect("retried startup authority verifies");
        bus.shutdown().expect("fixture bus stops");
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn equal_byte_replacement_with_an_existing_head_refuses_zero_write_seating() {
        let (root, paths) = resident_fixture("trust-existing-generation");
        let tracked = root.join("spice.toml");
        let bytes = "# stable tracked policy\n";
        fs::write(&tracked, bytes).expect("write tracked policy");
        let repository = Repository::discover(&root).expect("fixture repository resolves");
        let surface =
            crate::trust_command::Surface::load(&repository).expect("first surface loads");
        let journal = journal::Journal::open(&paths.journal).expect("fixture journal opens");
        let bus = Bus::start(journal).expect("fixture bus starts");
        commit_startup(&bus, &repository, &surface, test_uuid(12), None)
            .expect("first generation seats");
        let before = bus.fact_snapshot().expect("authority before replacement");

        let refused =
            commit_startup_with(&bus, &repository, &surface, test_uuid(13), None, || {
                replace_policy(&tracked, bytes)
            })
            .expect_err("same-byte generation replacement must refuse");
        assert!(matches!(
            refused,
            Outcome::Refusal(ref refusal) if refusal.reason == "trust::surface-moved"
        ));
        assert_eq!(
            bus.fact_snapshot().expect("authority after refusal"),
            before,
            "refused seating changed journal authority"
        );
        bus.verify().expect("refused startup authority verifies");
        bus.shutdown().expect("fixture bus stops");
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn grant_and_revoke_generation_movement_preserve_facts_heads_and_receipts() {
        let (root, paths) = resident_fixture("trust-ruling-generation");
        let tracked = root.join("spice.toml");
        let bytes = "# stable ruling policy\n";
        fs::write(&tracked, bytes).expect("write tracked policy");
        let repository = Repository::discover(&root).expect("fixture repository resolves");
        let surface =
            crate::trust_command::Surface::load(&repository).expect("trust surface loads");
        let digest = surface.digest().clone();
        let epoch = test_uuid(20);
        let journal = journal::Journal::open(&paths.journal).expect("fixture journal opens");
        let bus = Bus::start(journal).expect("fixture bus starts");
        commit_startup(&bus, &repository, &surface, epoch, None)
            .expect("surface pends and seats");

        let grant = admitted_trust_command(
            CommandKind::TrustGrant,
            digest.clone(),
            TrustState::Pended,
            epoch,
            test_uuid(21),
        );
        let reference = TrustRef {
            digest: digest.clone(),
        };
        let before_grant = bus.fact_snapshot().expect("facts before refused grant");
        let head_before_grant = bus
            .trust_head(&digest)
            .expect("head before refused grant reads");
        let receipts_before_grant = receipt_count(&paths.journal);
        let refused = dispatch_trust_rule_with(
            &bus,
            &repository,
            epoch,
            &grant,
            &reference,
            || replace_policy(&tracked, bytes),
        );
        assert!(matches!(
            refused,
            Outcome::Refusal(ref refusal) if refusal.reason == "trust::surface-moved"
        ));
        assert_eq!(
            bus.fact_snapshot().expect("facts after refused grant"),
            before_grant
        );
        assert_eq!(
            bus.trust_head(&digest)
                .expect("head after refused grant reads"),
            head_before_grant
        );
        assert_eq!(receipt_count(&paths.journal), receipts_before_grant);

        let current =
            crate::trust_command::Surface::load(&repository).expect("current surface loads");
        assert_eq!(current.digest(), &digest);
        let granted = dispatch_trust_rule(&bus, &repository, epoch, &grant, &reference);
        assert!(matches!(granted, Outcome::Reply(_)));

        let revoke = admitted_trust_command(
            CommandKind::TrustRevoke,
            digest.clone(),
            TrustState::Granted,
            epoch,
            test_uuid(22),
        );
        let before_revoke = bus.fact_snapshot().expect("facts before refused revoke");
        let head_before_revoke = bus
            .trust_head(&digest)
            .expect("head before refused revoke reads");
        let receipts_before_revoke = receipt_count(&paths.journal);
        let refused = dispatch_trust_rule_with(
            &bus,
            &repository,
            epoch,
            &revoke,
            &reference,
            || publish_local_policy(&root, "# newly published local policy\n"),
        );
        assert!(matches!(
            refused,
            Outcome::Refusal(ref refusal) if refusal.reason == "trust::surface-moved"
        ));
        assert_eq!(
            bus.fact_snapshot().expect("facts after refused revoke"),
            before_revoke
        );
        assert_eq!(
            bus.trust_head(&digest)
                .expect("head after refused revoke reads"),
            head_before_revoke
        );
        assert_eq!(receipt_count(&paths.journal), receipts_before_revoke);
        bus.verify().expect("ruling authority verifies");
        bus.shutdown().expect("fixture bus stops");
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn cooperative_and_forced_groups_drive_observed_counts_and_residue() {
        let (root, paths) = resident_fixture("groups");
        let repository = Repository::discover(&root).expect("fixture repository resolves");
        let surface =
            crate::trust_command::Surface::load(&repository).expect("fixture trust surface loads");
        let journal = journal::Journal::open(&paths.journal).expect("fixture journal opens");
        let bus = Bus::start(journal).expect("fixture bus starts");
        let epoch = test_uuid(1);
        commit_startup(&bus, &repository, &surface, epoch, None).expect("resident seats");
        let mut seats = Seats::new();

        let drained_lane = test_uuid(2);
        let drained_occupant = test_uuid(3);
        seats
            .seat(
                drained_lane,
                drained_occupant,
                process_spec("sleep 0.05", Duration::from_secs(5)),
            )
            .expect("cooperative group starts");
        seed_lane(&bus, epoch, drained_lane, drained_occupant);

        let killed_lane = test_uuid(4);
        let killed_occupant = test_uuid(5);
        seats
            .seat(
                killed_lane,
                killed_occupant,
                process_spec(
                    "trap '' TERM; while :; do sleep 1; done",
                    Duration::from_millis(100),
                ),
            )
            .expect("non-cooperative group starts");
        seed_lane(&bus, epoch, killed_lane, killed_occupant);

        fs::create_dir_all(&paths.degraded).expect("degraded directory");
        fs::write(
            paths.degraded.join(drained_lane.to_string()),
            "offline run\n",
        )
        .expect("degraded marker");
        fs::create_dir_all(&paths.tape).expect("tape directory");
        let tape_lane = test_uuid(6);
        fs::write(paths.tape.join(tape_lane.to_string()), "not a tape")
            .expect("malformed tape residue");
        let sealed_lane = test_uuid(8);
        let tape =
            tape::Tape::open(paths.tape.join(sealed_lane.to_string())).expect("live tape opens");
        let locus = tape
            .tee(b"resident shutdown evidence")
            .expect("tape captures");

        let shutdown_started = Monotonic::now();
        assert!(matches!(
            drive_shutdown(&bus, &paths, epoch, &mut seats),
            Outcome::Reply(_)
        ));
        assert!(
            shutdown_started.elapsed() < Duration::from_secs(2),
            "one group's unused runtime extended another group's drain bound"
        );
        assert!(seats.is_empty());
        bus.verify().expect("Dark authority verifies");
        let snapshot = bus.fact_snapshot().expect("facts read through bus");
        assert_eq!(
            fact_u64(&snapshot, wire::FactKind::UpDraining, "occupants"),
            2
        );
        assert_eq!(
            fact_u64(&snapshot, wire::FactKind::UpTerminating, "occupants"),
            1
        );
        assert_eq!(
            fact_u64(&snapshot, wire::FactKind::UpKilling, "occupants"),
            1
        );
        assert!(
            snapshot
                .facts()
                .iter()
                .any(|fact| fact.fact.kind() == wire::FactKind::LaneDrained)
        );
        assert!(
            snapshot
                .facts()
                .iter()
                .any(|fact| fact.fact.kind() == wire::FactKind::LaneExited)
        );
        let degraded = snapshot
            .facts()
            .iter()
            .filter(|fact| fact.fact.kind() == wire::FactKind::LaneDegraded)
            .count();
        assert_eq!(degraded, 2, "disk residue was not named once per lane");
        let head = bus
            .residency_head()
            .expect("head read")
            .expect("head exists")
            .into_body();
        assert_eq!(head.state, ResidentState::Dark);
        assert_eq!(head.lanes.len(), 2);
        assert!(head.lanes.iter().all(|lane| lane.occupant.is_none()));
        assert!(!paths.degraded.join(drained_lane.to_string()).exists());
        assert_eq!(
            tape.read(locus).expect("sealed tape remains readable"),
            b"resident shutdown evidence"
        );
        assert!(matches!(
            tape.seal().expect("tape was sealed"),
            tape::Seal::Empty
        ));
        bus.shutdown().expect("fixture bus stops");
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn a_post_seated_surface_failure_compensates_to_verified_dark() {
        let (root, paths) = resident_fixture("compensation");
        let repository = Repository::discover(&root).expect("fixture repository resolves");
        let surface =
            crate::trust_command::Surface::load(&repository).expect("fixture trust surface loads");
        let journal = journal::Journal::open(&paths.journal).expect("fixture journal opens");
        let bus = Bus::start(journal).expect("fixture bus starts");
        let epoch = test_uuid(7);
        let startup =
            commit_startup(&bus, &repository, &surface, epoch, None).expect("resident seats");
        let original = environment_fault("canvas::fixture-failed", "after UpSeated");

        let settled = settle_surface(
            &bus,
            &paths,
            epoch,
            startup,
            &mut Seats::new(),
            Err(original),
        );

        let Outcome::Fault(fault) = settled else {
            panic!("the original startup failure must remain visible");
        };
        assert_eq!(fault.reason, "canvas::fixture-failed");
        let head = bus
            .residency_head()
            .expect("head read")
            .expect("head exists")
            .into_body();
        assert_eq!(head.state, ResidentState::Dark);
        assert!(head.lanes.is_empty());
        bus.verify().expect("compensated authority verifies");
        bus.shutdown().expect("fixture bus stops");
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    fn resident_fixture(name: &str) -> (PathBuf, RuntimePaths) {
        let root = fixture(name);
        fs::create_dir_all(root.join(".git")).expect("git directory");
        let paths = RuntimePaths::discover(&root).expect("runtime paths resolve");
        fs::create_dir_all(&paths.data).expect("data directory");
        (root, paths)
    }

    fn process_spec(command: &str, runtime: Duration) -> Spec {
        Spec {
            argv: Argv::new("/bin/sh", ["-c", command]).expect("fixture argv"),
            cwd: PathBuf::from("/"),
            environment: Environment::new([("PATH", "/usr/bin:/bin")])
                .expect("fixture environment"),
            group: ProcessGroup::New,
            deadlines: Deadlines::new(runtime, Duration::from_secs(1), Duration::from_secs(1))
                .expect("fixture deadlines"),
            wait: WaitPolicy::new(
                "resident fixture",
                Cancellation::Never,
                RestartDisposition::Never,
            )
            .expect("fixture wait policy"),
            output: Output::new(StdinPolicy::Null, StreamPolicy::Null, StreamPolicy::Null),
            redaction: Redaction::None,
            grace: Duration::from_millis(100),
        }
    }

    fn seed_lane(bus: &Bus, epoch: CanonicalUuid, lane: CanonicalUuid, occupant: CanonicalUuid) {
        let current = current_residency(bus, epoch).expect("current residency");
        let mut head = current.body().clone();
        head.lanes.push(wire::LaneResidency {
            lane,
            occupant: Some(occupant),
            lineage: vec![occupant],
            capabilities: vec![test_uuid(9)],
            disposition: "occupied".to_owned(),
        });
        head.lanes.sort_by_key(|lane| lane.lane);
        let mut write = write(Actor::Spice, epoch, Producer::ResidentRecognition)
            .expect("fixture write starts");
        let seated = write
            .fact(
                wire::body::LaneSeated {
                    lane,
                    capability: test_uuid(9),
                    worktree: "/fixture".to_owned(),
                },
                FactCause::Existing(current.position()),
            )
            .expect("lane seats");
        let started = write
            .fact(
                wire::body::LaneStarted {
                    lane,
                    occupant,
                    epoch,
                },
                FactCause::Pending(seated),
            )
            .expect("occupant starts");
        write.residency_head(started, move |_| head);
        commit(bus, write).expect("fixture lane commits");
    }

    fn fact_u64(snapshot: &journal::FactSnapshot, kind: wire::FactKind, field: &str) -> u64 {
        snapshot
            .facts()
            .iter()
            .find(|fact| fact.fact.kind() == kind)
            .and_then(|fact| fact.fact.body().get(field))
            .and_then(|value| value.as_u64())
            .expect("fact carries numeric fixture field")
    }

    fn replace_policy(path: &Path, bytes: &str) {
        let replacement = path.with_extension("replacement");
        fs::write(&replacement, bytes).expect("write policy replacement");
        fs::rename(&replacement, path).expect("publish policy replacement");
    }

    fn publish_local_policy(root: &Path, bytes: &str) {
        let parent = root.join(".git/.spice");
        fs::create_dir(&parent).expect("publish local policy parent");
        fs::write(parent.join("config.toml"), bytes).expect("publish local policy");
    }

    fn trust_pend_digests(snapshot: &journal::FactSnapshot) -> Vec<wire::Digest> {
        snapshot
            .facts()
            .iter()
            .filter(|fact| fact.fact.kind() == wire::FactKind::TrustPended)
            .map(|envelope| {
                let bytes = wire::encode(&envelope.fact).expect("trust fact encodes");
                wire::decode::<wire::Fact<wire::body::TrustPended>>(&bytes)
                    .expect("trust fact decodes")
                    .body
                    .digest
            })
            .collect()
    }

    fn admitted_trust_command(
        kind: CommandKind,
        digest: wire::Digest,
        standing: TrustState,
        epoch: CanonicalUuid,
        id: CanonicalUuid,
    ) -> wire::AdmittedCommand {
        let reference = TrustRef {
            digest: digest.clone(),
        };
        wire::Command {
            v: wire::WireVersion::CURRENT,
            id,
            epoch,
            kind,
            expected: vec![Witness::Trust { digest, standing }],
            body: serde_json::to_value(reference).expect("trust reference encodes"),
        }
        .admit(&wire::Accepted {
            wire: wire::WireVersion::CURRENT,
            actor: Actor::Operator,
            epoch,
        })
        .expect("fixture trust command admits")
    }

    fn receipt_count(path: &Path) -> u64 {
        let count: i64 = rusqlite::Connection::open(path)
            .expect("receipt fixture opens")
            .query_row("SELECT COUNT(*) FROM command_receipts", [], |row| {
                row.get(0)
            })
            .expect("receipt count reads");
        u64::try_from(count).expect("receipt count is nonnegative")
    }

    fn test_uuid(last: u8) -> CanonicalUuid {
        format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02}")
            .parse()
            .expect("fixture UUID")
    }
}
