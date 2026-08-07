use app::runtime::{LaneMember, LaneRow};
use app::watchband::{
    ACCENTS, FIELD_BUDGET, LINEAGE_JOIN_GUTTER, LineageDrop, SHORT_LEN, accent, arrangement, badge,
    elapsed_label, first_nonquoted_prose, leaf, one_line, reported_tasks, short, standing_title,
    standings, ticker_line, topology, truncate,
};
use wire::CanonicalUuid;
use wire::intent::{BoardTask, LaneInbox};

const LANE_A: &str = "11111111-1111-4111-8111-111111111111";
const LANE_B: &str = "22222222-2222-4222-8222-222222222222";
const LANE_C: &str = "66666666-6666-4666-8666-666666666666";
const LANE_D: &str = "77777777-7777-4777-8777-777777777777";
const LINEAGE_ONE: &str = "88888888-8888-4888-8888-888888888888";
const LINEAGE_TWO: &str = "99999999-9999-4999-8999-999999999999";
const LINEAGE_THREE: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const OCCUPANT: &str = "33333333-3333-4333-8333-333333333333";
const EPOCH: &str = "44444444-4444-4444-8444-444444444444";
const TASK_UUID: &str = "55555555-5555-4555-8555-555555555555";
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

fn lane(id: &str, worktree: &str) -> LaneRow {
    LaneRow {
        lane: uuid(id),
        subscription: wire::intent::LineageLife::Steer,
        members: vec![member(id, worktree)],
    }
}

fn member(id: &str, worktree: &str) -> LaneMember {
    LaneMember {
        lane: uuid(id),
        name: None,
        worktree: worktree.to_owned(),
        life: "docker".to_owned(),
        vendor: "codex".parse().expect("the fixture spells a vendor name"),
        model: Some("gpt-5.2-codex".to_owned()),
        effort: Some("high".to_owned()),
        occupant: Some(uuid(OCCUPANT)),
        session: Some("session-fixture".to_owned()),
        disposition: "seated".to_owned(),
        liveness: wire::intent::LaneLiveness::Running,
        // A seat that has run: `bound` records that a resumable session was once
        // declared for it, which is true of every lane this fixture builds
        // because each one is occupied. No standing reads the field, so the
        // value is the fixture's stance rather than anything asserted below --
        // spelled out because a bare `true` reads as load-bearing when it is not.
        bound: true,
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
        "pending": pending,
    }))
    .expect("the fixture spells a lane inbox")
}

