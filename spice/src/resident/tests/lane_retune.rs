//! Retuning durable seat model and effort selections.
//!
//! Both selections are the launcher's authoritative inputs across relaunch,
//! recovery, and succession. A running occupant changes only after its
//! supervisor confirms the exact live provider command; these fixture
//! occupants have no command door, so they exercise the truthful next-launch
//! fallback without replacing the supervised process.

use super::*;

/// One seated, supervised lane and the fixture that keeps it running.
struct Seated {
    root: PathBuf,
    paths: RuntimePaths,
    bus: Bus,
    epoch: CanonicalUuid,
    seats: Seats,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
}

impl Seated {
    fn new(name: &str) -> Self {
        let (root, paths) = resident_fixture(name);
        let repository = Repository::discover(&root).expect("fixture repository resolves");
        let surface =
            crate::trust_command::Surface::load(&repository).expect("fixture trust surface loads");
        let journal = journal::Journal::open(&paths.journal).expect("fixture journal opens");
        let bus = Bus::start(journal).expect("fixture bus starts");
        let epoch = test_uuid(1);
        commit_startup(&bus, &repository, &surface, epoch, None).expect("resident seats");
        let lane = test_uuid(2);
        let occupant = test_uuid(3);
        let mut seats = Seats::new();
        seats
            .seat(
                lane,
                occupant,
                process_spec("sleep 5", Duration::from_secs(30)),
            )
            .expect("supervised occupant starts");
        seed_lane_in_team(&bus, epoch, lane, lane, occupant, &root);
        Self {
            root,
            paths,
            bus,
            epoch,
            seats,
            lane,
            occupant,
        }
    }

    fn retune(&self, id: u8, lane: CanonicalUuid, model: Option<&str>) -> Outcome<CommandAnswer> {
        self.retune_at(id, lane, model, self.head().topology_revision)
    }

    fn retune_at(
        &self,
        id: u8,
        lane: CanonicalUuid,
        model: Option<&str>,
        topology_revision: u64,
    ) -> Outcome<CommandAnswer> {
        let request = wire::intent::LaneRetune {
            topology_revision,
            lane,
            model: model.map(str::to_owned),
        };
        let command = Command {
            v: wire::WireVersion::CURRENT,
            id: test_uuid(id),
            epoch: self.epoch,
            kind: CommandKind::LaneRetune,
            expected: Vec::new(),
            body: serde_json::to_value(&request).expect("lane retune encodes"),
        }
        .admit(&wire::Accepted {
            wire: wire::WireVersion::CURRENT,
            actor: Actor::Operator,
            epoch: self.epoch,
        })
        .expect("operator retune admits");
        super::super::lane_surface::retune(
            &self.bus,
            &self.paths,
            self.epoch,
            &self.seats,
            &command,
            &request,
        )
    }
    fn retune_effort(
        &self,
        id: u8,
        effort: Option<&str>,
        topology_revision: u64,
    ) -> Outcome<CommandAnswer> {
        let (command, request) = self.effort_command(id, effort, topology_revision);
        super::super::lane_surface::retune_effort(
            &self.bus,
            &self.paths,
            self.epoch,
            &self.seats,
            &command,
            &request,
        )
    }

    fn effort_command(
        &self,
        id: u8,
        effort: Option<&str>,
        topology_revision: u64,
    ) -> (wire::AdmittedCommand, wire::intent::LaneEffortRetune) {
        let request = wire::intent::LaneEffortRetune {
            topology_revision,
            lane: self.lane,
            effort: effort.map(str::to_owned),
        };
        let command = Command {
            v: wire::WireVersion::CURRENT,
            id: test_uuid(id),
            epoch: self.epoch,
            kind: CommandKind::LaneEffortRetune,
            expected: Vec::new(),
            body: serde_json::to_value(&request).expect("lane effort retune encodes"),
        }
        .admit(&wire::Accepted {
            wire: wire::WireVersion::CURRENT,
            actor: Actor::Operator,
            epoch: self.epoch,
        })
        .expect("operator effort retune admits");
        (command, request)
    }

