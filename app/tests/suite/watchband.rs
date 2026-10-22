use app::Session;
use app::runtime::{AppRuntime, LaneMember, LaneRow};
use app::watchband::{
    ACCENTS, MemberPresenceState, SHORT_LEN, TEAM_JOIN_GUTTER, TeamDrop, UNKNOWN_AGENT_LABEL,
    arrangement, badge, elapsed_label, elapsed_label_deadline_millis, first_nonquoted_prose,
    group_accent, history_frontier_request_due, job_matches_tasks, jobs_kept, latest_lane_prose,
    leaf, member_presence, one_line, remaining_label, remaining_label_deadline_millis,
    reported_tasks, short, standing_title, standings, task_mentions, team_accent, team_jobs, teams,
    ticker_chunks, ticker_line, topology, truncate,
};
use wire::CanonicalUuid;
use wire::head::TEAM_MEMBER_LIMIT;
use wire::intent::{BoardTask, LaneInbox};

const LANE_A: &str = "11111111-1111-4111-8111-111111111111";
const LANE_B: &str = "22222222-2222-4222-8222-222222222222";
const LANE_C: &str = "66666666-6666-4666-8666-666666666666";
const LANE_D: &str = "77777777-7777-4777-8777-777777777777";
const LANE_E: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const LANE_F: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
const LANE_G: &str = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";
const TEAM_ONE: &str = "88888888-8888-4888-8888-888888888888";
const TEAM_TWO: &str = "99999999-9999-4999-8999-999999999999";
const TEAM_THREE: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const OCCUPANT: &str = "33333333-3333-4333-8333-333333333333";
const EPOCH: &str = "44444444-4444-4444-8444-444444444444";
const BASE: &str = "2026-08-05T12:00:00.000Z";

fn uuid(text: &str) -> CanonicalUuid {
    text.parse().expect("the fixture spells a canonical uuid")
}

/// Milliseconds since the epoch at `BASE`, offset by `delta`.
fn at(delta: i64) -> i64 {
    BASE.parse::<wire::Instant>()
        .expect("the fixture spells a canonical instant")
        .get()
        .as_millisecond()
        + delta
}

