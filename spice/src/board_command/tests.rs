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
                acceptance: Vec::new(),
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
        let entries = entries(&available)
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
    fn board_read_retains_hidden_evidence_while_next_and_presentation_stay_public() {
        let (root, bus, epoch, seats) = fixture("hidden-visibility", 1);
        let lane = Actor::Lane(seats[0].lane);
        let hidden = add_project(
            &bus,
            epoch,
            Actor::Operator,
            "HIDDEN-oops1",
            "secret allocator failure details",
            ".oops.correctness",
            Some("todo"),
        );
        let public = add_project(
            &bus,
            epoch,
            Actor::Operator,
            "PUBLIC-work1",
            "Visible allocator work",
            "board.visibility",
            Some("todo"),
        );

        let available = board(&bus, epoch, lane.clone());
        let hidden_heads = at(&available, &hidden);
        assert_eq!(hidden_heads.visibility.hidden_stem(), Some("oops"));
        assert_eq!(hidden_heads.statement.body.project, ".oops.correctness");
        assert_eq!(hidden_heads.presentation_title(), "Hidden task");
        assert_eq!(at(&available, &public).presentation_title(), "Visible allocator work");
        let json = serde_json::to_value(&available).expect("board answer encodes");
        let hidden_json = json["tasks"]
            .as_array()
            .expect("task rows")
            .iter()
            .find(|task| task["statement"]["body"]["handle"] == hidden.to_string())
            .expect("hidden JSON row");
        assert_eq!(
            hidden_json["visibility"],
            serde_json::json!({"kind": "hidden", "stem": "oops"})
        );
        assert_eq!(hidden_json["presentation_title"], "Hidden task");
        let rendered = Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(available.cursor),
            body: CommandAnswer::Board(Box::new(available.clone())),
        });
        for form in [crate::render::Form::Text, crate::render::Form::Json] {
            let document = crate::render::render(&rendered, form)
                .expect("board presentation renders")
                .document()
                .to_vec();
            assert!(
                String::from_utf8(document)
                    .expect("presentation is UTF-8")
                    .contains("Hidden task"),
                "{form:?} presentation"
            );
        }

        let rule: Name = "next".parse().expect("allocator rule");
        assert_committed(issue(
            &bus,
            epoch,
            lane.clone(),
            CommandKind::ClaimWin,
            claim_expected(&available, &public, &seats[0]),
            &ClaimWin { rule },
        ));
        let claimed = board(&bus, epoch, lane);
        assert!(at(&claimed, &hidden).claim.is_none());
        assert!(at(&claimed, &public).claim.is_some());
        clean(&root, bus);
    }

    fn seed_edit_tasks(
        bus: &Bus,
        epoch: CanonicalUuid,
    ) -> (TaskHandle, TaskHandle, TaskHandle) {
        let alpha = add(
            bus,
            epoch,
            Actor::Operator,
            "BOARD-alpha001",
            "Alpha work",
            Some("todo"),
        );
        let beta = add(
            bus,
            epoch,
            Actor::Operator,
            "BOARD-beta0001",
            "Beta prerequisite",
            Some("todo"),
        );
        let oops: TaskHandle = "OOPS-oops0001".parse().expect("oops handle");
        assert_committed(issue(
            bus,
            epoch,
            Actor::Operator,
            CommandKind::TaskAdd,
            vec![Witness::Absence { task: oops.clone() }],
            &TaskAdd {
                title: "A command was unexpectedly awkward".to_owned(),
                project: Some("board.oops".to_owned()),
                description: Some("captured once as durable work".to_owned()),
                acceptance: Vec::new(),
                flow: None,
                depends: Vec::new(),
            },
        ));
        assert_committed(issue(
            bus,
            epoch,
            Actor::Operator,
            CommandKind::TaskNote,
            vec![Witness::Existence {
                task: alpha.clone(),
            }],
            &TaskNote {
                task: alpha.clone(),
                note: "one current observation".to_owned(),
            },
        ));
        (alpha, beta, oops)
    }

    fn exercise_deferral_staleness(bus: &Bus, epoch: CanonicalUuid, alpha: &TaskHandle) {
        let before_defer = board(bus, epoch, Actor::Operator);
        let alpha_before = at(&before_defer, alpha).clone();
        let until = instant_after(current_instant().expect("clock"), 300).expect("future instant");
        assert_committed(issue(
            bus,
            epoch,
            Actor::Operator,
            CommandKind::TaskDefer,
            vec![
                workflow_witness(&alpha_before),
                deferral_witness(&alpha_before),
            ],
            &TaskDefer {
                task: alpha.clone(),
                wait: until,
            },
        ));
        let stale = issue(
            bus,
            epoch,
            Actor::Operator,
            CommandKind::TaskWake,
            vec![
                workflow_witness(&alpha_before),
                deferral_witness(&alpha_before),
            ],
            &TaskRef {
                task: alpha.clone(),
            },
        );
        assert!(matches!(
            stale,
            Outcome::Refusal(Refusal { ref reason, .. }) if reason == "board::witness-moved"
        ));
        let deferred = board(bus, epoch, Actor::Operator);
        assert_eq!(at(&deferred, alpha).workflow.body.deferral, Some(until));
        assert_committed(issue(
            bus,
            epoch,
            Actor::Operator,
            CommandKind::TaskWake,
            vec![
                workflow_witness(at(&deferred, alpha)),
                deferral_witness(at(&deferred, alpha)),
            ],
            &TaskRef {
                task: alpha.clone(),
            },
        ));
    }

    fn add_dependency(
        bus: &Bus,
        epoch: CanonicalUuid,
        alpha: &TaskHandle,
        beta: &TaskHandle,
    ) {
        let before_edge = board(bus, epoch, Actor::Operator);
        let alpha_heads = at(&before_edge, alpha);
        let beta_heads = at(&before_edge, beta);
        assert_committed(issue(
            bus,
            epoch,
            Actor::Operator,
            CommandKind::TaskDepends,
            vec![
                statement_witness(alpha_heads),
                workflow_witness(alpha_heads),
                statement_witness(beta_heads),
                workflow_witness(beta_heads),
                Witness::Edge {
                    task: alpha.clone(),
                    after: beta.clone(),
                },
            ],
            &TaskDepends {
                task: alpha.clone(),
                after: vec![beta.clone()],
            },
        ));
    }

    fn assert_graph_handout_and_ledger(
        bus: &Bus,
        epoch: CanonicalUuid,
        alpha: &TaskHandle,
    ) -> Outcome<CommandAnswer> {
        let graph_one = graph(bus, epoch);
        let graph_two = graph(bus, epoch);
        assert_eq!(graph_one, graph_two);
        let Outcome::Reply(Reply {
            position: graph_position @ ReplyPosition::Snapshot(_),
            body: CommandAnswer::Text(ref graph),
        }) = graph_one
        else {
            panic!("graph was not a text snapshot: {graph_one:?}");
        };
        assert!(graph.text.contains("BOARD-alpha001 -> BOARD-beta0001"));
        let handout = issue(
            bus,
            epoch,
            Actor::Operator,
            CommandKind::BoardHandout,
            Vec::new(),
            &BoardHandout {
                output: "-".to_owned(),
                at: None,
            },
        );
        let Outcome::Reply(Reply {
            position: handout_position,
            body: CommandAnswer::Text(ref document),
        }) = handout
        else {
            panic!("handout was not a text snapshot: {handout:?}");
        };
        assert_eq!(handout_position, graph_position);
        assert!(document.text.contains(&graph.text));
        assert_ledger_contains(bus, epoch, alpha, "# Alpha work");
        graph_one
    }

    fn graph(bus: &Bus, epoch: CanonicalUuid) -> Outcome<CommandAnswer> {
        issue(
            bus,
            epoch,
            Actor::Operator,
            CommandKind::BoardGraph,
            Vec::new(),
            &BoardGraph {
                view: "dependencies".to_owned(),
                at: None,
            },
        )
    }

    fn assert_ledger_contains(
        bus: &Bus,
        epoch: CanonicalUuid,
        task: &TaskHandle,
        expected: &str,
    ) {
        let ledger = issue(
            bus,
            epoch,
            Actor::Operator,
            CommandKind::DocLedger,
            Vec::new(),
            &DocLedger { task: task.clone() },
        );
        assert!(matches!(
            ledger,
            Outcome::Reply(Reply {
                body: CommandAnswer::Text(TextAnswer { ref text }),
                ..
            }) if text.contains(expected)
        ));
    }

    fn assert_render_form_parity(graph: &Outcome<CommandAnswer>) {
        let text = crate::render::render(graph, crate::render::Form::Text)
            .expect("graph text renders");
        let json = crate::render::render(graph, crate::render::Form::Json)
            .expect("graph JSON renders");
        let text_body = wire::intent::AnswerShape::Text
            .decode(text.document())
            .expect("text body is the selected command answer");
        let json_outcome: Outcome<serde_json::Value> =
            wire::decode(json.document()).expect("JSON is the typed outcome");
        let Outcome::Reply(Reply { body, .. }) = graph else {
            unreachable!("graph reply matched above")
        };
        assert_eq!(&text_body, body);
        assert_eq!(
            serde_json::to_value(json_outcome).expect("decoded JSON outcome re-encodes"),
            serde_json::to_value(graph).expect("source outcome encodes"),
        );
    }

    fn assert_oops_note_exists(bus: &Bus, oops: &TaskHandle) {
        let facts = bus.fact_snapshot().expect("fixture facts read");
        assert!(facts.facts().iter().any(|fact| {
            fact.fact.kind() == wire::FactKind::TaskNoted
                && fact
                    .fact
                    .body()
                    .get("task")
                    .and_then(serde_json::Value::as_str)
                    == Some(oops.as_str())
        }));
    }

    #[test]
    fn lane_claim_free_done_and_review_finding_survive_restart() {
        let (root, bus, epoch, seats) = fixture("workflow", 2);
        let lane_one = Actor::Lane(seats[0].lane);
        let lane_two = Actor::Lane(seats[1].lane);
        assert_lane_resolution(&bus, epoch, &seats, &lane_one);
        let task = add(
            &bus,
            epoch,
            Actor::Operator,
            "FLOW-flow0001",
            "Complete and review",
            None,
        );
        advance_task_to_review(&bus, epoch, &seats[0], &lane_one, &task);
        complete_review_with_finding(&bus, epoch, &seats[1], &lane_two, &task);
        let settled = assert_review_settled(&bus, epoch, &task);
        assert_restart_preserves(&root, bus, epoch, &settled);
    }

    fn assert_lane_resolution(
        bus: &Bus,
        epoch: CanonicalUuid,
        seats: &[Seat],
        lane_one: &Actor,
    ) {
        let operator = board(bus, epoch, Actor::Operator);
        assert!(matches!(
            actor_seat(&operator),
            Err(Outcome::Refusal(Refusal { ref reason, .. }))
                if reason == "claim::operator-lane-ambiguous"
        ));
        let lane_board = board(bus, epoch, lane_one.clone());
        assert_eq!(
            actor_seat(&lane_board).expect("lane resolves its own seat"),
            (seats[0].lane, seats[0].occupant)
        );
    }

    fn advance_task_to_review(
        bus: &Bus,
        epoch: CanonicalUuid,
        seat: &Seat,
        lane: &Actor,
        task: &TaskHandle,
    ) {
        let available = board(bus, epoch, lane.clone());
        assert_committed(issue(
            bus,
            epoch,
            lane.clone(),
            CommandKind::ClaimWin,
            claim_expected(&available, task, seat),
            &ClaimWin {
                rule: "next".parse().expect("allocator rule"),
            },
        ));
        let held = board(bus, epoch, lane.clone());
        let held_heads = at(&held, task);
        assert_committed(issue(
            bus,
            epoch,
            lane.clone(),
            CommandKind::ClaimFree,
            vec![
                claim_witness(held_heads),
                Witness::Lease {
                    task: task.clone(),
                    head: held_heads.lease.as_ref().expect("lease head").position,
                },
                residency(&held, seat),
            ],
            &TaskRef { task: task.clone() },
        ));
        claim_and_complete_task(bus, epoch, seat, lane, task);
    }

    fn claim_and_complete_task(
        bus: &Bus,
        epoch: CanonicalUuid,
        seat: &Seat,
        lane: &Actor,
        task: &TaskHandle,
    ) {
        let available = board(bus, epoch, lane.clone());
        assert_committed(issue(
            bus,
            epoch,
            lane.clone(),
            CommandKind::ClaimWin,
            claim_expected(&available, task, seat),
            &ClaimWin {
                rule: "next".parse().expect("allocator rule"),
            },
        ));
        let held = board(bus, epoch, lane.clone());
        let expected = progress_witnesses(&held, at(&held, task), seat.lane, seat.occupant, false)
            .expect("held task has progress witnesses");
        assert_committed(issue(
            bus,
            epoch,
            lane.clone(),
            CommandKind::TaskDone,
            expected,
            &TaskDone {
                task: task.clone(),
                validation: "implementation and focused checks complete".to_owned(),
            },
        ));
    }

    fn complete_review_with_finding(
        bus: &Bus,
        epoch: CanonicalUuid,
        seat: &Seat,
        lane: &Actor,
        task: &TaskHandle,
    ) {
        let reviewable = board(bus, epoch, lane.clone());
        assert_eq!(at(&reviewable, task).workflow.body.phase.as_str(), "review");
        assert_committed(issue(
            bus,
            epoch,
            lane.clone(),
            CommandKind::ClaimWin,
            claim_expected(&reviewable, task, seat),
            &ClaimWin {
                rule: "next".parse().expect("allocator rule"),
            },
        ));
        let assigned = board(bus, epoch, lane.clone());
        assert_eq!(
            at(&assigned, task).workflow.body.assignment,
            Some(lane.clone())
        );
        let expected = progress_witnesses(
            &assigned,
            at(&assigned, task),
            seat.lane,
            seat.occupant,
            true,
        )
        .expect("assigned review has exact witnesses");
        assert_committed(issue(
            bus,
            epoch,
            lane.clone(),
            CommandKind::TaskReview,
            expected,
            &TaskReview {
                task: task.clone(),
                finding: "missing negative-path evidence".to_owned(),
                note: Some("recorded for the next authored correction".to_owned()),
            },
        ));
    }

    fn assert_review_settled(
        bus: &Bus,
        epoch: CanonicalUuid,
        task: &TaskHandle,
    ) -> BoardAnswer {
        let settled = board(bus, epoch, Actor::Operator);
        let heads = at(&settled, task);
        assert_eq!(heads.workflow.body.status, "completed");
        assert_eq!(heads.workflow.body.review, "finding");
        assert_eq!(
            heads.claim.as_ref().expect("review claim head").body.disposition,
            ClaimDisposition::Freed
        );
        let facts = bus.fact_snapshot().expect("workflow facts read");
        assert!(facts.facts().iter().any(|fact| {
            fact.fact.kind() == wire::FactKind::TaskReviewed
                && fact
                    .fact
                    .body()
                    .get("finding")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|finding| finding.contains("negative-path evidence"))
        }));
        settled
    }

    fn assert_restart_preserves(
        root: &Path,
        bus: Bus,
        epoch: CanonicalUuid,
        settled: &BoardAnswer,
    ) {
        bus.verify().expect("workflow authority verifies before restart");
        bus.shutdown().expect("workflow bus stops");
        let reopened = journal::Journal::open(root.join("authority.sqlite3"))
            .expect("workflow journal reopens");
        let reopened = Bus::start(reopened).expect("workflow bus restarts");
        assert_eq!(board(&reopened, epoch, Actor::Operator), *settled);
        clean(root, reopened);
    }

    struct AppliedDocument {
        preview: board::Preview,
        document: String,
        answer: wire::intent::DocAnswer,
        board: BoardAnswer,
    }

    #[test]
    fn authored_document_apply_is_atomic_idempotent_and_barrier_bound() {
        let (root, bus, epoch, _) = fixture("document", 0);
        let initial_board = board(&bus, epoch, Actor::Operator);
        let initial = apply_document(
            &bus,
            epoch,
            "# Goal\n\n- First task\n- Second task\n  After: first-task\n",
            &initial_board,
        );
        assert_initial_document(&initial);
        let edited = apply_document(
            &bus,
            epoch,
            "# Goal\n\n- First task\n  Tags: revised\n- Second task\n  After: first-task\n",
            &initial.board,
        );
        assert_edited_document(&bus, &initial, &edited);
        assert_idempotent_document(&bus, epoch, &edited);
        assert_stale_document_refused(&bus, epoch, &initial_board);
        assert_ledger_contains(&bus, epoch, &initial.answer.touched[1], "# First task");
        clean(&root, bus);
    }

    fn apply_document(
        bus: &Bus,
        epoch: CanonicalUuid,
        source: &str,
        barrier: &BoardAnswer,
    ) -> AppliedDocument {
        let preview = board::Preview::classify(source).expect("document previews");
        let document = dialect::emit(preview.document());
        let preview = board::Preview::classify(&document).expect("normal document previews");
        let answer = assert_committed(issue(
            bus,
            epoch,
            Actor::Operator,
            CommandKind::DocApply,
            vec![
                Witness::Preview {
                    digest: preview.digest().clone(),
                },
                Witness::Barrier {
                    name: "board".parse().expect("board barrier name"),
                    digest: board_barrier(barrier),
                },
            ],
            &DocApply {
                preview: preview.digest().clone(),
                document: document.clone(),
            },
        ));
        let CommandAnswer::Doc(answer) = answer else {
            panic!("document apply returned the wrong answer shape")
        };
        AppliedDocument {
            preview,
            document,
            answer,
            board: board(bus, epoch, Actor::Operator),
        }
    }

    fn assert_initial_document(applied: &AppliedDocument) {
        assert_eq!(applied.answer.touched.len(), 3);
        assert_eq!(applied.board.tasks.len(), 3);
        assert!(
            at(&applied.board, &applied.answer.touched[2])
                .statement
                .body
                .dependencies
                .contains(&applied.answer.touched[1])
        );
    }

    fn assert_edited_document(
        bus: &Bus,
        initial: &AppliedDocument,
        edited: &AppliedDocument,
    ) {
        assert_ne!(edited.preview.digest(), initial.preview.digest());
        assert_eq!(edited.answer.touched, initial.answer.touched);
        assert_eq!(edited.board.tasks.len(), 3);
        assert!(
            at(&edited.board, &edited.answer.touched[1])
                .statement
                .body
                .tags
                .contains(&"revised".to_owned())
        );
        let facts = bus.fact_snapshot().expect("document edit facts read");
        assert_eq!(
            facts
                .facts()
                .iter()
                .filter(|fact| fact.fact.kind() == wire::FactKind::TaskEdited)
                .count(),
            3,
            "an edited document must update its stable task identities",
        );
    }

    fn assert_idempotent_document(bus: &Bus, epoch: CanonicalUuid, edited: &AppliedDocument) {
        let before = bus
            .fact_snapshot()
            .expect("facts before idempotent apply")
            .facts()
            .len();
        let repeated = issue(
            bus,
            epoch,
            Actor::Operator,
            CommandKind::DocApply,
            vec![
                Witness::Preview {
                    digest: edited.preview.digest().clone(),
                },
                Witness::Barrier {
                    name: "board".parse().expect("board barrier name"),
                    digest: board_barrier(&edited.board),
                },
            ],
            &DocApply {
                preview: edited.preview.digest().clone(),
                document: edited.document.clone(),
            },
        );
        assert!(matches!(
            repeated,
            Outcome::Reply(Reply {
                position: ReplyPosition::Committed(_),
                body: CommandAnswer::Doc(_),
            })
        ));
        assert_eq!(
            bus.fact_snapshot()
                .expect("facts after idempotent apply")
                .facts()
                .len(),
            before,
            "the same document digest must not append a second durable path",
        );
    }

    fn assert_stale_document_refused(
        bus: &Bus,
        epoch: CanonicalUuid,
        initial_board: &BoardAnswer,
    ) {
        let changed = board::Preview::classify("# Goal\n\n- Third task\n")
            .expect("changed document previews");
        let stale = issue(
            bus,
            epoch,
            Actor::Operator,
            CommandKind::DocApply,
            vec![
                Witness::Preview {
                    digest: changed.digest().clone(),
                },
                Witness::Barrier {
                    name: "board".parse().expect("board barrier name"),
                    digest: board_barrier(initial_board),
                },
            ],
            &DocApply {
                preview: changed.digest().clone(),
                document: dialect::emit(changed.document()),
            },
        );
        assert!(matches!(
            stale,
            Outcome::Refusal(Refusal { ref reason, .. }) if reason == "doc::barrier-moved"
        ));
    }
}