    fn head(&self) -> wire::ResidencyHead {
        current_residency(&self.bus, self.epoch)
            .expect("seated residency reads")
            .into_body()
    }

    /// The seat spelling the next launch would read.
    fn seated_model(&self) -> Option<String> {
        self.head()
            .lanes
            .iter()
            .find(|row| row.lane == self.lane)
            .expect("the seeded lane stays seated")
            .model
            .clone()
    }
    fn seated_effort(&self) -> Option<String> {
        self.head()
            .lanes
            .iter()
            .find(|row| row.lane == self.lane)
            .expect("the seeded lane stays seated")
            .effort
            .clone()
    }

    fn retuned_facts(&self) -> Vec<serde_json::Value> {
        self.bus
            .fact_snapshot()
            .expect("retuned journal reads")
            .facts()
            .iter()
            .filter(|fact| fact.fact.kind() == wire::FactKind::LaneRetuned)
            .map(|fact| serde_json::Value::Object(fact.fact.body().clone()))
            .collect()
    }
    fn effort_facts(&self) -> Vec<serde_json::Value> {
        self.bus
            .fact_snapshot()
            .expect("retuned journal reads")
            .facts()
            .iter()
            .filter(|fact| fact.fact.kind() == wire::FactKind::LaneEffortRetuned)
            .map(|fact| serde_json::Value::Object(fact.fact.body().clone()))
            .collect()
    }

    fn release(mut self) {
        self.seats
            .vacate(self.lane)
            .expect("retuned occupant remains supervised")
            .wait()
            .expect("fixture occupant exits");
        let _ = std::fs::remove_dir_all(&self.root);
    }

    /// Give the occupant a declared OMP launch running `effort`.
    fn declare_launch(&self, effort: &str) {
        let session = wire::LaneSession {
            lane: self.lane,
            occupant: self.occupant,
            worktree: self.root.display().to_string(),
            vendor: "omp".parse().expect("fixture vendor"),
            launch: Some(wire::body::LaneLaunchIdentity::new("sonnet", effort)),
            epoch: self.epoch,
            stream: "stdout".to_owned(),
            identity: "fixture-session".to_owned(),
            locus: wire::TapeLocus {
                member: 1,
                offset: 0,
                length: 1,
            },
        };
        rewrite_row(&self.bus, self.epoch, self.lane, |row| {
            row.session = Some(session);
        });
    }
}

/// Commit one residency row change as an unrelated writer would.
fn rewrite_row(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: CanonicalUuid,
    change: impl FnOnce(&mut wire::LaneResidency),
) {
    let current = current_residency(bus, epoch).expect("residency reads");
    let mut head = current.body().clone();
    change(
        head.lanes
            .iter_mut()
            .find(|row| row.lane == lane)
            .expect("fixture lane is seated"),
    );
    let mut write =
        write(Actor::Spice, epoch, Producer::ResidentRecognition).expect("fixture write starts");
    let fact = write
        .fact(
            wire::body::LaneRetuned {
                lane,
                from: None,
                model: None,
            },
            FactCause::Existing(current.position()),
        )
        .expect("fixture fact builds");
    write.residency_head(fact, move |_| head);
    commit(bus, write).expect("fixture row commits");
}

impl Seated {
    fn running_effort(&self) -> Option<String> {
        self.head()
            .lanes
            .iter()
            .find(|row| row.lane == self.lane)
            .and_then(|row| row.session.as_ref())
            .and_then(|session| session.launch.as_ref())
            .map(|launch| launch.effort.clone())
    }
}

fn committed_rows(outcome: Outcome<CommandAnswer>) -> Vec<wire::intent::LaneAnswerRow> {
    let Outcome::Reply(reply) = outcome else {
        panic!("a retune replies: {outcome:?}");
    };
    assert!(matches!(reply.position, ReplyPosition::Committed(_)));
    let CommandAnswer::Lane(answer) = reply.body else {
        panic!("a retune returns the authoritative lane table");
    };
    answer.lanes
}