/// The canonical spelling of `BASE` offset by `delta` milliseconds.
///
/// Every offset the fixtures use stays inside the day `BASE` names, so the
/// spelling only has to carry a time. Euclidean division keeps a negative
/// offset walking backwards through the hour rather than truncating toward it.
fn spell(delta: i64) -> String {
    let seconds = 43_200 + delta.div_euclid(1_000);
    let millis = delta.rem_euclid(1_000);
    let (hour, minute, second) = (seconds / 3_600, seconds % 3_600 / 60, seconds % 60);
    format!("2026-08-05T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

fn app_runtime() -> (Session, AppRuntime) {
    let session = Session::open(uuid(OCCUPANT), uuid(EPOCH));
    let accepted = session
        .admit_accepted(
            format!(r#"{{"wire":1,"actor":"operator","epoch":"{EPOCH}","release":"1.2.3"}}"#)
                .as_bytes(),
        )
        .expect("acceptance is admitted");
    let runtime = AppRuntime::new(&accepted);
    (session, runtime)
}

fn apply_agent_message(
    session: &Session,
    runtime: &mut AppRuntime,
    lane: &str,
    author: &str,
    authored: &str,
) {
    let bytes = serde_json::to_vec(&serde_json::json!({
        "stream": "messages",
        "v": 1,
        "id": TEAM_THREE,
        "epoch": EPOCH,
        "limit": 256,
        "generation": {
            "generation": 1,
            "horizon": 1,
            "messages": [{
                "lane": lane,
                "author": author,
                "locus": {"member": 1, "offset": 1, "length": 4, "event": 0},
                "authored": authored,
                "text": "done",
            }],
            "frontiers": [{"lane": lane, "state": "complete"}],
        },
    }))
    .expect("message frame serializes");
    let admitted = session
        .admit_stream(&bytes)
        .expect("message frame is admitted");
    runtime.apply_stream_frame(&admitted);
}

fn apply_lane_fact(
    session: &Session,
    runtime: &mut AppRuntime,
    position: u64,
    at: &str,
    fact: serde_json::Value,
) {
    let envelope = serde_json::json!({
        "v": 1,
        "seq": position,
        "at": at,
        "actor": "spice",
        "origin": format!("lane:{LANE_A}"),
        "fact": fact,
    });
    let bytes = serde_json::to_vec(&serde_json::json!({
        "stream": "fact",
        "v": 1,
        "id": TEAM_THREE,
        "epoch": EPOCH,
        "fact": envelope,
    }))
    .expect("fact frame serializes");
    let admitted = session
        .admit_stream(&bytes)
        .expect("fact frame is admitted");
    runtime.apply_stream_frame(&admitted);
}

fn standing_for(member: &LaneMember) -> app::watchband::Standing {
    let team = LaneRow {
        lane: member.lane,
        subscription: wire::intent::TeamLife::Steer,
        fast_mode: false,
        members: vec![member.clone()],
        console: None,
    };
    standings(&[team], &[], &[])
        .into_iter()
        .next()
        .expect("one member produces one standing")
}

fn lane(id: &str, worktree: &str) -> LaneRow {
    LaneRow {
        lane: uuid(id),
        subscription: wire::intent::TeamLife::Steer,
        fast_mode: false,
        members: vec![member(id, worktree)],
        console: None,
    }
}

fn member(id: &str, worktree: &str) -> LaneMember {
    LaneMember {
        lane: uuid(id),
        name: None,
        worktree: worktree.to_owned(),
        speech: wire::SpeechPlayback::Disabled {},
        life: "docker".to_owned(),
        vendor: "omp".parse().expect("the fixture spells a vendor name"),
        model: Some("openai-codex/gpt-5.2-codex".to_owned()),
        seat_model: None,
        effort: Some("high".to_owned()),
        seat_effort: None,
        fast_mode: false,
        seat_fast_mode: false,
        occupant: Some(uuid(OCCUPANT)),
        session: Some("session-fixture".to_owned()),
        disposition: "seated".to_owned(),
        liveness: wire::intent::LaneLiveness::Working,
        // A seat that has run: `bound` records that a resumable session was once
        // declared for it, and a bound seat is `wakeable`, which is true of every
        // lane this fixture builds because each one is occupied. No standing
        // reads either field, so the values are the fixture's stance rather than
        // anything asserted below -- spelled out because bare `true`s read as
        // load-bearing when they are not.
        bound: true,
        wakeable: true,
        jobs: Vec::new(),
    }
}

fn inbox(id: &str, directives: &[(&str, Option<&str>)]) -> LaneInbox {
    let pending = directives
        .iter()
        .enumerate()
        .map(|(index, (text, preview))| {
            let mut directive = serde_json::json!({
                "directive": OCCUPANT,
                "key": format!("key{index}"),
                "text": text,
                "authored": spell(0),
                "stage": "published",
                "expires": spell(600_000),
            });
            if let Some(preview) = preview {
                directive["preview"] = serde_json::Value::String((*preview).to_owned());
            }
            directive
        })
        .collect::<Vec<_>>();
    serde_json::from_value(serde_json::json!({
        "lane": id,
        "occupant": OCCUPANT,
        "compaction": 1,
        "pending": pending,
    }))
    .expect("the fixture spells a lane inbox")
}

/// A board task whose claim names `lane` and whose lease began `held` ago.
fn task(handle: &str, phase: &str, lane: &str, fact: u64, disposition: &str) -> BoardTask {
    let born = handle
        .parse::<wire::TaskHandle>()
        .expect("fixture task handle")
        .born()
        .expect("fixture handle carries a birth stamp");
    serde_json::from_value(serde_json::json!({
        "visibility": {"kind": "public"},
        "origin_thread": "operator",
        "presentation_title": "Rebuild the watchband",
        "statement": {
            "position": 1,
            "body": {
                "handle": handle,
                "born": born,
                "queue_origin": 1,
                "pitch": "The watchband shows who holds what.",
                "project": "canvas.fidelity",
                "dependencies": [],
                "provenance": format!("task:{born}"),
                "creation": {
                    "kind": "unavailable",
                    "reason": "operator-actor",
                },
                "revision": 1,
            },
        },
        "workflow": {
            "position": 2,
            "body": {
                "flow": ["todo", "review"],
                "phase": phase,
                "status": "pending",
                "review": "unassigned",
            },
        },
        "claim": {
            "position": 3,
            "body": {
                "actor": "operator",
                "fact": fact,
                "claimed": fact,
                "cell": {
                    "project": "canvas.fidelity",
                    "phase": phase,
                },
                "lane": lane,
                "occupant": OCCUPANT,
                "epoch": EPOCH,
                "disposition": disposition,
            },
        },
        "lease": {
            "position": 4,
            "body": {
                "start": spell(-300_000),
                "renewal": spell(-4_000),
                "maximum": spell(3_600_000),
                "expiry": spell(600_000),
                "epoch": EPOCH,
            },
        },
    }))
    .expect("the fixture spells a board task")
}

#[test]
fn presence_prefers_a_claim_then_current_occupant_activity() {
    let (session, mut runtime) = app_runtime();
    apply_agent_message(&session, &mut runtime, LANE_A, OCCUPANT, &spell(0));

    let mut lane_member = member(LANE_A, "main-a");
    lane_member.liveness = wire::intent::LaneLiveness::Running;
    let mut standing = standing_for(&lane_member);

    let idle = member_presence(&runtime, &lane_member, &standing);
    assert_eq!(idle.state, MemberPresenceState::Idle);
    assert_eq!(
        idle.since_millis,
        Some(at(
            i64::try_from(wire::intent::LANE_QUIET_HORIZON_MILLIS).unwrap()
        )),
        "idle begins at the resident's shared quiet boundary"
    );

    standing.handle = Some("APP-1kKRSfyV".to_owned());
    standing.held_since = Some(at(-300_000));
    let claimed = member_presence(&runtime, &lane_member, &standing);
    assert_eq!(claimed.state, MemberPresenceState::Working);
    assert_eq!(claimed.since_millis, standing.held_since);

    standing.handle = None;
    standing.held_since = None;
    lane_member.liveness = wire::intent::LaneLiveness::Working;
    let active = member_presence(&runtime, &lane_member, &standing);
    assert_eq!(active.state, MemberPresenceState::Working);
    assert_eq!(active.since_millis, Some(at(0)));
}

#[test]
fn presence_dates_successor_start_and_vacancy_from_durable_lane_facts() {
    let (session, mut runtime) = app_runtime();
    apply_agent_message(&session, &mut runtime, LANE_A, OCCUPANT, &spell(0));
    apply_lane_fact(
        &session,
        &mut runtime,
        1,
        &spell(1_000),
        serde_json::json!({
            "kind": "lane::started",
            "lane": LANE_A,
            "occupant": TEAM_TWO,
            "epoch": EPOCH,
            "group": 1,
            "fresh_session": true,
        }),
    );

    let mut successor = member(LANE_A, "main-a");
    successor.occupant = Some(uuid(TEAM_TWO));
    successor.liveness = wire::intent::LaneLiveness::Running;
    let starting = member_presence(&runtime, &successor, &standing_for(&successor));
    assert_eq!(starting.state, MemberPresenceState::Starting);
    assert_eq!(
        starting.since_millis,
        Some(at(1_000)),
        "predecessor activity cannot make its successor idle"
    );

    apply_lane_fact(
        &session,
        &mut runtime,
        2,
        &spell(2_000),
        serde_json::json!({
            "kind": "lane::exited",
            "lane": LANE_A,
            "occupant": TEAM_TWO,
            "code": 0,
        }),
    );
    successor.occupant = None;
    successor.liveness = wire::intent::LaneLiveness::Vacant;
    let mut stale_claim = standing_for(&successor);
    stale_claim.handle = Some("APP-1kKRSfyV".to_owned());
    stale_claim.held_since = Some(at(-300_000));
    let vacant = member_presence(&runtime, &successor, &stale_claim);
    assert_eq!(
        vacant.state,
        MemberPresenceState::Vacant,
        "a retained claim cannot make an absent occupant present"
    );
    assert_eq!(vacant.since_millis, Some(at(2_000)));
}

#[test]
fn a_standing_joins_the_lane_table_the_inbox_and_the_board() {
    let lanes = [lane(LANE_A, "main-c"), lane(LANE_B, "main-g")];
    let waiting = [inbox(
        LANE_A,
        &[
            ("first direction", Some("first direction")),
            ("second direction", Some("second direction")),
        ],
    )];
    let tasks = [task("FIDELIT-1kKRSfyV", "review", LANE_A, 7, "active")];

    let rows = standings(&lanes, &waiting, &tasks);

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].worktree, "main-c");
    assert_eq!(rows[0].vendor, "omp");
    assert_eq!(rows[0].model.as_deref(), Some("openai-codex/gpt-5.2-codex"));
    assert_eq!(rows[0].effort.as_deref(), Some("high"));
    assert_eq!(rows[0].thread, Some(uuid(OCCUPANT)));
    assert_eq!(rows[0].session.as_deref(), Some("session-fixture"));
    assert_eq!(rows[0].life, "docker");
    assert_eq!(rows[0].disposition, "seated");
    assert_eq!(rows[0].liveness, wire::intent::LaneLiveness::Working);
    assert_eq!(rows[0].pending, 2);
    assert_eq!(rows[0].handle.as_deref(), Some("FIDELIT-1kKRSfyV"));
    assert_eq!(rows[0].task_title.as_deref(), Some("Rebuild the watchband"));
    assert_eq!(rows[0].phase.as_deref(), Some("review"));

    // The second lane is deliberately unlike the first: no inbox row and no
    // claim, so a join that returned the first match for every lane fails here
    // rather than passing on two identical rows.
    assert_eq!(rows[1].worktree, "main-g");
    assert_eq!(rows[1].pending, 0);
    assert_eq!(rows[1].headline, None);
    assert_eq!(rows[1].handle, None);
    assert_eq!(rows[1].task_title, None);
    assert_eq!(rows[1].phase, None);
    assert_eq!(rows[1].held_since, None);
    assert_eq!(rows[1].expires_at, None);
}

#[test]
fn one_team_emits_one_standing_per_member_without_losing_the_teamentity() {
    let team = LaneRow {
        lane: uuid(EPOCH),
        subscription: wire::intent::TeamLife::Steer,
        fast_mode: false,
        members: vec![member(LANE_A, "main-a"), member(LANE_B, "main-b")],
        console: Some(0),
    };

    let rows = standings(&[team], &[], &[]);

    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row.team == uuid(EPOCH)));
    assert_eq!(rows[0].lane, uuid(LANE_A));
    assert_eq!(rows[1].lane, uuid(LANE_B));
    assert_eq!(rows[0].label, "main-a");
    assert_eq!(rows[1].label, "main-b");
}

