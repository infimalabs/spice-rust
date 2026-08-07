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

    #[test]
    fn one_captured_band_drives_board_answer_server_selection_and_guard_replay() {
        let (root, bus, epoch, seats) = fixture("allocation-band", 1);
        let lane = Actor::Lane(seats[0].lane);
        let lower = add(
            &bus,
            epoch,
            Actor::Operator,
            "BAND-a1",
            "Short lower handle",
            Some("todo"),
        );
        let higher = add(
            &bus,
            epoch,
            Actor::Operator,
            "BAND-z1",
            "Longer urgent flow",
            Some("todo,verify,review"),
        );
        let allocation = board::NextConfig::new(1);
        let available = board_with_allocation(&bus, epoch, lane.clone(), &allocation);
        assert_eq!(available.allocation_band, 1);

        let entries = actor_entries(&available).expect("configured board projects to entries");
        let subscription = allocation_subscription(&available.residency.body, &lane)
            .expect("fixture lane has one allocation subscription");
        let selected = board::select(&entries, &lane, subscription, available.allocation_band)
            .expect("configured client preselection has work");
        assert_eq!(selected.entry().statement().handle, higher);

        assert_committed(issue_with_allocation(
            &bus,
            epoch,
            lane.clone(),
            CommandKind::ClaimWin,
            claim_expected(&available, &higher, &seats[0]),
            &ClaimWin {
                rule: "next".parse().expect("allocator rule"),
            },
            &allocation,
        ));
        let held = board_with_allocation(&bus, epoch, lane, &allocation);
        assert!(at(&held, &lower).claim.is_none());
        assert_eq!(
            at(&held, &higher)
                .claim
                .as_ref()
                .expect("server and guard accepted the configured winner")
                .body
                .disposition,
            ClaimDisposition::Active,
        );
        clean(&root, bus);
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
        assert_review_assignment_fact(bus, &assigned, task, lane);
        let expected = progress_witnesses(
            &assigned,
            at(&assigned, task),
            seat.lane,
            seat.occupant,
            true,
        )
        .expect("assigned review has exact witnesses");
        let tracked: TaskHandle = "FOLLOW-1kKRRsDP".parse().expect("follow-up handle");
        let mut expected = expected;
        expected.push(Witness::Absence {
            task: tracked.clone(),
        });
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
                then: vec![ReviewFollowup {
                    task: tracked,
                    add: TaskAdd {
                        title: "Cover the negative path the review named".to_owned(),
                        project: None,
                        description: None,
                        acceptance: Vec::new(),
                        origin: None,
                        flow: None,
                        depends: Vec::new(),
                    },
                }],
                followup: Vec::new(),
            },
        ));
    }

    fn assert_review_assignment_fact(
        bus: &Bus,
        assigned: &BoardAnswer,
        task: &TaskHandle,
        reviewer: &Actor,
    ) {
        let heads = at(assigned, task);
        let snapshot = bus.fact_snapshot().expect("review assignment facts read");
        let envelope = snapshot
            .facts()
            .iter()
            .find(|fact| fact.seq == heads.workflow.position)
            .expect("review workflow head names a durable fact");
        let typed: wire::Envelope<wire::Fact<wire::body::TaskAssigned>> = wire::decode(
            &wire::encode(envelope).expect("opaque review assignment fact encodes"),
        )
        .expect("workflow head is the registered review assignment fact");
        assert_eq!(typed.fact.body.task, *task);
        assert_eq!(typed.fact.body.reviewer, *reviewer);
        assert_eq!(typed.fact.body.phase.as_str(), "review");
        assert!(typed.is_root(), "assignment begins the claim causal chain");

        let claim = heads.claim.as_ref().expect("review claim head");
        let won = snapshot
            .facts()
            .iter()
            .find(|fact| fact.seq == claim.position)
            .expect("review claim head names a durable fact");
        assert_eq!(won.fact.kind(), wire::FactKind::ClaimWon);
        assert_eq!(won.cause, Some(typed.seq));
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
        let entries = actor_entries(&settled).expect("settled board projects to entries");
        let entry = entries
            .iter()
            .find(|entry| entry.statement().handle == *task)
            .expect("settled task remains on the board");
        assert_eq!(
            accepted_transition(
                &settled.principal,
                &settled.residency.body,
                &entries,
                entry,
            ),
            NextTransition::Settled,
            "completion outranks the retained final review phase",
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

    /// A finding must name what will carry it, and the word clean must not.
    ///
    /// The two halves share one fixture on purpose: the refusal has to leave
    /// the claim exactly where it was, or the clean review that follows it
    /// could not be issued against the same witnesses.
    #[test]
    fn an_unclean_review_needs_follow_up_tracking_and_clean_does_not() {
        let (root, bus, epoch, seats) = fixture("review-followup", 2);
        let lane_one = Actor::Lane(seats[0].lane);
        let lane_two = Actor::Lane(seats[1].lane);
        assert_lane_resolution(&bus, epoch, &seats, &lane_one);
        let task = add(
            &bus,
            epoch,
            Actor::Operator,
            "TRACK-track001",
            "Track what a finding asks for",
            None,
        );
        advance_task_to_review(&bus, epoch, &seats[0], &lane_one, &task);
        let reviewable = board(&bus, epoch, lane_two.clone());
        assert_committed(issue(
            &bus,
            epoch,
            lane_two.clone(),
            CommandKind::ClaimWin,
            claim_expected(&reviewable, &task, &seats[1]),
            &ClaimWin {
                rule: "next".parse().expect("allocator rule"),
            },
        ));
        let assigned = board(&bus, epoch, lane_two.clone());
        let expected = progress_witnesses(
            &assigned,
            at(&assigned, &task),
            seats[1].lane,
            seats[1].occupant,
            true,
        )
        .expect("assigned review has exact witnesses");

        let untracked = issue(
            &bus,
            epoch,
            lane_two.clone(),
            CommandKind::TaskReview,
            expected.clone(),
            &TaskReview {
                task: task.clone(),
                finding: "the negative path has no test".to_owned(),
                note: None,
                then: Vec::new(),
                followup: Vec::new(),
            },
        );
        assert!(
            matches!(
                untracked,
                Outcome::Refusal(Refusal { ref reason, .. })
                    if reason == "review::followup-required"
            ),
            "a finding landed with nothing tracking it: {untracked:?}",
        );

        // The predecessor default and its allocator hint both spell the
        // verdict this way, so it has to settle the task clean rather than
        // demand follow-up tracking for a review that asked for nothing.
        assert_committed(issue(
            &bus,
            epoch,
            lane_two,
            CommandKind::TaskReview,
            expected,
            &TaskReview {
                task: task.clone(),
                finding: "CLEAN".to_owned(),
                note: None,
                then: Vec::new(),
                followup: Vec::new(),
            },
        ));
        let settled = board(&bus, epoch, Actor::Operator);
        let heads = at(&settled, &task);
        assert_eq!(heads.workflow.body.status, "completed");
        assert_eq!(
            heads.workflow.body.review, "clean",
            "the word clean recorded a finding named after the verdict"
        );
        clean(&root, bus);
    }

    /// Both follow-up forms persist the dependency they promise.
    #[test]
    fn a_review_creates_and_links_its_follow_up_tracking() {
        let (root, bus, epoch, seats) = fixture("review-followup-persistence", 2);
        let implementer = Actor::Lane(seats[0].lane);
        let reviewer = Actor::Lane(seats[1].lane);
        assert_lane_resolution(&bus, epoch, &seats, &implementer);
        let reviewed = add(
            &bus,
            epoch,
            Actor::Operator,
            "TRACK-track002",
            "Persist review tracking",
            None,
        );
        advance_task_to_review(&bus, epoch, &seats[0], &implementer, &reviewed);
        let reviewable = board(&bus, epoch, reviewer.clone());
        assert_committed(issue(
            &bus,
            epoch,
            reviewer.clone(),
            CommandKind::ClaimWin,
            claim_expected(&reviewable, &reviewed, &seats[1]),
            &ClaimWin {
                rule: "next".parse().expect("allocator rule"),
            },
        ));
        let linked = add(
            &bus,
            epoch,
            Actor::Operator,
            "TRACK-track003",
            "Link review tracking",
            None,
        );
        let assigned = board(&bus, epoch, reviewer.clone());
        let mut expected = progress_witnesses(
            &assigned,
            at(&assigned, &reviewed),
            seats[1].lane,
            seats[1].occupant,
            true,
        )
        .expect("assigned review has exact witnesses");
        let created: TaskHandle = "FOLLOW-1kKRRsDQ".parse().expect("follow-up handle");
        expected.push(Witness::Absence { task: created.clone() });
        let linked_heads = at(&assigned, &linked);
        expected.push(statement_witness(linked_heads));
        expected.push(workflow_witness(linked_heads));
        expected.push(Witness::Edge {
            task: linked.clone(),
            after: reviewed.clone(),
        });
        assert_committed(issue(
            &bus,
            epoch,
            reviewer,
            CommandKind::TaskReview,
            expected,
            &TaskReview {
                task: reviewed.clone(),
                finding: "two corrections need durable owners".to_owned(),
                note: None,
                then: vec![ReviewFollowup {
                    task: created.clone(),
                    add: TaskAdd {
                        title: "Create the missing review fixture".to_owned(),
                        project: Some("board.test".to_owned()),
                        description: None,
                        acceptance: vec!["the review path is covered".to_owned()],
                        origin: None,
                        flow: None,
                        depends: Vec::new(),
                    },
                }],
                followup: vec![linked.clone()],
            },
        ));
        assert_review_followup_tracking(&bus, epoch, &reviewed, &created, &linked);
        clean(&root, bus);
    }

    fn assert_review_followup_tracking(
        bus: &Bus,
        epoch: CanonicalUuid,
        reviewed: &TaskHandle,
        created: &TaskHandle,
        linked: &TaskHandle,
    ) {
        let settled = board(bus, epoch, Actor::Operator);
        assert_eq!(
            at(&settled, created).statement.body.dependencies,
            vec![reviewed.clone()],
            "--then did not create a task behind the reviewed task"
        );
        assert!(
            at(&settled, linked)
                .statement
                .body
                .dependencies
                .contains(reviewed),
            "--followup did not wire the existing task behind the review"
        );
        assert_eq!(at(&settled, reviewed).workflow.body.review, "finding");
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
    fn doc_leaf_owns_classification_and_normal_form_reasons() {
        let number = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "spice-board-command-doc-client-{}-{number}",
            std::process::id()
        ));
        fs::create_dir_all(root.join(".git")).expect("doc client fixture exists");
        let path = root.join("tasks.md");
        let repository = Repository::discover(&root).expect("doc client repository resolves");

        fs::write(&path, " \n\t\n").expect("refused document is written");
        let matches = crate::cli::command()
            .try_get_matches_from(["spice", "doc", &path.display().to_string()])
            .expect("doc leaf parses");
        let contract = crate::cli::selected_contract(&matches).expect("doc contract is selected");
        let leaf = crate::cli::selected_matches(&matches);
        let classification = run_doc(&repository, contract, leaf, dialect::emit);
        assert!(
            matches!(
                classification,
                Outcome::Refusal(Refusal { ref reason, .. })
                    if reason == "doc::classification-refused"
            ),
            "doc leaf lost its classification reason: {classification:?}",
        );

        fs::write(&path, "# Goal\n\n- First task\n").expect("valid document is written");
        let matches = crate::cli::command()
            .try_get_matches_from(["spice", "doc", &path.display().to_string()])
            .expect("doc leaf parses");
        let contract = crate::cli::selected_contract(&matches).expect("doc contract is selected");
        let leaf = crate::cli::selected_matches(&matches);
        let normal_form = run_doc(&repository, contract, leaf, |_| " \n\t\n".to_owned());
        assert!(
            matches!(
                normal_form,
                Outcome::Fault(Fault {
                    class: FaultClass::Protocol,
                    ref reason,
                    ..
                }) if reason == "doc::normal-form-invalid"
            ),
            "doc leaf lost its normal-form reason: {normal_form:?}",
        );

        fs::remove_dir_all(&root).expect("doc client fixture is removed");
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