fn refused(outcome: Outcome<CommandAnswer>) -> String {
    match outcome {
        Outcome::Refusal(refusal) => refusal
            .diagnostic()
            .expect("a lane refusal carries a diagnostic")
            .reason()
            .to_owned(),
        other => panic!("expected a refusal: {other:?}"),
    }
}

#[test]
fn retuning_a_seat_changes_the_next_launch_and_not_the_running_occupant() {
    let fixture = Seated::new("lane-retune-pin");

    let rows = committed_rows(fixture.retune(4, fixture.lane, Some("claude-opus-5-5")));

    let row = rows
        .iter()
        .find(|row| row.lane == fixture.lane)
        .expect("the retuned lane is in the answer");
    assert_eq!(row.seat_model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(
        row.occupant,
        Some(fixture.occupant),
        "a retune states a seat, so the occupant it reports is the one still running",
    );
    assert_eq!(row.disposition, "occupied");
    assert_eq!(
        fixture.seated_model().as_deref(),
        Some("claude-opus-5-5"),
        "the durable row is the launcher's only model input",
    );
    assert!(
        fixture.seats.occupied(fixture.lane),
        "retuning never touches the supervised process",
    );
    assert_eq!(
        fixture.retuned_facts(),
        vec![serde_json::json!({
            "lane": fixture.lane.to_string(),
            "model": "claude-opus-5-5",
        })],
    );

    fixture.release();
}

#[test]
fn clearing_a_seat_returns_it_to_the_configured_vendor_model() {
    let fixture = Seated::new("lane-retune-clear");
    committed_rows(fixture.retune(4, fixture.lane, Some("claude-opus-5-5")));

    let rows = committed_rows(fixture.retune(5, fixture.lane, None));

    let row = rows
        .iter()
        .find(|row| row.lane == fixture.lane)
        .expect("the cleared lane is in the answer");
    assert_eq!(
        row.seat_model, None,
        "a cleared seat names no model, so the vendor configuration governs its next launch",
    );
    assert_eq!(fixture.seated_model(), None);
    assert_eq!(
        fixture.retuned_facts().last(),
        Some(&serde_json::json!({
            "lane": fixture.lane.to_string(),
            "from": "claude-opus-5-5",
        })),
        "the clearing fact carries the spelling it replaced and no successor",
    );

    fixture.release();
}
#[test]
fn effort_retune_persists_next_launch_and_keeps_unconfirmed_runtime_separate() {
    let fixture = Seated::new("lane-effort-retune");
    let head = fixture.head();
    let revision = head.topology_revision;
    let running = head.lanes[0]
        .session
        .as_ref()
        .and_then(|session| session.launch.as_ref())
        .map(|launch| launch.effort.as_str());
    let target = if running == Some("low") { "max" } else { "low" };

    let rows = committed_rows(fixture.retune_effort(4, Some(target), revision));
    let row = rows
        .iter()
        .find(|row| row.lane == fixture.lane)
        .expect("the retuned lane is in the answer");
    assert_eq!(row.seat_effort.as_deref(), Some(target));
    assert_ne!(
        row.effort.as_deref(),
        row.seat_effort.as_deref(),
        "an occupant without a command door must not claim an immediate change",
    );
    assert_eq!(fixture.seated_effort().as_deref(), Some(target));
    assert_eq!(
        fixture.effort_facts(),
        vec![serde_json::json!({
            "lane": fixture.lane.to_string(),
            "effort": target,
        })],
    );

    let rows = committed_rows(fixture.retune_effort(5, None, revision + 1));
    let row = rows
        .iter()
        .find(|row| row.lane == fixture.lane)
        .expect("the cleared lane is in the answer");
    assert_eq!(row.seat_effort, None);
    assert_eq!(fixture.seated_effort(), None);
    assert_eq!(
        fixture.effort_facts().last(),
        Some(&serde_json::json!({
            "lane": fixture.lane.to_string(),
            "from": target,
        })),
    );
    assert_eq!(
        refused(fixture.retune_effort(6, Some("turbo"), revision + 2)),
        "lane::effort-invalid",
    );
    assert_eq!(fixture.seated_effort(), None);

    fixture.release();
}

/// The seat commits before the occupant is touched, so a residency write that
/// lands while OMP is answering neither loses the seat nor the confirmed
/// running change.
#[cfg(unix)]
#[test]
fn a_residency_write_during_a_live_retune_keeps_seat_and_running_effort() {
    use std::os::unix::net::UnixDatagram;

    let fixture = Seated::new("lane-effort-live-race");
    fixture.declare_launch("low");
    let door = crate::resident::supervisor::command::kick_path(&fixture.paths, fixture.lane);
    let _ = std::fs::remove_file(&door);
    let listener = UnixDatagram::bind(&door).expect("fixture kick door binds");
    let revision = fixture.head().topology_revision;
    let (bus, epoch, lane) = (&fixture.bus, fixture.epoch, fixture.lane);
    let (command, request) = fixture.effort_command(4, Some("max"), revision);

    let outcome = std::thread::scope(|scope| {
        scope.spawn(move || {
            let mut bytes = [0u8; 4096];
            let length = listener.recv(&mut bytes).expect("the retune kick arrives");
            let kick: serde_json::Value =
                serde_json::from_slice(&bytes[..length]).expect("kick decodes");
            assert_eq!(kick["kind"], "retune");
            assert_eq!(kick["retune"]["setting"], "effort");
            // Another writer advances residency while OMP is still answering.
            rewrite_row(bus, epoch, lane, |row| row.capabilities.push(test_uuid(10)));
            let answer = serde_json::json!({
                "request": kick["request"],
                "result": {"disposition": "applied"},
            });
            UnixDatagram::unbound()
                .expect("reply socket")
                .send_to(
                    answer.to_string().as_bytes(),
                    kick["reply"].as_str().expect("reply door"),
                )
                .expect("applied reply sends");
        });
        super::super::lane_surface::retune_effort(
            &fixture.bus,
            &fixture.paths,
            fixture.epoch,
            &fixture.seats,
            &command,
            &request,
        )
    });
    let _ = std::fs::remove_file(&door);
    let returned_position = match &outcome {
        Outcome::Reply(reply) => reply.position,
        other => panic!("a live retune replies: {other:?}"),
    };
    let replayed = super::super::command_surface::replay_command(&fixture.bus, &command)
        .expect("the committed retune has a durable receipt");
    let replayed_position = match replayed {
        Outcome::Reply(reply) => reply.position,
        other => panic!("a successful retune receipt replays: {other:?}"),
    };
    assert_eq!(
        returned_position, replayed_position,
        "the live reply names its durable command transaction, not interleaved facts",
    );
    let rows = committed_rows(outcome);

    let row = rows
        .iter()
        .find(|row| row.lane == fixture.lane)
        .expect("the retuned lane is in the answer");
    assert_eq!(row.seat_effort.as_deref(), Some("max"));
    assert_eq!(
        row.effort.as_deref(),
        Some("max"),
        "the confirmed change is running"
    );
    assert_eq!(fixture.seated_effort().as_deref(), Some("max"));
    assert_eq!(fixture.running_effort().as_deref(), Some("max"));
    assert!(
        fixture.head().lanes[0]
            .capabilities
            .contains(&test_uuid(10)),
        "the concurrent write survives"
    );
    fixture.release();
}

#[test]
fn a_retune_refuses_an_unseated_lane_a_blank_spelling_and_a_stale_topology() {
    let fixture = Seated::new("lane-retune-refusals");

    assert_eq!(
        refused(fixture.retune(4, test_uuid(40), Some("claude-opus-5-5"))),
        "lane::missing",
    );
    assert_eq!(
        refused(fixture.retune(5, fixture.lane, Some("  "))),
        "lane::model-invalid",
    );
    let stale = fixture.head().topology_revision.saturating_sub(1);
    assert_eq!(
        refused(fixture.retune_at(6, fixture.lane, Some("claude-opus-5-5"), stale)),
        "lane::topology-moved",
    );
    assert_eq!(
        fixture.seated_model(),
        None,
        "no refused retune reaches the seat",
    );
    assert!(fixture.retuned_facts().is_empty());

    fixture.release();
}