#[test]
fn the_headline_is_the_newest_producer_preview() {
    let lanes = [lane(LANE_A, "main-c")];
    let waiting = [inbox(
        LANE_A,
        &[
            ("older direction", Some("older direction")),
            (
                "> quoted context\n\nnewer author words",
                Some("newer author words"),
            ),
        ],
    )];

    let rows = standings(&lanes, &waiting, &[]);

    assert_eq!(rows[0].headline.as_deref(), Some("newer author words"));
}

#[test]
fn absent_producer_preview_does_not_reinterpret_the_directive_text() {
    let lanes = [lane(LANE_A, "main-c")];
    let waiting = [inbox(LANE_A, &[("text the client must not parse", None)])];

    let rows = standings(&lanes, &waiting, &[]);

    assert_eq!(rows[0].pending, 1);
    assert_eq!(rows[0].headline, None);
}

#[test]
fn the_newest_active_claim_on_a_lane_is_the_one_showing() {
    let lanes = [lane(LANE_A, "main-c")];
    let tasks = [
        task("OLDER-1kKQXgDd", "todo", LANE_A, 7, "active"),
        task("NEWER-1kKRSfyV", "review", LANE_A, 9, "active"),
        // A released claim with the greatest position of all: it must not win
        // simply for being newest.
        task("FREED-1kKRRQ28", "done", LANE_A, 11, "freed"),
        // Another lane's active claim, newer still.
        task("OTHER-1kKRRQJN", "todo", LANE_B, 13, "active"),
    ];

    let rows = standings(&lanes, &[], &tasks);

    assert_eq!(rows[0].handle.as_deref(), Some("NEWER-1kKRSfyV"));
    assert_eq!(rows[0].task_title.as_deref(), Some("Rebuild the watchband"));
    assert_eq!(rows[0].phase.as_deref(), Some("review"));
}

/// A standing states *when* the lease began and expires, not precomputed spans.
///
/// The second half is the part that matters to a quiet app. One row, built
/// once, is read against two clocks a minute apart and spells two different
/// values: claim age increases while remaining lease time decreases. A lane the
/// resident has stopped mentioning therefore remains truthful without another
/// frame, refresh, or replay.
#[test]
fn a_standing_carries_an_expiry_that_counts_down_after_the_row_is_built() {
    let lanes = [lane(LANE_A, "main-c")];
    let tasks = [task("FIDELIT-1kKRSfyV", "review", LANE_A, 7, "active")];

    let rows = standings(&lanes, &[], &tasks);

    let held = rows[0].held_since.expect("the lease began");
    let expiry = rows[0].expires_at.expect("the lease expires");
    assert_eq!(held, at(-300_000));
    assert_eq!(expiry, at(600_000));
    assert_eq!(elapsed_label(at(0) - held), "5m");
    assert_eq!(remaining_label(expiry - at(0)), "10m");

    assert_eq!(elapsed_label(at(60_000) - held), "6m");
    assert_eq!(remaining_label(expiry - at(60_000)), "9m");
}

#[test]
fn team_members_receive_the_complete_palette_from_the_main_right_edge() {
    let members = [
        (LANE_A, "main-a"),
        (LANE_B, "main-b"),
        (LANE_C, "main-c"),
        (LANE_D, "main-d"),
        (LANE_E, "main-e"),
        (LANE_F, "main-f"),
    ];
    let mut reversed = members;
    reversed.reverse();

    let first = standings(&[seated(EPOCH, &members)], &[], &[]);
    let second = standings(&[seated(EPOCH, &reversed)], &[], &[]);

    let complete_palette = (0..ACCENTS).collect::<Vec<_>>();
    assert_eq!(
        first.iter().map(|row| row.accent).collect::<Vec<_>>(),
        complete_palette
    );
    assert_eq!(
        second.iter().map(|row| row.accent).collect::<Vec<_>>(),
        complete_palette
    );
    assert_eq!(first[0].lane, second[5].lane);
    assert_eq!(first[0].accent, 0);
    assert_eq!(
        second[5].accent, 5,
        "a member reordered to the far left receives that position's accent"
    );
    assert_eq!(
        (0..=ACCENTS)
            .map(|position| team_accent(0, position))
            .collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4, 5, 0],
        "the seventh member wraps at the far left without moving the right anchor"
    );
}

