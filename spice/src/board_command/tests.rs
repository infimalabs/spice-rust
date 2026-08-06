/// Durable board command tests, including lane lifecycle task persistence.
#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use serde::Serialize;
    use wire::{Accepted, CanonicalUuid, ResidencyHead, ResidentState};

    use super::*;

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(1);
    static NEXT_COMMAND: AtomicU64 = AtomicU64::new(100);

    #[derive(Clone, Debug)]
    struct Seat {
        lane: CanonicalUuid,
        occupant: CanonicalUuid,
    }

    fn uuid(value: u64) -> CanonicalUuid {
        format!("018f82d4-6f59-7d06-92e3-{value:012x}")
            .parse()
            .expect("fixture UUID is canonical")
    }

    fn fixture(name: &str, lane_count: usize) -> (PathBuf, Bus, CanonicalUuid, Vec<Seat>) {
        let number = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "spice-board-command-{name}-{}-{number}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("fixture directory exists");
        let path = root.join("authority.sqlite3");
        let journal = journal::Journal::open(&path).expect("fixture journal opens");
        let bus = Bus::start(journal).expect("fixture bus starts");
        let epoch = uuid(number + 1);
        let seats = (0..lane_count)
            .map(|index| Seat {
                lane: uuid(number * 100 + index as u64 + 10),
                occupant: uuid(number * 100 + index as u64 + 30),
            })
            .collect::<Vec<_>>();
        let lanes = seats
            .iter()
            .enumerate()
            .map(|(index, seat)| wire::LaneResidency {
                lane: seat.lane,
                lineage_id: seat.lane,
                name: None,
                worktree: "/fixture".to_owned(),
                life: "drive".to_owned(),
                vendor: "claude".parse().expect("fixture vendor"),
                occupant: Some(seat.occupant),
                lineage: vec![seat.occupant],
                capabilities: vec![uuid(number * 100 + index as u64 + 50)],
                session: None,
                disposition: "occupied".to_owned(),
            })
            .collect::<Vec<_>>();
        let origin = format!("resident:{epoch}")
            .parse()
            .expect("resident fixture origin");
        let at = Instant::new(jiff::Timestamp::now()).expect("current fixture instant");
        let mut write = JournalWrite::new(Actor::Spice, origin, at, Producer::CommandDoor);
        let seated = write
            .fact(body::UpSeated { epoch }, FactCause::Root)
            .expect("resident seating fact builds");
        write.residency_head(seated, move |_| ResidencyHead {
            epoch,
            state: ResidentState::Seated,
            lanes,
            subscriptions: Vec::new(),
        });
        match bus.commit(write).expect("fixture seating reaches writer") {
            CommitOutcome::Committed(_) => {}
            outcome => panic!("fixture seating did not commit: {outcome:?}"),
        }
        (root, bus, epoch, seats)
    }

    fn issue<B: Serialize>(
        bus: &Bus,
        epoch: CanonicalUuid,
        actor: Actor,
        kind: CommandKind,
        expected: Vec<Witness>,
        body: &B,
    ) -> Outcome<CommandAnswer> {
        let command = Command {
            v: WireVersion::CURRENT,
            id: uuid(NEXT_COMMAND.fetch_add(1, Ordering::Relaxed)),
            epoch,
            kind,
            expected,
            body: serde_json::to_value(body).expect("fixture command body encodes"),
        }
        .admit(&Accepted {
            wire: WireVersion::CURRENT,
            actor,
            epoch,
        })
        .expect("fixture command admits");
        dispatch(bus, &command).expect("board command owns fixture kind")
    }

    fn assert_committed(outcome: Outcome<CommandAnswer>) -> CommandAnswer {
        let Outcome::Reply(Reply {
            position: ReplyPosition::Committed(_),
            body,
        }) = outcome
        else {
            panic!("command did not commit: {outcome:?}");
        };
        body
    }

    fn board(bus: &Bus, epoch: CanonicalUuid, actor: Actor) -> BoardAnswer {
        let outcome = issue(
            bus,
            epoch,
            actor,
            CommandKind::BoardRead,
            Vec::new(),
            &BoardRead { family: None },
        );
        let Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(position),
            body: CommandAnswer::Board(answer),
        }) = outcome
        else {
            panic!("board read did not return a snapshot: {outcome:?}");
        };
        assert_eq!(position, answer.cursor);
        *answer
    }

    fn add(
        bus: &Bus,
        epoch: CanonicalUuid,
        actor: Actor,
        task: &str,
        title: &str,
        flow: Option<&str>,
    ) -> TaskHandle {
        add_project(bus, epoch, actor, task, title, "board.test", flow)
    }

    fn add_project(
        bus: &Bus,
        epoch: CanonicalUuid,
        actor: Actor,
        task: &str,
        title: &str,
        project: &str,
        flow: Option<&str>,
    ) -> TaskHandle {
        add_project_with_acceptance(bus, epoch, actor, task, title, project, flow, &[])
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "task fixtures retain the command's authored fields"
    )]
    fn add_project_with_acceptance(
        bus: &Bus,
        epoch: CanonicalUuid,
        actor: Actor,
        task: &str,
        title: &str,
        project: &str,
        flow: Option<&str>,
        acceptance: &[&str],
    ) -> TaskHandle {
        let task = task.parse::<TaskHandle>().expect("fixture task handle");
        assert_committed(issue(
            bus,
            epoch,
            actor,
            CommandKind::TaskAdd,
            vec![Witness::Absence { task: task.clone() }],
            &TaskAdd {
                title: title.to_owned(),
                project: Some(project.to_owned()),
                description: None,
                acceptance: acceptance.iter().map(|item| (*item).to_owned()).collect(),
                origin: None,
                flow: flow.map(str::to_owned),
                depends: Vec::new(),
            },
        ));
        task
    }

    fn at<'a>(answer: &'a BoardAnswer, task: &TaskHandle) -> &'a BoardTask {
        task_heads(answer, task).expect("fixture task is visible")
    }

    fn residency(answer: &BoardAnswer, seat: &Seat) -> Witness {
        residency_witness(answer, seat.lane, seat.occupant)
    }

    fn claim_expected(answer: &BoardAnswer, task: &TaskHandle, seat: &Seat) -> Vec<Witness> {
        let heads = at(answer, task);
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
            Witness::Eligibility {
                rule: "next".parse().expect("fixture allocator rule"),
            },
            residency(answer, seat),
        ]
    }

    fn clean(root: &Path, bus: Bus) {
        bus.verify().expect("fixture authority verifies");
        bus.shutdown().expect("fixture bus stops");
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn native_task_handles_use_the_predecessor_stamp_alphabet() {
        const PREDECESSOR_ALPHABET: &[u8; 52] =
            b"0123456789BCDFGHJKLMNPQRSTVWXYZbcdfghjklmnpqrstvwxyz";

        for (value, expected) in PREDECESSOR_ALPHABET.iter().copied().enumerate() {
            let handle = mint_handle_at(Some("task.identity"), "ignored", value as i64);
            let (_, stamp) = handle
                .as_str()
                .split_once('-')
                .expect("native handle carries a stem and stamp");
            assert_eq!(stamp.len(), steer::Key::WIDTH);
            assert!(
                stamp
                    .bytes()
                    .all(|byte| PREDECESSOR_ALPHABET.contains(&byte)),
                "{stamp:?} is outside the predecessor task alphabet"
            );
            assert_eq!(stamp.as_bytes()[steer::Key::WIDTH - 1], expected);
        }
    }

    #[test]
    fn task_status_counts_one_snapshot_and_task_add_retains_authored_context() {
        let (root, bus, epoch, _) = fixture("task-bootstrap", 1);
        let task = "CLI-context01"
            .parse::<TaskHandle>()
            .expect("fixture task handle");
        let origin: wire::Origin = "task:CLI-parent".parse().expect("fixture origin");
        assert_committed(issue(
            &bus,
            epoch,
            Actor::Operator,
            CommandKind::TaskAdd,
            vec![Witness::Absence { task: task.clone() }],
            &TaskAdd {
                title: "Carry native bootstrap context".to_owned(),
                project: Some("task.cli".to_owned()),
                description: Some("The agent needs the complete handoff".to_owned()),
                acceptance: vec![
                    "Status reports board counts".to_owned(),
                    "Creation provenance survives".to_owned(),
                ],
                origin: Some(origin.clone()),
                flow: Some("todo".to_owned()),
                depends: Vec::new(),
            },
        ));
        add(
            &bus,
            epoch,
            Actor::Operator,
            "CLI-review001",
            "Review native bootstrap",
            Some("review"),
        );

        let answer = board(&bus, epoch, Actor::Operator);
        let statement = &at(&answer, &task).statement.body;
        assert_eq!(
            statement.description.as_deref(),
            Some("The agent needs the complete handoff")
        );
        assert_eq!(
            statement.acceptance,
            [
                "Status reports board counts",
                "Creation provenance survives"
            ]
        );
        assert_eq!(statement.provenance, origin);

        let status = task_status(&answer).expect("validated board yields status");
        for expected in [
            "claim -",
            "actor operator",
            "filter ( project:board or project:rim or project:serve or project:task )",
            "public task project depth 2..3 dotted segments",
            "active 0",
            "ready 1",
            "review 1",
            "blocked 0",
            "waiting 0",
            "stale 0",
            "oops 0",
        ] {
            assert!(status.lines().any(|line| line == expected), "{status}");
        }
        clean(&root, bus);
    }

    #[test]
    fn task_list_preserves_scope_status_project_and_newest_limit_semantics() {
        let (root, bus, epoch, _) = fixture("task-list", 0);
        let (answer, first, descendant, waiting) = task_list_fixture(&bus, epoch);
        assert_task_list_queries(&answer, &first, &descendant, &waiting);
        clean(&root, bus);
    }

    fn task_list_fixture(
        bus: &Bus,
        epoch: CanonicalUuid,
    ) -> (BoardAnswer, TaskHandle, TaskHandle, TaskHandle) {
        let first = add_project(
            bus,
            epoch,
            Actor::Operator,
            "CLI-list0001",
            "First task route",
            "task.cli",
            Some("todo"),
        );
        let descendant = add_project(
            bus,
            epoch,
            Actor::Operator,
            "CLI-list0002",
            "Descendant task route",
            "task.cli.deep",
            Some("todo"),
        );
        let waiting = add_project(
            bus,
            epoch,
            Actor::Operator,
            "CLI-list0003",
            "Deferred serve route",
            "serve.ops",
            Some("todo"),
        );
        let completed = add_project(
            bus,
            epoch,
            Actor::Operator,
            "CLI-list0004",
            "Completed board route",
            "board.test",
            Some("todo"),
        );
        let mut answer = board(bus, epoch, Actor::Operator);
        answer
            .tasks
            .iter_mut()
            .find(|task| task.statement.body.handle == first)
            .expect("first row")
            .statement
            .body
            .tags
            .push("priority:medium".to_owned());
        answer
            .tasks
            .iter_mut()
            .find(|task| task.statement.body.handle == waiting)
            .expect("waiting row")
            .workflow
            .body
            .deferral = Some(fixture_instant("fixture deferral"));
        let completed_row = answer
            .tasks
            .iter_mut()
            .find(|task| task.statement.body.handle == completed)
            .expect("completed row");
        completed_row.workflow.body.status = "completed".to_owned();
        completed_row.workflow.body.completion = Some(fixture_instant("fixture completion"));
        (answer, first, descendant, waiting)
    }

    fn assert_task_list_queries(
        answer: &BoardAnswer,
        first: &TaskHandle,
        descendant: &TaskHandle,
        waiting: &TaskHandle,
    ) {
        let default = task_list(
            answer,
            &TaskListOptions {
                all: false,
                project: None,
                limit: None,
                status: None,
            },
        );
        assert!(default.starts_with(concat!(
            "scope actor-route filter ",
            "( project:board or project:rim or project:serve or project:task )\n"
        )));
        assert!(default.contains("CLI-list0001 [todo] P:M task.cli First task route"));
        assert!(default.contains("CLI-list0002 [todo] task.cli.deep Descendant task route"));
        assert!(!default.contains("CLI-list0003"), "{default}");
        assert!(!default.contains("CLI-list0004"), "{default}");

        let project = task_list(
            answer,
            &TaskListOptions {
                all: false,
                project: Some("task.cli".to_owned()),
                limit: None,
                status: Some(TaskListStatus::Pending),
            },
        );
        assert!(project.starts_with("scope explicit-project project:task.cli\n"));
        assert!(project.contains(first.as_str()));
        assert!(project.contains(descendant.as_str()));
        assert!(!project.contains(waiting.as_str()));

        let waiting_only = task_list(
            answer,
            &TaskListOptions {
                all: false,
                project: None,
                limit: None,
                status: Some(TaskListStatus::Waiting),
            },
        );
        assert!(waiting_only.contains(waiting.as_str()));
        assert!(!waiting_only.contains(first.as_str()));

        let newest_global = task_list(
            answer,
            &TaskListOptions {
                all: true,
                project: None,
                limit: Some(1),
                status: None,
            },
        );
        assert_eq!(
            newest_global,
            "scope global --all\nCLI-list0004 [done] board.test Completed board route"
        );
        let deleted = task_list(
            answer,
            &TaskListOptions {
                all: true,
                project: None,
                limit: None,
                status: Some(TaskListStatus::Deleted),
            },
        );
        assert_eq!(deleted, "scope global --all\nno tasks in global scope");
    }

    fn fixture_instant(context: &str) -> Instant {
        "2030-04-12T18:44:09.271Z"
            .parse()
            .unwrap_or_else(|error| panic!("{context}: {error}"))
    }

    /// A held task is named on the claim line and counted as active.
    ///
    /// The snapshot above reports every claim field at rest, where a wrong
    /// predicate and a right one agree: with nothing claimed, `claim -` and
    /// `active 0` are what any filter produces. Bootstrap step three exists to
    /// tell an agent which handle it already holds, so the reading that matters
    /// is the one taken while a claim is live, and it has to distinguish the
    /// principal asking from the other lanes holding rows of their own.
    #[test]
    fn task_status_names_the_claim_the_asking_principal_holds() {
        let (root, bus, epoch, seats) = fixture("task-bootstrap-claim", 2);
        let held = "CLI-held00001"
            .parse::<TaskHandle>()
            .expect("fixture held handle");
        let lane = Actor::Lane(seats[0].lane);
        add(
            &bus,
            epoch,
            Actor::Operator,
            "CLI-held00001",
            "Hold one native bootstrap row",
            Some("todo"),
        );
        let available = board(&bus, epoch, lane.clone());
        assert_committed(issue(
            &bus,
            epoch,
            lane.clone(),
            CommandKind::ClaimWin,
            claim_expected(&available, &held, &seats[0]),
            &ClaimWin {
                rule: "next".parse().expect("allocator rule"),
            },
        ));

        let holder = task_status(&board(&bus, epoch, lane)).expect("holder reads status");
        assert!(
            holder.lines().any(|line| line == "claim CLI-held00001"),
            "{holder}"
        );
        assert!(holder.lines().any(|line| line == "active 1"), "{holder}");

        // A peer's claim is still active work on the board, but it is not this
        // principal's, so the same row has to count without being named.
        let other = task_status(&board(&bus, epoch, Actor::Lane(seats[1].lane)))
            .expect("peer reads status");
        assert!(other.lines().any(|line| line == "claim -"), "{other}");
        assert!(other.lines().any(|line| line == "active 1"), "{other}");
        clean(&root, bus);
    }

    #[test]
    fn current_board_edits_are_guarded_deterministic_and_render_form_invariant() {
        let (root, bus, epoch, seats) = fixture("edits", 1);
        let initial = board(&bus, epoch, Actor::Operator);
        assert_eq!(
            actor_seat(&initial).expect("sole operator seat"),
            (seats[0].lane, seats[0].occupant,)
        );
        let (alpha, beta, oops) = seed_edit_tasks(&bus, epoch);
        exercise_deferral_staleness(&bus, epoch, &alpha);
        add_dependency(&bus, epoch, &alpha, &beta);
        let graph = assert_graph_handout_and_ledger(&bus, epoch, &alpha);
        assert_render_form_parity(&graph);
        assert_oops_note_exists(&bus, &oops);
        clean(&root, bus);
    }

    #[test]
    fn capture_claims_one_exact_eligible_non_allocator_task() {
        let (root, bus, epoch, seats) = fixture("capture", 1);
        let lane = Actor::Lane(seats[0].lane);
        let alpha = add(
            &bus,
            epoch,
            Actor::Operator,
            "CAP-alpha001",
            "Allocator winner",
            Some("todo"),
        );
        let beta = add(
            &bus,
            epoch,
            Actor::Operator,
            "CAP-beta0001",
            "Exact capture target",
            Some("todo"),
        );
        let available = board(&bus, epoch, lane.clone());
        assert_eq!(available.active_public_tasks().count(), 2);
        let entries = actor_entries(&available)
            .expect("capture board entries are valid");
        assert_eq!(
            board::select(&entries, &lane)
                .expect("allocator winner")
                .entry()
                .statement()
                .handle,
            alpha,
        );
        let heads = at(&available, &beta);
        let rule: Name = "capture".parse().expect("capture rule");
        assert_committed(issue(
            &bus,
            epoch,
            lane.clone(),
            CommandKind::ClaimWin,
            vec![
                statement_witness(heads),
                workflow_witness(heads),
                claim_witness(heads),
                Witness::Eligibility { rule: rule.clone() },
                residency(&available, &seats[0]),
            ],
            &ClaimWin { rule },
        ));

        let claimed = board(&bus, epoch, lane);
        assert!(at(&claimed, &alpha).claim.is_none());
        assert_eq!(
            at(&claimed, &beta)
                .claim
                .as_ref()
                .expect("capture claim")
                .body
                .disposition,
            ClaimDisposition::Active,
        );
        clean(&root, bus);
    }

    #[test]
    fn claims_warn_on_shared_acceptance_surface_not_shared_project_stem() {
        let (root, bus, epoch, seats) = fixture("claim-overlap", 3);
        let first = add_project_with_acceptance(
            &bus,
            epoch,
            Actor::Operator,
            "MOSAIC-1kKQTw1V",
            "Canvas lacks the mosaic lane interface",
            "board.alpha",
            Some("todo"),
            &["The watchband mosaic uses per-lane cards and stays stable"],
        );
        let overlapping = add_project_with_acceptance(
            &bus,
            epoch,
            Actor::Operator,
            "FIDELIT-1kKRSfyV",
            "Rebuild the lane watchband and message mosaic",
            "rim.beta",
            Some("todo"),
            &["The mosaic watchband keeps focused-agent state visible"],
        );
        let same_stem = add_project_with_acceptance(
            &bus,
            epoch,
            Actor::Operator,
            "MOSAIC-1kKQTw1W",
            "Audit unrelated board output",
            "board.gamma",
            Some("todo"),
            &["Export signed ledger receipts for audit and keep output stable"],
        );

        let first_answer = capture_for(&bus, epoch, &seats[0], &first);
        assert!(first_answer.warnings.is_empty());

        let overlap_answer = capture_for(&bus, epoch, &seats[1], &overlapping);
        assert_eq!(
            overlap_answer.warnings,
            [wire::intent::ClaimWarning {
                task: first,
                lane: seats[0].lane,
                surface: vec!["mosaic".to_owned(), "watchband".to_owned()],
            }]
        );

        let same_stem_answer = capture_for(&bus, epoch, &seats[2], &same_stem);
        assert!(same_stem_answer.warnings.is_empty());
        clean(&root, bus);
    }

    fn capture_for(
        bus: &Bus,
        epoch: CanonicalUuid,
        seat: &Seat,
        task: &TaskHandle,
    ) -> wire::intent::ClaimAnswer {
        let lane = Actor::Lane(seat.lane);
        let available = board(bus, epoch, lane.clone());
        let heads = at(&available, task);
        let rule: Name = "capture".parse().expect("capture rule");
        let answer = assert_committed(issue(
            bus,
            epoch,
            lane,
            CommandKind::ClaimWin,
            vec![
                statement_witness(heads),
                workflow_witness(heads),
                claim_witness(heads),
                Witness::Eligibility { rule: rule.clone() },
                residency(&available, seat),
            ],
            &ClaimWin { rule },
        ));
        let CommandAnswer::Claim(answer) = answer else {
            panic!("claim win returned another answer shape: {answer:?}");
        };
        answer
    }

    #[test]
    fn stealing_takes_a_live_claim_at_once_where_capture_is_refused() {
        let (root, bus, epoch, seats) = fixture("steal", 2);
        let holder = Actor::Lane(seats[0].lane);
        let rival = Actor::Lane(seats[1].lane);
        let task = add(
            &bus,
            epoch,
            Actor::Operator,
            "STEAL-live01",
            "Held under a live lease",
            Some("todo"),
        );

        let available = board(&bus, epoch, holder.clone());
        let heads = at(&available, &task);
        let capture: Name = "capture".parse().expect("capture rule");
        assert_committed(issue(
            &bus,
            epoch,
            holder.clone(),
            CommandKind::ClaimWin,
            vec![
                statement_witness(heads),
                workflow_witness(heads),
                claim_witness(heads),
                Witness::Eligibility {
                    rule: capture.clone(),
                },
                residency(&available, &seats[0]),
            ],
            &ClaimWin { rule: capture },
        ));

        let held = board(&bus, epoch, rival.clone());
        let heads = at(&held, &task);
        let witnesses = |rule: &Name| {
            vec![
                statement_witness(heads),
                workflow_witness(heads),
                claim_witness(heads),
                Witness::Eligibility { rule: rule.clone() },
                residency(&held, &seats[1]),
            ]
        };

        // Same board, same rival, same witnesses: only the rule differs, so a
        // steal that behaved like a capture would be refused right here.
        let capture: Name = "capture".parse().expect("capture rule");
        let declined = issue(
            &bus,
            epoch,
            rival.clone(),
            CommandKind::ClaimWin,
            witnesses(&capture),
            &ClaimWin { rule: capture },
        );
        assert!(
            matches!(
                declined,
                Outcome::Refusal(Refusal { ref reason, .. })
                    if reason == "claim::capture-ineligible"
            ),
            "capture took a held claim: {declined:?}",
        );

        let steal: Name = "steal".parse().expect("steal rule");
        assert_committed(issue(
            &bus,
            epoch,
            rival.clone(),
            CommandKind::ClaimWin,
            witnesses(&steal),
            &ClaimWin { rule: steal },
        ));

        // The lease the holder still had did not delay the transfer, and the
        // ClaimWon naming the new holder is what tells the old one.
        let after = board(&bus, epoch, rival.clone());
        let taken = at(&after, &task).claim.as_ref().expect("stolen claim");
        assert_eq!(taken.body.actor, rival);
        assert_eq!(taken.body.disposition, ClaimDisposition::Active);
        assert_ne!(taken.body.actor, holder);
        clean(&root, bus);
    }

    #[test]
    fn reclaim_renews_only_the_lane_owned_claim_and_survives_restart() {
        let (root, bus, epoch, seats) = fixture("reclaim", 1);
        let lane = Actor::Lane(seats[0].lane);
        let held_task = add(
            &bus,
            epoch,
            Actor::Operator,
            "RECLAIM-held1",
            "Held recovery work",
            Some("todo"),
        );
        let available_task = add(
            &bus,
            epoch,
            Actor::Operator,
            "RECLAIM-next1",
            "Next allocator work",
            Some("todo"),
        );
        let available = board(&bus, epoch, lane.clone());
        assert_committed(issue(
            &bus,
            epoch,
            lane.clone(),
            CommandKind::ClaimWin,
            claim_expected(&available, &held_task, &seats[0]),
            &ClaimWin {
                rule: "next".parse().expect("allocator rule"),
            },
        ));
        let held = board(&bus, epoch, lane.clone());
        let before = at(&held, &held_task).clone();
        let claim = before.claim.as_ref().expect("active claim");
        let lease = before.lease.as_ref().expect("active lease");
        let expected = vec![
            Witness::Claim {
                task: held_task.clone(),
                head: Some(claim.position),
            },
            Witness::Lease {
                task: held_task.clone(),
                head: lease.position,
            },
            residency(&held, &seats[0]),
        ];
        assert_committed(issue(
            &bus,
            epoch,
            lane.clone(),
            CommandKind::ClaimRenew,
            expected.clone(),
            &ClaimRenew {
                task: held_task.clone(),
                lease_seconds: 30 * 60,
            },
        ));

        let renewed = board(&bus, epoch, lane.clone());
        let after = at(&renewed, &held_task);
        assert_eq!(after.statement, before.statement);
        assert_eq!(after.workflow, before.workflow);
        assert_eq!(after.claim, before.claim);
        assert_ne!(
            after.lease.as_ref().expect("renewed lease").position,
            lease.position
        );
        assert_eq!(
            after.lease.as_ref().expect("renewed lease").body.maximum,
            lease.body.maximum,
        );
        assert!(after.lease.as_ref().expect("renewed lease").body.expiry > lease.body.expiry);
        assert!(
            after.lease.as_ref().expect("renewed lease").body.expiry
                <= after.lease.as_ref().expect("renewed lease").body.maximum
        );
        assert!(matches!(
            issue(
                &bus,
                epoch,
                lane.clone(),
                CommandKind::ClaimRenew,
                expected,
                &ClaimRenew {
                    task: held_task.clone(),
                    lease_seconds: 30 * 60,
                },
            ),
            Outcome::Refusal(Refusal { ref reason, .. }) if reason == "board::witness-moved"
        ));
        assert!(bus
            .fact_snapshot()
            .expect("reclaim facts read")
            .facts()
            .iter()
            .any(|fact| fact.fact.kind() == wire::FactKind::ClaimRenewed));

        bus.verify().expect("reclaimed authority verifies");
        bus.shutdown().expect("reclaim fixture bus stops");
        let reopened = journal::Journal::open(root.join("authority.sqlite3"))
            .expect("reclaim journal reopens");
        let reopened = Bus::start(reopened).expect("reclaim bus restarts");
        let after_restart = board(&reopened, epoch, lane.clone());
        assert_eq!(at(&after_restart, &held_task), after);
        let entries = actor_entries(&after_restart).expect("restarted board validates");
        assert_eq!(
            board::select(&entries, &lane)
                .expect("allocator still has unclaimed work")
                .entry()
                .statement()
                .handle,
            available_task,
            "the renewed claim remains excluded from allocation after restart",
        );
        clean(&root, reopened);
    }

    // Lane lifecycle, review, and document tests share this fixture harness.
    // Their execution seam lives separately so each source stays reviewable.
    include!("tests/resolution.rs");

    // The hidden-visibility tests and the two helpers only they use. They share
    // this module's fixture harness, so the seam is the law under test rather
    // than the scaffolding.
    include!("tests/hidden.rs");
}