/// A board task whose claim names `lane` and whose lease began `held` ago.
fn task(handle: &str, phase: &str, lane: &str, fact: u64, disposition: &str) -> BoardTask {
    serde_json::from_value(serde_json::json!({
        "visibility": {"kind": "public"},
        "origin_thread": "operator",
        "presentation_title": "Rebuild the watchband",
        "statement": {
            "position": 1,
            "body": {
                "handle": handle,
                "uuid": TASK_UUID,
                "project": "canvas.fidelity",
                "tags": [],
                "dependencies": [],
                "provenance": format!("task:{handle}"),
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

    let rows = standings(&lanes, &waiting, &tasks, at(0));

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].worktree, "main-c");
    assert_eq!(rows[0].vendor, "codex");
    assert_eq!(rows[0].model.as_deref(), Some("gpt-5.2-codex"));
    assert_eq!(rows[0].effort.as_deref(), Some("high"));
    assert_eq!(rows[0].thread, Some(uuid(OCCUPANT)));
    assert_eq!(rows[0].session.as_deref(), Some("session-fixture"));
    assert_eq!(rows[0].life, "docker");
    assert_eq!(rows[0].disposition, "seated");
    assert_eq!(rows[0].liveness, wire::intent::LaneLiveness::Running);
    assert_eq!(rows[0].pending, 2);
    assert_eq!(rows[0].handle.as_deref(), Some("FIDELIT-1kKRSfyV"));
    assert_eq!(rows[0].phase.as_deref(), Some("review"));

    // The second lane is deliberately unlike the first: no inbox row and no
    // claim, so a join that returned the first match for every lane fails here
    // rather than passing on two identical rows.
    assert_eq!(rows[1].worktree, "main-g");
    assert_eq!(rows[1].pending, 0);
    assert_eq!(rows[1].headline, None);
    assert_eq!(rows[1].handle, None);
    assert_eq!(rows[1].phase, None);
    assert_eq!(rows[1].held, None);
}

#[test]
fn one_lineage_emits_one_standing_per_member_without_losing_the_lineage_identity() {
    let lineage = LaneRow {
        lane: uuid(EPOCH),
        subscription: wire::intent::LineageLife::Steer,
        members: vec![member(LANE_A, "main-a"), member(LANE_B, "main-b")],
    };

    let rows = standings(&[lineage], &[], &[], at(0));

    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row.lineage_id == uuid(EPOCH)));
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

    let rows = standings(&lanes, &waiting, &[], at(0));

    assert_eq!(rows[0].headline.as_deref(), Some("newer author words"));
}

#[test]
fn absent_producer_preview_does_not_reinterpret_the_directive_text() {
    let lanes = [lane(LANE_A, "main-c")];
    let waiting = [inbox(LANE_A, &[("text the client must not parse", None)])];

    let rows = standings(&lanes, &waiting, &[], at(0));

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

    let rows = standings(&lanes, &[], &tasks, at(0));

    assert_eq!(rows[0].handle.as_deref(), Some("NEWER-1kKRSfyV"));
    assert_eq!(rows[0].phase.as_deref(), Some("review"));
}

#[test]
fn the_lease_measures_holding_and_renewal_separately() {
    let lanes = [lane(LANE_A, "main-c")];
    let tasks = [task("FIDELIT-1kKRSfyV", "review", LANE_A, 7, "active")];

    let rows = standings(&lanes, &[], &tasks, at(0));

    assert_eq!(rows[0].held, Some(300_000));
    assert_eq!(rows[0].renewed, Some(4_000));
    assert_eq!(elapsed_label(rows[0].held.expect("the lease began")), "5m");
    assert_eq!(
        elapsed_label(rows[0].renewed.expect("the lease renewed")),
        "4s"
    );
}

#[test]
fn a_lane_keeps_its_accent_when_the_table_reorders() {
    let forward = [lane(LANE_A, "main-c"), lane(LANE_B, "main-g")];
    let reversed = [lane(LANE_B, "main-g"), lane(LANE_A, "main-c")];

    let first = standings(&forward, &[], &[], at(0));
    let second = standings(&reversed, &[], &[], at(0));

    // An accent read from the row index would move with the reorder, and one
    // fixed for every lane would survive the reorder but collapse the two
    // lanes onto one slot; the pair of claims admits neither.
    assert_eq!(first[0].accent, second[1].accent);
    assert_eq!(first[1].accent, second[0].accent);
    assert_ne!(first[0].accent, first[1].accent);
    assert!(first.iter().all(|row| row.accent < ACCENTS));
    assert!(accent(uuid(LANE_A)) < ACCENTS);
}

#[test]
fn a_lane_is_named_by_the_identity_the_resident_gave_it() {
    let rows = standings(&[lane(LANE_A, "main-c")], &[], &[], at(0));

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

    let rows = standings(&lanes, &[], &[], at(0));

    assert_eq!(rows[0].label, "spice-rust-a");
    assert_eq!(rows[1].label, "spice-rust-g");
    assert_eq!(rows[0].worktree, "/fleet/one/spice-rust-a");
    assert!(
        rows.iter()
            .all(|row| row.label.chars().count() <= FIELD_BUDGET)
    );
    assert_eq!(leaf("main-c"), "main-c");
    assert_eq!(leaf("/"), "/");
}

#[test]
fn a_configured_agent_name_wins_while_an_unconfigured_lane_uses_the_worktree_leaf() {
    let mut configured = lane(LANE_A, "/fleet/one/spice-rust-a");
    configured.members[0].name = Some("Ava".to_owned());
    let unconfigured = lane(LANE_B, "/fleet/one/spice-rust-g");

    let rows = standings(&[configured, unconfigured], &[], &[], at(0));

    assert_eq!(rows[0].label, "Ava");
    assert_eq!(rows[0].worktree, "/fleet/one/spice-rust-a");
    assert_eq!(rows[1].label, "spice-rust-g");
    assert_eq!(rows[1].worktree, "/fleet/one/spice-rust-g");
    assert_eq!(rows[0].pending, 0, "pending remains an inbox-derived join");
}

#[test]
fn configured_agent_names_distinguish_two_lanes_that_share_one_worktree() {
    let mut ava = lane(LANE_A, "/fleet/shared");
    ava.members[0].name = Some("Ava".to_owned());
    let mut matilda = lane(LANE_B, "/fleet/shared");
    matilda.members[0].name = Some("Matilda".to_owned());

    let rows = standings(&[ava, matilda], &[], &[], at(0));

    assert_eq!(rows[0].label, "Ava");
    assert_eq!(rows[1].label, "Matilda");
    assert_eq!(rows[0].worktree, rows[1].worktree);
}

#[test]
fn a_standing_title_names_recorded_and_unrecorded_launches_without_substitution() {
    let mut standing = standings(&[lane(LANE_A, "/fleet/one/spice-rust-a")], &[], &[], at(0))
        .pop()
        .expect("fixture standing");
    assert_eq!(
        standing_title(&standing),
        format!(
            "Codex driver; model: gpt-5.2-codex; effort: high; thread: {OCCUPANT}; \
             session: session-fixture"
        )
    );

    standing.model = None;
    standing.effort = None;
    assert_eq!(
        standing_title(&standing),
        format!(
            "Codex driver; model: unrecorded; effort: unrecorded; thread: {OCCUPANT}; \
             session: session-fixture"
        )
    );
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
fn an_entirely_quoted_message_has_no_composer_prose() {
    assert_eq!(
        first_nonquoted_prose("> borrowed\n> > borrowed deeper\n  > borrowed after whitespace"),
        None
    );
    assert_eq!(first_nonquoted_prose(" \r\n\t\r\n"), None);
}

#[test]
fn a_pill_wears_no_badge_at_zero() {
    assert_eq!(badge(0), None);
    assert_eq!(badge(4).as_deref(), Some("4"));
    assert_eq!(badge(27).as_deref(), Some("27"));
}

#[test]
fn authored_prose_reports_each_canonical_task_once_through_markdown_punctuation() {
    let tasks = reported_tasks(
        "Settled `CONTROL-1kKbC3j4`; follow APP-a1 (CONTROL-1kKbC3j4 again). \
         lower-a1 TASK- TASK-a-b and ordinary-hyphenated-prose are not handles.",
    );

    assert_eq!(
        tasks.iter().map(ToString::to_string).collect::<Vec<_>>(),
        ["CONTROL-1kKbC3j4", "APP-a1"],
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

/// One team holding several agents, so a join has more than one agent to move.
fn seated(id: &str, members: &[(&str, &str)]) -> LaneRow {
    LaneRow {
        lane: uuid(id),
        subscription: wire::intent::LineageLife::Steer,
        members: members
            .iter()
            .map(|(lane, worktree)| member(lane, worktree))
            .collect(),
    }
}

fn arranged(
    lanes: &[LaneRow],
    dragged: &str,
    target: &str,
    drop: LineageDrop,
) -> Vec<(CanonicalUuid, Vec<CanonicalUuid>)> {
    arrangement(&topology(lanes), uuid(dragged), uuid(target), drop)
        .expect("a drop of one live team onto another states an arrangement")
}

#[test]
fn a_reorder_moves_the_dragged_lineage_into_the_target_position_without_moving_an_agent() {
    // No team is named for one of its own members: a team read off its first
    // member would arrange the same three teams into the same three orders.
    let lanes = [
        seated(LINEAGE_ONE, &[(LANE_A, "main-a"), (LANE_B, "main-b")]),
        seated(LINEAGE_TWO, &[(LANE_C, "main-c")]),
        seated(LINEAGE_THREE, &[(LANE_D, "main-d")]),
    ];

    let forward = arranged(&lanes, LINEAGE_THREE, LINEAGE_ONE, LineageDrop::Reorder);
    assert_eq!(
        forward,
        vec![
            (uuid(LINEAGE_THREE), vec![uuid(LANE_D)]),
            (uuid(LINEAGE_ONE), vec![uuid(LANE_A), uuid(LANE_B)]),
            (uuid(LINEAGE_TWO), vec![uuid(LANE_C)]),
        ],
        "the dragged team takes the target's position and every agent stays put",
    );

    // Dragging the other way is not the mirror image: the dragged team still
    // lands where the target stood, which for a forward drag is one position
    // later than the target's index in the arrangement it was read from.
    let backward = arranged(&lanes, LINEAGE_ONE, LINEAGE_THREE, LineageDrop::Reorder);
    assert_eq!(
        backward,
        vec![
            (uuid(LINEAGE_TWO), vec![uuid(LANE_C)]),
            (uuid(LINEAGE_ONE), vec![uuid(LANE_A), uuid(LANE_B)]),
            (uuid(LINEAGE_THREE), vec![uuid(LANE_D)]),
        ],
    );
}

#[test]
fn a_join_gives_the_dragged_agents_to_the_target_and_keeps_its_primary_composer() {
    let lanes = [
        seated(LINEAGE_ONE, &[(LANE_A, "main-a")]),
        seated(LINEAGE_TWO, &[(LANE_C, "main-c"), (LANE_D, "main-d")]),
    ];

    let joined = arranged(&lanes, LINEAGE_TWO, LINEAGE_ONE, LineageDrop::Join);
    assert_eq!(
        joined,
        vec![(
            uuid(LINEAGE_ONE),
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
fn a_drop_that_states_nothing_sends_nothing() {
    let lanes = [
        seated(LINEAGE_ONE, &[(LANE_A, "main-a")]),
        seated(LINEAGE_TWO, &[(LANE_C, "main-c")]),
    ];

    assert_eq!(
        arrangement(
            &topology(&lanes),
            uuid(LINEAGE_ONE),
            uuid(LINEAGE_ONE),
            LineageDrop::Reorder
        ),
        None,
        "a team dropped on itself is not an arrangement",
    );
    assert_eq!(
        arrangement(
            &topology(&lanes),
            uuid(LINEAGE_THREE),
            uuid(LINEAGE_ONE),
            LineageDrop::Join
        ),
        None,
        "a team the surface no longer shows cannot be dragged",
    );
    assert_eq!(
        arrangement(
            &topology(&lanes),
            uuid(LINEAGE_ONE),
            uuid(LINEAGE_THREE),
            LineageDrop::Join
        ),
        None,
        "a team the surface no longer shows cannot be dropped on",
    );
    assert_eq!(
        arrangement(
            &topology(&lanes),
            uuid(LANE_A),
            uuid(LINEAGE_TWO),
            LineageDrop::Join
        ),
        None,
        "one member of a team is not the team, and drags nothing",
    );
}

#[test]
fn the_gutter_that_joins_is_the_outer_fifth_of_the_lineage_under_the_pointer() {
    assert_eq!(LINEAGE_JOIN_GUTTER, 0.2);
    assert_eq!(LineageDrop::at(0.0, 100.0), LineageDrop::Join);
    assert_eq!(LineageDrop::at(20.0, 100.0), LineageDrop::Join);
    assert_eq!(LineageDrop::at(20.001, 100.0), LineageDrop::Reorder);
    assert_eq!(LineageDrop::at(50.0, 100.0), LineageDrop::Reorder);
    assert_eq!(LineageDrop::at(79.999, 100.0), LineageDrop::Reorder);
    assert_eq!(LineageDrop::at(80.0, 100.0), LineageDrop::Join);
    assert_eq!(LineageDrop::at(100.0, 100.0), LineageDrop::Join);
    // A team with no width has no gutter, so it can only be reordered. The
    // offset outside it is the case that needs stating: dividing by a zero
    // width there is an infinite fraction, which lands in a gutter.
    assert_eq!(LineageDrop::at(0.0, 0.0), LineageDrop::Reorder);
    assert_eq!(LineageDrop::at(-5.0, 0.0), LineageDrop::Reorder);
}