#[test]
fn teams_and_working_groups_deal_their_bases_from_opposite_palette_ends() {
    assert_eq!(
        (0..=ACCENTS)
            .map(|console| team_accent(console, 0))
            .collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4, 5, 0],
        "each later team's lead starts one slot further up the palette"
    );
    assert_eq!(
        (0..=ACCENTS)
            .map(|console| group_accent(console, 0))
            .collect::<Vec<_>>(),
        vec![5, 4, 3, 2, 1, 0, 5],
        "each later working group's lead design starts one slot further down"
    );
    assert_eq!(
        (0..=ACCENTS)
            .map(|position| group_accent(1, position))
            .collect::<Vec<_>>(),
        vec![4, 3, 2, 1, 0, 5, 4],
        "a group's later designs count down from its base and wrap"
    );
}

#[test]
fn wrapped_team_accents_do_not_replace_member_identity() {
    let team = seated(
        EPOCH,
        &[
            (LANE_A, "main-a"),
            (LANE_B, "main-b"),
            (LANE_C, "main-c"),
            (LANE_D, "main-d"),
            (LANE_E, "main-e"),
            (LANE_F, "main-f"),
            (LANE_G, "main-g"),
        ],
    );

    let rows = standings(&[team], &[], &[]);

    assert_eq!(rows[0].accent, rows[6].accent);
    assert_ne!(rows[0].lane, rows[6].lane);
    assert_ne!(
        rows[0].name, rows[6].name,
        "identity stays distinct when colors wrap"
    );
}

#[test]
fn a_lane_is_named_by_the_identity_the_resident_gave_it() {
    let rows = standings(&[lane(LANE_A, "main-c")], &[], &[]);

    assert_eq!(rows[0].name, "11111111");
    assert_eq!(rows[0].name.chars().count(), SHORT_LEN);
    assert_eq!(short(uuid(LANE_B)), "22222222");
}

#[test]
fn a_lane_is_labelled_by_what_separates_it_from_its_fleet() {
    // Real worktrees are siblings under one parent, which is the case a label
    // cut from the front of the path cannot survive: both would read
    // "/fleet/one/spice-…" and the band would show two identical cards.
    let lanes = [
        lane(LANE_A, "/fleet/one/spice-rust-a"),
        lane(LANE_B, "/fleet/one/spice-rust-g/"),
    ];

    let rows = standings(&lanes, &[], &[]);

    assert_eq!(rows[0].label, "spice-rust-a");
    assert_eq!(rows[1].label, "spice-rust-g");
    assert_eq!(rows[0].worktree, "/fleet/one/spice-rust-a");
    assert_eq!(leaf("main-c"), "main-c");
    assert_eq!(leaf("/"), "/");
}

#[test]
fn a_configured_agent_name_wins_while_an_unconfigured_lane_uses_the_worktree_leaf() {
    let mut configured = lane(LANE_A, "/fleet/one/spice-rust-a");
    configured.members[0].name = Some(" Ava ".to_owned());
    let unconfigured = lane(
        LANE_B,
        "/fleet/one/lane-4-release-observability-and-follow-through",
    );
    let unidentified = lane(LANE_C, "/");

    let rows = standings(&[configured, unconfigured, unidentified], &[], &[]);

    assert_eq!(rows[0].label, "Ava");
    assert_eq!(rows[0].worktree, "/fleet/one/spice-rust-a");
    assert_eq!(
        rows[1].label,
        "lane-4-release-observability-and-follow-through"
    );
    assert_eq!(
        rows[1].worktree,
        "/fleet/one/lane-4-release-observability-and-follow-through"
    );
    assert_eq!(rows[2].label, UNKNOWN_AGENT_LABEL);
    assert_eq!(rows[0].pending, 0, "pending remains an inbox-derived join");
}

#[test]
fn configured_agent_names_distinguish_two_lanes_that_share_one_worktree() {
    let mut ava = lane(LANE_A, "/fleet/shared");
    ava.members[0].name = Some("Ava".to_owned());
    let mut matilda = lane(LANE_B, "/fleet/shared");
    matilda.members[0].name = Some("Matilda".to_owned());

    let rows = standings(&[ava, matilda], &[], &[]);

    assert_eq!(rows[0].label, "Ava");
    assert_eq!(rows[1].label, "Matilda");
    assert_eq!(rows[0].worktree, rows[1].worktree);
}

#[test]
fn a_standing_title_names_recorded_and_unrecorded_launches_without_substitution() {
    let mut standing = standings(&[lane(LANE_A, "/fleet/one/spice-rust-a")], &[], &[])
        .pop()
        .expect("fixture standing");
    assert_eq!(
        standing_title(&standing),
        format!(
            "OpenAI / gpt-5.2-codex; effort: high; thread: {OCCUPANT}; \
             session: session-fixture"
        )
    );

    standing.model = None;
    standing.effort = None;
    assert_eq!(
        standing_title(&standing),
        format!(
            "model unrecorded; effort: unrecorded; thread: {OCCUPANT}; \
             session: session-fixture"
        )
    );

    standing.jobs = vec![
        wire::intent::LaneJob {
            id: "6ZbQk3mPx".to_owned(),
            command: "cargo test".to_owned(),
            task: None,
            kind: wire::intent::LaneJobKind::Command,
            standing: wire::intent::LaneJobStanding::Running,
        },
        wire::intent::LaneJob {
            id: "6ZbQk3mPy".to_owned(),
            command: "cargo doc".to_owned(),
            task: None,
            kind: wire::intent::LaneJobKind::Service,
            standing: wire::intent::LaneJobStanding::Unheard,
        },
    ];
    assert_eq!(
        standing_title(&standing),
        format!(
            "model unrecorded; effort: unrecorded; thread: {OCCUPANT}; \
             session: session-fixture; waiting on 2 jobs (1 running, 1 unheard)"
        )
    );
    standing.jobs.pop();
    assert_eq!(
        jobs_kept(&standing.jobs).as_deref(),
        Some("waiting on 1 job (1 running)")
    );
    assert_eq!(jobs_kept(&[]), None);
}

#[test]
fn a_task_view_keeps_only_jobs_associated_with_its_selected_tasks() {
    let selected = wire::TaskStamp::for_micros(10).expect("fixture task");
    let other = wire::TaskStamp::for_micros(20).expect("fixture task");
    let job = |task| wire::intent::LaneJob {
        id: "6ZbQk3mPx".to_owned(),
        command: "cargo test".to_owned(),
        task,
        kind: wire::intent::LaneJobKind::Command,
        standing: wire::intent::LaneJobStanding::Running,
    };

    assert!(job_matches_tasks(
        &job(Some(selected.clone())),
        std::slice::from_ref(&selected),
    ));
    assert!(!job_matches_tasks(
        &job(Some(other)),
        std::slice::from_ref(&selected),
    ));
    assert!(!job_matches_tasks(
        &job(None),
        std::slice::from_ref(&selected),
    ));
}

#[test]
fn team_jobs_interleave_members_by_start_and_keep_row_identity_across_standing() {
    let mut members = standings(
        &[
            lane(LANE_A, "/fleet/one/spice-rust-a"),
            lane(LANE_B, "/fleet/one/spice-rust-b"),
        ],
        &[],
        &[],
    );
    let micros = 1_790_000_000_123_456_i64;
    let key = |offset: i64| {
        wire::TaskStamp::for_micros(micros + offset)
            .expect("fixture moment")
            .to_string()
    };
    let job = |offset: i64, command: &str, standing| wire::intent::LaneJob {
        id: key(offset),
        command: command.to_owned(),
        task: None,
        kind: wire::intent::LaneJobKind::Command,
        standing,
    };
    use wire::intent::LaneJobStanding::{Running, Unheard};
    members[0].jobs = vec![job(0, "a first", Running), job(20, "a third", Unheard)];
    members[1].jobs = vec![job(10, "b second", Running)];

    let rows = team_jobs(&members);
    assert_eq!(
        rows.iter()
            .map(|row| row.job.command.as_str())
            .collect::<Vec<_>>(),
        ["a first", "b second", "a third"]
    );
    assert_eq!(rows[0].started_millis(), Some(micros / 1_000));
    let before = rows.iter().map(|row| row.key()).collect::<Vec<_>>();

    members[0].jobs[0].standing = Unheard;
    let after = team_jobs(&members)
        .iter()
        .map(|row| row.key())
        .collect::<Vec<_>>();
    assert_eq!(before, after);
    assert_ne!(before[0], before[1]);
    assert!(team_jobs(&[]).is_empty());
}

#[test]
fn elapsed_names_one_unit_at_each_boundary() {
    assert_eq!(elapsed_label(-5_000), "now");
    assert_eq!(elapsed_label(999), "now");
    assert_eq!(elapsed_label(1_000), "1s");
    assert_eq!(elapsed_label(59_999), "59s");
    assert_eq!(elapsed_label(60_000), "1m");
    assert_eq!(elapsed_label(3_599_999), "59m");
    assert_eq!(elapsed_label(3_600_000), "1h");
    assert_eq!(elapsed_label(86_399_999), "23h");
    assert_eq!(elapsed_label(86_400_000), "1d");
}

#[test]
fn elapsed_deadlines_wake_only_when_the_visible_unit_can_change() {
    assert_eq!(elapsed_label_deadline_millis(-5_000), Some(6_000));
    assert_eq!(elapsed_label_deadline_millis(0), Some(1_000));
    assert_eq!(elapsed_label_deadline_millis(999), Some(1));
    assert_eq!(elapsed_label_deadline_millis(1_000), Some(1_000));
    assert_eq!(elapsed_label_deadline_millis(59_999), Some(1));
    assert_eq!(elapsed_label_deadline_millis(60_000), Some(60_000));
    assert_eq!(elapsed_label_deadline_millis(3_599_999), Some(1));
    assert_eq!(elapsed_label_deadline_millis(3_600_000), Some(3_600_000));
    assert_eq!(elapsed_label_deadline_millis(86_399_999), Some(1));
    assert_eq!(elapsed_label_deadline_millis(86_400_000), Some(86_400_000));
    assert_eq!(
        elapsed_label_deadline_millis(99 * 86_400_000),
        Some(86_400_000),
    );
    assert_eq!(elapsed_label_deadline_millis(100 * 86_400_000), None);
    assert_eq!(elapsed_label_deadline_millis(i64::MAX), None);
}

#[test]
fn remaining_time_counts_down_across_the_existing_unit_boundaries() {
    assert_eq!(remaining_label(-1), "now");
    assert_eq!(remaining_label(999), "now");
    assert_eq!(remaining_label(1_000), "1s");
    assert_eq!(remaining_label(59_999), "59s");
    assert_eq!(remaining_label(60_000), "1m");
    assert_eq!(remaining_label(3_599_999), "59m");
    assert_eq!(remaining_label(3_600_000), "1h");
    assert_eq!(remaining_label(86_399_999), "23h");
    assert_eq!(remaining_label(86_400_000), "1d");

    assert_eq!(remaining_label_deadline_millis(-1), None);
    assert_eq!(remaining_label_deadline_millis(999), None);
    assert_eq!(remaining_label_deadline_millis(1_000), Some(1));
    assert_eq!(remaining_label_deadline_millis(1_001), Some(2));
    assert_eq!(remaining_label_deadline_millis(59_999), Some(1_000));
    assert_eq!(remaining_label_deadline_millis(60_000), Some(1));
    assert_eq!(remaining_label_deadline_millis(60_001), Some(2));
    assert_eq!(remaining_label_deadline_millis(3_599_999), Some(60_000));
    assert_eq!(remaining_label_deadline_millis(3_600_000), Some(1));
    assert_eq!(remaining_label_deadline_millis(86_400_000), Some(1));
    assert_eq!(remaining_label_deadline_millis(99 * 86_400_000), Some(1));
    assert_eq!(remaining_label_deadline_millis(100 * 86_400_000), Some(1));
    assert_eq!(
        remaining_label_deadline_millis(100 * 86_400_000 + 5_000),
        Some(5_001),
    );
}

/// The saturating arm has to be legible as saturation, not misread as a real
/// count, and it has to begin exactly one day after the last true count.
#[test]
fn the_day_arm_saturates_rather_than_widening() {
    assert_eq!(elapsed_label(99 * 86_400_000), "99d");
    assert_eq!(elapsed_label(100 * 86_400_000), "99d+");
    assert_eq!(elapsed_label(i64::MAX), "99d+");
}

#[test]
fn composer_prose_skips_blank_and_borrowed_lines_at_any_quote_depth() {
    assert_eq!(
        first_nonquoted_prose(
            "\r\n  > > borrowed nested context\r\n\t> borrowed context\r\n\r\n  authored first line  \r\nauthored second line"
        )
        .as_deref(),
        Some("authored first line")
    );
}

#[test]
fn composer_prose_skips_structural_quote_links() {
    let target = wire::QuoteTarget::Steering {
        key: "dTW6dyT56".to_owned(),
        digest: wire::QuoteTarget::text_digest("quoted operator message"),
    };
    let message = format!(
        "{}\n\n> copied fallback context\n\n  **authored reply**  ",
        target.markdown(),
    );

    assert_eq!(
        first_nonquoted_prose(&message).as_deref(),
        Some("**authored reply**"),
    );
}

#[test]
fn composer_prose_skips_exact_unavailable_quote_context_only() {
    let target = wire::QuoteTarget::Steering {
        key: "dTW6dyT56".to_owned(),
        digest: wire::QuoteTarget::text_digest("quoted operator message"),
    };
    let marker = format!(
        "[quote unavailable: reason=unknown source={:?}]",
        target.to_string()
    );

    assert_eq!(
        first_nonquoted_prose(&format!("{marker}\n\n  **authored reply**  ")).as_deref(),
        Some("**authored reply**"),
    );
    assert_eq!(first_nonquoted_prose(&marker), None);
    assert_eq!(
        first_nonquoted_prose("[quote unavailable: temporarily offline]").as_deref(),
        Some("[quote unavailable: temporarily offline]"),
    );
    assert_eq!(
        first_nonquoted_prose(
            "[quote unavailable: reason=unknown source=\"https://example.test\"]"
        )
        .as_deref(),
        Some("[quote unavailable: reason=unknown source=\"https://example.test\"]"),
    );
}

#[test]
fn an_entirely_quoted_message_has_no_composer_prose() {
    assert_eq!(
        first_nonquoted_prose("> borrowed\n> > borrowed deeper\n  > borrowed after whitespace"),
        None
    );
    assert_eq!(first_nonquoted_prose(" \r\n\t\r\n"), None);
}

/// One admitted agent message, `delta` milliseconds after `BASE`, in `lane`.
fn message(lane: &str, delta: i64, activity: Option<&str>, text: &str) -> wire::AgentMessage {
    let mut message = serde_json::json!({
        "lane": lane,
        "author": OCCUPANT,
        "locus": {"member": 1, "offset": delta, "length": 20, "event": 0},
        "authored": spell(delta),
        "text": text,
    });
    if let Some(activity) = activity {
        message["activity"] = serde_json::Value::String(activity.to_owned());
    }
    serde_json::from_value(message).expect("the fixture spells an admitted agent message")
}

#[test]
fn a_lane_without_prose_of_its_own_borrows_none_from_the_team() {
    let messages = [
        message(LANE_A, 0, None, "Boarded as WATCHBA-1"),
        message(LANE_A, 1_000, None, "Boarded as WATCHBA-2"),
    ];

    assert_eq!(
        latest_lane_prose(uuid(LANE_A), messages.iter())
            .map(|(source, prose)| (source.display_millis(), prose)),
        Some((at(1_000), "Boarded as WATCHBA-2".to_owned()))
    );
    assert_eq!(latest_lane_prose(uuid(LANE_B), messages.iter()), None);
    assert_eq!(latest_lane_prose(uuid(LANE_B), [].iter()), None);
}

#[test]
fn a_lane_keeps_its_own_newest_prose_when_a_peer_speaks_later() {
    let messages = [
        message(LANE_A, 0, None, "older own prose"),
        message(LANE_A, 1_000, None, "newest own prose"),
        message(LANE_B, 2_000, None, "the peer's newer prose"),
    ];

    let (source, prose) =
        latest_lane_prose(uuid(LANE_A), messages.iter()).expect("lane A authored prose");
    assert_eq!(source.lane, uuid(LANE_A));
    assert_eq!(source.display_millis(), at(1_000));
    assert_eq!(prose, "newest own prose");
}

#[test]
fn ineligible_own_activity_yields_older_own_prose_never_a_peer_message() {
    let messages = [
        message(LANE_A, 0, None, "> borrowed\nauthored under the quote"),
        message(LANE_B, 1_000, None, "the peer's prose"),
        message(
            LANE_A,
            2_000,
            None,
            "> entirely borrowed\n> > at every depth",
        ),
        message(LANE_A, 3_000, Some("tool"), "Bash: cargo test"),
        message(
            LANE_A,
            4_000,
            Some("reasoning"),
            "thinking about the header",
        ),
        message(LANE_A, 5_000, Some("turn"), "new turn"),
        message(LANE_A, 6_000, Some("compaction"), "compaction"),
    ];

    let (source, prose) =
        latest_lane_prose(uuid(LANE_A), messages.iter()).expect("lane A authored prose");
    assert_eq!(source.display_millis(), at(0));
    assert_eq!(prose, "authored under the quote");

    let only_ineligible = &messages[1..];
    assert_eq!(
        latest_lane_prose(uuid(LANE_A), only_ineligible.iter()),
        None
    );
}
#[test]
fn missing_composer_prose_demands_only_successive_history_frontiers() {
    assert!(history_frontier_request_due(
        None, 4, false, true, false, false, false,
    ));
    assert!(!history_frontier_request_due(
        Some(4),
        5,
        false,
        true,
        false,
        false,
        false,
    ));
    assert!(!history_frontier_request_due(
        None, 4, false, false, false, false, false,
    ));
    assert!(history_frontier_request_due(
        None, 4, false, false, true, false, false,
    ));
    assert!(history_frontier_request_due(
        Some(4),
        5,
        true,
        false,
        true,
        false,
        false,
    ));
}

#[test]
fn a_pill_wears_no_badge_at_zero() {
    assert_eq!(badge(0), None);
    assert_eq!(badge(4).as_deref(), Some("4"));
    assert_eq!(badge(27).as_deref(), Some("27"));
}

#[test]
fn authored_prose_reports_both_canonical_task_widths_once_through_punctuation() {
    for candidate in [
        "APP-a1",
        "OVERFLOW-0000000000",
        "WRONG-1kKAC3j4",
        "SHAPED-abcdefgh",
    ] {
        assert!(
            candidate.parse::<wire::TaskHandle>().is_err(),
            "{candidate} must fail canonical task-handle admission"
        );
    }

    let text = "Settled `CONTROL-1kKbC3j4` (CONTROL-1kKbC3j4 again) and \
                IDENTIT-dRyDYZqJJ. APP-a1 is short; \
                OVERFLOW-0000000000 is wide; WRONG-1kKAC3j4 uses the wrong alphabet; \
                SHAPED-abcdefgh only matches the broad shape; lower-a1 TASK- TASK-a-b \
                and ordinary-hyphenated-prose are not handles.";
    let mentions = task_mentions(text);
    assert_eq!(
        mentions
            .iter()
            .map(|(range, task)| (&text[range.clone()], task.to_string()))
            .collect::<Vec<_>>(),
        [
            ("CONTROL-1kKbC3j4", "CONTROL-1kKbC3j4".to_owned()),
            ("CONTROL-1kKbC3j4", "CONTROL-1kKbC3j4".to_owned()),
            ("IDENTIT-dRyDYZqJJ", "IDENTIT-dRyDYZqJJ".to_owned()),
        ],
        "the parser-backed scanner retains only canonical inline locations",
    );

    let tasks = reported_tasks(text);

    assert_eq!(
        tasks.iter().map(ToString::to_string).collect::<Vec<_>>(),
        ["CONTROL-1kKbC3j4", "IDENTIT-dRyDYZqJJ"],
    );
}

#[test]
fn the_ticker_folds_a_command_onto_one_line_within_its_budget() {
    let detail = "const r = await tools.exec_command({\n\tcmd: \"spice task show\"\n})";

    let full = ticker_line("exec", detail, 200);
    assert_eq!(
        full,
        "exec: const r = await tools.exec_command({\\n cmd: \"spice task show\"\\n})"
    );
    assert!(!full.contains('\n'));

    let cut = ticker_line("exec", detail, 24);
    assert_eq!(cut.chars().count(), 24);
    assert!(cut.ends_with('…'));
    assert_eq!(cut, "exec: const r = await t…");

    // A class with no detail is the whole line, and a line inside the budget
    // keeps every character rather than spending one on an ellipsis.
    assert_eq!(ticker_line("board.read", "", 24), "board.read");
    assert_eq!(truncate("exec", 4), "exec");
    assert_eq!(truncate("exécut", 4), "exé…");
    assert_eq!(one_line("  spaced \r\n words  "), "spaced \\n words");
}

#[test]
fn tool_ticker_chunks_accent_only_the_first_whitespace_delimited_token() {
    assert_eq!(
        ticker_chunks("tool", "cargo test:all --url https://host:443", 80),
        (
            "cargo".to_owned(),
            "test:all --url https://host:443".to_owned(),
        ),
    );
    assert_eq!(
        ticker_chunks("tool", "Read: app/src/lib.rs", 80),
        ("Read".to_owned(), "app/src/lib.rs".to_owned()),
    );
    assert_eq!(
        ticker_chunks("assistant", "concise response", 80),
        ("assistant".to_owned(), "concise response".to_owned()),
    );
    let budget = 18;
    let (prefix, detail) = ticker_chunks("tool", "Bash: a deliberately long command", budget);
    assert_eq!(prefix, "Bash");
    assert!(detail.ends_with('…'));
    assert!(format!("{prefix} {detail}").chars().count() <= budget);
}

/// One team holding several agents, so a join has more than one agent to move.
fn seated(id: &str, members: &[(&str, &str)]) -> LaneRow {
    LaneRow {
        lane: uuid(id),
        subscription: wire::intent::TeamLife::Steer,
        fast_mode: false,
        members: members
            .iter()
            .map(|(lane, worktree)| member(lane, worktree))
            .collect(),
        console: Some(0),
    }
}

fn arranged(
    lanes: &[LaneRow],
    dragged: &str,
    target: &str,
    drop: TeamDrop,
) -> Vec<(CanonicalUuid, Vec<CanonicalUuid>)> {
    arrangement(&topology(lanes), uuid(dragged), uuid(target), drop)
        .expect("a drop of one live team onto another states an arrangement")
}

#[test]
fn a_swap_exchanges_exactly_the_two_team_positions_without_moving_an_agent() {
    let lanes = [
        seated(TEAM_ONE, &[(LANE_A, "main-a"), (LANE_B, "main-b")]),
        seated(TEAM_TWO, &[(LANE_C, "main-c")]),
        seated(TEAM_THREE, &[(LANE_D, "main-d")]),
    ];
    let expected = vec![
        (uuid(TEAM_THREE), vec![uuid(LANE_D)]),
        (uuid(TEAM_TWO), vec![uuid(LANE_C)]),
        (uuid(TEAM_ONE), vec![uuid(LANE_A), uuid(LANE_B)]),
    ];

    assert_eq!(
        arranged(&lanes, TEAM_THREE, TEAM_ONE, TeamDrop::Swap),
        expected,
        "the target and source exchange positions while the middle team stays put",
    );
    assert_eq!(
        arranged(&lanes, TEAM_ONE, TEAM_THREE, TeamDrop::Swap),
        expected,
        "pairwise swap has the same result in either drag direction",
    );
}

#[test]
fn only_a_team_stands_in_the_fleet_while_a_lone_seat_stays_in_the_topology() {
    // LANE_A sits alone under its own identity: the default seat every lane
    // starts in, not a team. TEAM_TWO holds one agent too, but under a distinct
    // identity, which is how "New team" and `lane add --team` create a team
    // of one. The fleet, the arrangement targets, and the Spice sidebar all read
    // the same rule, so a lone seat draws no surface while the topology still
    // restates it in full.
    let lanes = [
        lane(LANE_A, "main-a"),
        seated(TEAM_ONE, &[(LANE_B, "main-b"), (LANE_C, "main-c")]),
        seated(TEAM_TWO, &[(LANE_D, "main-d")]),
    ];

    assert_eq!(
        lanes.iter().map(LaneRow::teamed).collect::<Vec<_>>(),
        [false, true, true],
        "a lone seat under its own identity is the one row that is not a team",
    );
    let restated = topology(&lanes);
    assert_eq!(
        restated,
        vec![
            (uuid(LANE_A), vec![uuid(LANE_A)]),
            (uuid(TEAM_ONE), vec![uuid(LANE_B), uuid(LANE_C)]),
            (uuid(TEAM_TWO), vec![uuid(LANE_D)]),
        ],
        "the topology restates every seat, teamed or not",
    );
    assert_eq!(
        teams(&restated).collect::<Vec<_>>(),
        vec![uuid(TEAM_ONE), uuid(TEAM_TWO)],
        "only teams stand in the fleet and receive arrangement drops",
    );
    assert_eq!(
        teams(&topology(&[lane(LANE_A, "main-a"), lane(LANE_B, "main-b")])).count(),
        0,
        "a fleet of lone seats is an empty fleet",
    );
}

#[test]
fn a_join_gives_the_dragged_agents_to_the_target_and_keeps_its_primary_composer() {
    let lanes = [
        seated(TEAM_ONE, &[(LANE_A, "main-a")]),
        seated(TEAM_TWO, &[(LANE_C, "main-c"), (LANE_D, "main-d")]),
    ];

    let joined = arranged(&lanes, TEAM_TWO, TEAM_ONE, TeamDrop::Join);
    assert_eq!(
        joined,
        vec![(
            uuid(TEAM_ONE),
            vec![uuid(LANE_A), uuid(LANE_C), uuid(LANE_D)]
        )],
        "the target keeps its agents first and the emptied team is gone",
    );
    assert_eq!(
        joined[0].1.first(),
        Some(&uuid(LANE_A)),
        "the target's first agent stays its primary composer",
    );
}

#[test]
fn a_join_admits_six_members_and_rejects_a_seventh() {
    assert_eq!(TEAM_MEMBER_LIMIT, 6);
    let admitted = [
        seated(
            TEAM_ONE,
            &[
                (LANE_A, "main-a"),
                (LANE_B, "main-b"),
                (LANE_C, "main-c"),
                (LANE_D, "main-d"),
            ],
        ),
        seated(TEAM_TWO, &[(LANE_E, "main-e"), (LANE_F, "main-f")]),
        seated(TEAM_THREE, &[(LANE_G, "main-g")]),
    ];
    assert_eq!(
        arranged(&admitted, TEAM_TWO, TEAM_ONE, TeamDrop::Join),
        vec![
            (
                uuid(TEAM_ONE),
                vec![
                    uuid(LANE_A),
                    uuid(LANE_B),
                    uuid(LANE_C),
                    uuid(LANE_D),
                    uuid(LANE_E),
                    uuid(LANE_F),
                ],
            ),
            (uuid(TEAM_THREE), vec![uuid(LANE_G)]),
        ],
        "the receiver keeps its identity and order, then appends source members",
    );

    let over_capacity = [
        seated(
            TEAM_ONE,
            &[
                (LANE_A, "main-a"),
                (LANE_B, "main-b"),
                (LANE_C, "main-c"),
                (LANE_D, "main-d"),
                (LANE_G, "main-g"),
            ],
        ),
        seated(TEAM_TWO, &[(LANE_E, "main-e"), (LANE_F, "main-f")]),
    ];
    assert_eq!(
        arrangement(
            &topology(&over_capacity),
            uuid(TEAM_TWO),
            uuid(TEAM_ONE),
            TeamDrop::Join,
        ),
        None,
        "an interactive join that would create a seventh member sends nothing",
    );
}

#[test]
fn a_drop_that_states_nothing_sends_nothing() {
    let lanes = [
        seated(TEAM_ONE, &[(LANE_A, "main-a")]),
        seated(TEAM_TWO, &[(LANE_C, "main-c")]),
    ];

    assert_eq!(
        arrangement(
            &topology(&lanes),
            uuid(TEAM_ONE),
            uuid(TEAM_ONE),
            TeamDrop::Swap,
        ),
        None,
        "a team dropped on itself is not an arrangement",
    );
    assert_eq!(
        arrangement(
            &topology(&lanes),
            uuid(TEAM_THREE),
            uuid(TEAM_ONE),
            TeamDrop::Swap,
        ),
        None,
        "a removed source cannot commit a stale pointer session",
    );
    assert_eq!(
        arrangement(
            &topology(&lanes),
            uuid(TEAM_ONE),
            uuid(TEAM_THREE),
            TeamDrop::Join,
        ),
        None,
        "a removed receiver cannot accept a stale drop",
    );
    assert_eq!(
        arrangement(
            &topology(&lanes),
            uuid(LANE_A),
            uuid(TEAM_TWO),
            TeamDrop::Join,
        ),
        None,
        "one member of a team is not the team, and drags nothing",
    );
}

#[test]
fn the_gutter_that_joins_is_the_outer_fifth_of_the_team_under_the_pointer() {
    assert_eq!(TEAM_JOIN_GUTTER, 0.2);
    assert_eq!(TeamDrop::at(0.0, 100.0), TeamDrop::Join);
    assert_eq!(TeamDrop::at(20.0, 100.0), TeamDrop::Join);
    assert_eq!(TeamDrop::at(20.001, 100.0), TeamDrop::Swap);
    assert_eq!(TeamDrop::at(50.0, 100.0), TeamDrop::Swap);
    assert_eq!(TeamDrop::at(79.999, 100.0), TeamDrop::Swap);
    assert_eq!(TeamDrop::at(80.0, 100.0), TeamDrop::Join);
    assert_eq!(TeamDrop::at(100.0, 100.0), TeamDrop::Join);
    assert_eq!(TeamDrop::at(0.0, 0.0), TeamDrop::Swap);
    assert_eq!(TeamDrop::at(-5.0, 0.0), TeamDrop::Swap);
}
