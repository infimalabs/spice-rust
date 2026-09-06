use app::runtime::{LaneMember, LaneRow};
use app::watchband::{
    ACCENTS, AGE_LABEL_WIDTH, FIELD_BUDGET, LINEAGE_JOIN_GUTTER, LineageDrop, SHORT_LEN,
    TeamTaskSectionState, accent, arrangement, badge, elapsed_label, elapsed_label_deadline_millis,
    first_nonquoted_prose, leaf, one_line, remaining_label, remaining_label_deadline_millis,
    reported_tasks, short, split_lineage, standing_title, standings, task_mentions, ticker_line,
    topology, truncate,
};
use wire::CanonicalUuid;
use wire::intent::{BoardTask, LaneInbox};

const LANE_A: &str = "11111111-1111-4111-8111-111111111111";
const LANE_B: &str = "22222222-2222-4222-8222-222222222222";
const LANE_C: &str = "66666666-6666-4666-8666-666666666666";
const LANE_D: &str = "77777777-7777-4777-8777-777777777777";
const LANE_E: &str = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const LANE_F: &str = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
const LANE_G: &str = "dddddddd-dddd-4ddd-8ddd-dddddddddddd";
const LINEAGE_ONE: &str = "88888888-8888-4888-8888-888888888888";
const LINEAGE_TWO: &str = "99999999-9999-4999-8999-999999999999";
const LINEAGE_THREE: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
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

#[test]
fn team_task_sections_distinguish_unavailable_empty_and_ready_snapshots() {
    let active_unavailable = TeamTaskSectionState::active(false, 0);
    let direction_ready = TeamTaskSectionState::direction(true, 2);
    assert_eq!(
        active_unavailable.message(),
        Some("Task snapshot unavailable.")
    );
    assert!(active_unavailable.unavailable());
    assert_eq!(direction_ready.message(), None);
    assert!(!direction_ready.unavailable());

    let active_ready = TeamTaskSectionState::active(true, 1);
    let direction_unavailable = TeamTaskSectionState::direction(false, 0);
    assert_eq!(active_ready.message(), None);
    assert_eq!(
        direction_unavailable.message(),
        Some("Direction snapshot unavailable.")
    );
    assert!(direction_unavailable.unavailable());

    assert_eq!(
        TeamTaskSectionState::active(true, 0).message(),
        Some("No active tasks for this team.")
    );
    assert_eq!(
        TeamTaskSectionState::direction(true, 0).message(),
        Some("No queued direction.")
    );
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
                "project": "canvas.fidelity",
                "tags": [],
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
fn one_lineage_emits_one_standing_per_member_without_losing_the_lineage_identity() {
    let lineage = LaneRow {
        lane: uuid(EPOCH),
        subscription: wire::intent::LineageLife::Steer,
        members: vec![member(LANE_A, "main-a"), member(LANE_B, "main-b")],
    };

    let rows = standings(&[lineage], &[], &[]);

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
/// The second half is the part that matters to a quiet canvas. One row, built
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
fn lineage_members_receive_the_complete_palette_from_the_right_edge() {
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

    let complete_palette = (0..ACCENTS).rev().collect::<Vec<_>>();
    assert_eq!(
        first.iter().map(|row| row.accent).collect::<Vec<_>>(),
        complete_palette
    );
    assert_eq!(
        second.iter().map(|row| row.accent).collect::<Vec<_>>(),
        complete_palette
    );
    assert_eq!(first[0].lane, second[5].lane);
    assert_eq!(first[0].accent, 5);
    assert_eq!(
        second[5].accent, 0,
        "a member reordered to the right edge receives the green slot"
    );
    for members in 1..=ACCENTS + 1 {
        assert_eq!(
            accent(members, members - 1),
            0,
            "the rightmost member starts at green for a lineage of {members}"
        );
    }
    assert_eq!(
        (0..=ACCENTS)
            .map(|position| accent(ACCENTS + 1, position))
            .collect::<Vec<_>>(),
        vec![0, 5, 4, 3, 2, 1, 0],
        "the seventh member wraps at the far left without moving the right anchor"
    );
}

#[test]
fn the_stylesheet_matches_the_reference_accent_palette() {
    const STYLESHEET: &str = include_str!("../style.css");
    const LIGHT: [&str; ACCENTS] = [
        "#1b4332", "#4f63b5", "#8b5f00", "#b3402a", "#007c89", "#8a4fbf",
    ];
    const DARK: [&str; ACCENTS] = [
        "#95d5b2", "#8ea2ff", "#f2c14e", "#e0734f", "#5fd6d0", "#d1a3ff",
    ];

    let root = STYLESHEET
        .split_once(":root {")
        .expect("the stylesheet declares the light root")
        .1
        .split_once('}')
        .expect("the light root is closed")
        .0;
    let dark = STYLESHEET
        .split_once("@media (prefers-color-scheme: dark) {")
        .expect("the stylesheet declares a dark palette")
        .1
        .split_once(":root {")
        .expect("the dark media query declares a root")
        .1
        .split_once('}')
        .expect("the dark root is closed")
        .0;

    for (palette, expected) in [(root, LIGHT), (dark, DARK)] {
        let actual = palette
            .lines()
            .filter_map(|line| {
                let declaration = line.trim().strip_prefix("--accent-")?;
                let (slot, value) = declaration.split_once(':')?;
                Some((
                    slot.parse::<usize>().ok()?,
                    value.trim().trim_end_matches(';'),
                ))
            })
            .collect::<Vec<_>>();
        let expected = expected.into_iter().enumerate().collect::<Vec<_>>();

        assert_eq!(actual, expected, "the ordered accent palette drifted");
    }
}

#[test]
fn wrapped_lineage_accents_do_not_replace_member_identity() {
    let lineage = seated(
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

    let rows = standings(&[lineage], &[], &[]);

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

    let rows = standings(&[configured, unconfigured], &[], &[]);

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
            "Codex vendor; model: gpt-5.2-codex; effort: high; thread: {OCCUPANT}; \
             session: session-fixture"
        )
    );

    standing.model = None;
    standing.effort = None;
    assert_eq!(
        standing_title(&standing),
        format!(
            "Codex vendor; model: unrecorded; effort: unrecorded; thread: {OCCUPANT}; \
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

/// A ticking age may only change text, never the box the text sits in. The box
/// is sized once from `AGE_LABEL_WIDTH`, so that promise is only as good as the
/// label's inability to outgrow it — a single wider return reflows the slot and
/// every card beside it.
///
/// The sweep is what makes this more than a spot check. It walks every second
/// across the first two days, so it crosses each unit boundary the operator
/// actually watches, then jumps to the inputs no operator produces on purpose:
/// an unset instant read against the browser epoch, and the saturating end of
/// the type. Those are the ones that used to escape — the day arm counted up
/// without a ceiling, so `i64::MAX` spelled a thirteen-character label.
#[test]
fn age_labels_never_outgrow_the_slot() {
    let sweep = (0..=172_800).map(|second| second * 1_000);
    let extremes = [i64::MIN, -1, 1_754_000_000_000, i64::MAX - 1, i64::MAX];
    for millis in sweep.chain(extremes) {
        let label = elapsed_label(millis);
        assert!(
            label.chars().count() <= AGE_LABEL_WIDTH,
            "elapsed_label({millis}) is {:?}, wider than the {AGE_LABEL_WIDTH}-character slot",
            label
        );
        assert!(
            !label.contains(char::is_whitespace),
            "elapsed_label({millis}) is {label:?}, which can wrap out of a single line"
        );
        let remaining = remaining_label(millis);
        assert!(
            remaining.chars().count() <= AGE_LABEL_WIDTH,
            "remaining_label({millis}) is {remaining:?}, wider than the {AGE_LABEL_WIDTH}-character slot"
        );
        assert!(
            !remaining.contains(char::is_whitespace),
            "remaining_label({millis}) is {remaining:?}, which can wrap out of a single line"
        );
    }
}

/// The saturating arm has to be legible as saturation, not misread as a real
/// count, and it has to begin exactly one day after the last true count.
#[test]
fn the_day_arm_saturates_rather_than_widening() {
    assert_eq!(elapsed_label(99 * 86_400_000), "99d");
    assert_eq!(elapsed_label(100 * 86_400_000), "99d+");
    assert_eq!(elapsed_label(i64::MAX), "99d+");
}

/// `age_labels_never_outgrow_the_slot` proves the label fits the budget; this
/// proves the budget is the one the browser actually applies.
///
/// The canvas is served under `style-src 'self'`, so the component cannot hand
/// the width to the page as an inline style computed from `AGE_LABEL_WIDTH` —
/// the browser refuses the attribute and the slot silently loses its floor. The
/// number therefore lives twice, once in Rust and once written out by hand in
/// the stylesheet, and two copies of a number drift. This reads the stylesheet
/// back at test time and refuses the drift from either side: widening the label
/// budget without widening the rule fails here, and so does editing the rule.
///
/// Deleting the declaration is a failure too, not a quiet pass — each step of
/// the parse below expects what it looks for, so an absent rule, an absent
/// `min-width` or a width expressed in some other unit all fail loudly rather
/// than leaving nothing to compare.
#[test]
fn the_stylesheet_sizes_the_age_slot_from_the_label_budget() {
    const STYLESHEET: &str = include_str!("../style.css");

    let rule = STYLESHEET
        .split_once("[data-age-slot] {")
        .expect("the stylesheet has a rule sizing the age slot")
        .1
        .split_once('}')
        .expect("the age slot rule is closed")
        .0;
    let declared = rule
        .lines()
        .find_map(|line| line.trim().strip_prefix("min-width:"))
        .expect("the age slot rule floors the slot with a min-width")
        .trim()
        .trim_end_matches(';')
        .trim()
        // Character widths rather than pixels because the budget being bound is
        // counted in characters; any other unit would compare two different
        // things and only look like agreement.
        .strip_suffix("ch")
        .expect("the age slot is sized in character widths")
        .parse::<usize>()
        .expect("the age slot width is a whole number of characters");

    assert_eq!(
        declared, AGE_LABEL_WIDTH,
        "app/style.css floors the age slot at {declared}ch, but elapsed_label is \
         bounded to {AGE_LABEL_WIDTH} characters"
    );
}

#[test]
fn the_command_bar_is_not_delineated_from_the_canvas() {
    for (stylesheet, selector) in [
        (include_str!("../style.css"), ".commandbar {"),
        (include_str!("../chrome.css"), ".commandbar--mosaic {"),
        (include_str!("../responsive.css"), ".commandbar {"),
    ] {
        let rule = stylesheet
            .split_once(selector)
            .unwrap_or_else(|| panic!("the stylesheet has a {selector} rule"))
            .1
            .split_once('}')
            .unwrap_or_else(|| panic!("the {selector} rule is closed"))
            .0;

        assert!(
            !rule.contains("border-bottom"),
            "the {selector} rule delineates the header from the canvas"
        );
    }
}

#[test]
fn agent_surfaces_have_one_bounded_vertical_ownership_chain() {
    const STYLESHEET: &str = include_str!("../mosaic.css");

    for selector in [
        ".mosaic-workspace .content {",
        ".lineage-fleet {",
        ".team-pane {",
        ".team-compose {",
        ".team-activity-grid {",
        "\n.lane-mosaic-plane {",
    ] {
        let rule = STYLESHEET
            .split_once(selector)
            .unwrap_or_else(|| panic!("the stylesheet has a {selector} rule"))
            .1
            .split_once('}')
            .unwrap_or_else(|| panic!("the {selector} rule is closed"))
            .0;

        assert!(
            rule.contains("flex:") && rule.contains("min-height: 0"),
            "the {selector} rule participates in the bounded vertical flex chain"
        );
    }

    let fleet = STYLESHEET
        .split_once(".lineage-fleet {")
        .expect("mosaic.css should define the lineage fleet")
        .1
        .split_once('}')
        .expect("the lineage fleet rule is closed")
        .0;
    assert!(fleet.contains("flex-flow: row nowrap"));
    assert!(fleet.contains("overflow: auto hidden"));
    let surface = STYLESHEET
        .split_once(".team-surface {")
        .expect("mosaic.css should define team surfaces")
        .1
        .split_once('}')
        .expect("the team surface rule is closed")
        .0;
    assert!(surface.contains("flex:") && surface.contains("min-height: 100%"));

    let plane = STYLESHEET
        .split_once("\n.lane-mosaic-plane {")
        .expect("mosaic.css should define the lane Mosaic plane")
        .1
        .split_once('}')
        .expect("the lane Mosaic plane rule should be closed")
        .0;
    assert!(
        plane.contains("overflow-y: auto") && plane.contains("max-height: none"),
        "the desktop lane plane alone owns long-history scrolling"
    );

    let responsive = include_str!("../responsive.css");
    assert!(
        responsive.contains("@media (max-height: 440px)")
            && !responsive.contains(".team-surface:nth-child(2)"),
        "only an unsatisfiable block size returns lanes to intrinsic document flow"
    );
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
fn authored_prose_reports_only_canonical_base52_tasks_once_through_punctuation() {
    for candidate in [
        "APP-a1",
        "OVERFLOW-000000000",
        "WRONG-1kKAC3j4",
        "SHAPED-abcdefgh",
    ] {
        let broad = candidate
            .parse::<wire::TaskHandle>()
            .expect("fixture deliberately matches the compatibility handle shape");
        assert!(
            broad.born().is_err(),
            "{candidate} must fail canonical task-stamp admission"
        );
    }

    let text = "Settled `CONTROL-1kKbC3j4` (CONTROL-1kKbC3j4 again). APP-a1 is short; \
                OVERFLOW-000000000 is wide; WRONG-1kKAC3j4 uses the wrong alphabet; \
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
        ],
        "the parser-backed scanner retains only canonical inline locations",
    );

    let tasks = reported_tasks(text);

    assert_eq!(
        tasks.iter().map(ToString::to_string).collect::<Vec<_>>(),
        ["CONTROL-1kKbC3j4"],
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

    let backward = arranged(&lanes, LINEAGE_THREE, LINEAGE_ONE, LineageDrop::Reorder);
    assert_eq!(
        backward,
        vec![
            (uuid(LINEAGE_THREE), vec![uuid(LANE_D)]),
            (uuid(LINEAGE_ONE), vec![uuid(LANE_A), uuid(LANE_B)]),
            (uuid(LINEAGE_TWO), vec![uuid(LANE_C)]),
        ],
        "the dragged team takes the target's position and every agent stays put",
    );

    // Dragging the other way is not the mirror image. A reorder always inserts
    // the dragged team immediately before the target, which only coincides with
    // the target's own index when the drag came from later in the row: here
    // LINEAGE_THREE stood third, and LINEAGE_ONE lands second, because removing
    // it first shifts the target one place earlier than the row it was read
    // from. Asserting only the backward case would let "insert before" and
    // "take the target's index" pass as the same rule.
    let forward = arranged(&lanes, LINEAGE_ONE, LINEAGE_THREE, LineageDrop::Reorder);
    assert_eq!(
        forward,
        vec![
            (uuid(LINEAGE_TWO), vec![uuid(LANE_C)]),
            (uuid(LINEAGE_ONE), vec![uuid(LANE_A), uuid(LANE_B)]),
            (uuid(LINEAGE_THREE), vec![uuid(LANE_D)]),
        ],
    );
}

#[test]
fn an_after_reorder_places_the_dragged_lineage_after_its_target() {
    let lanes = [
        seated(LINEAGE_ONE, &[(LANE_A, "main-a")]),
        seated(LINEAGE_TWO, &[(LANE_C, "main-c")]),
        seated(LINEAGE_THREE, &[(LANE_D, "main-d")]),
    ];

    let arranged = arranged(&lanes, LINEAGE_ONE, LINEAGE_TWO, LineageDrop::ReorderAfter);
    assert_eq!(
        arranged,
        vec![
            (uuid(LINEAGE_TWO), vec![uuid(LANE_C)]),
            (uuid(LINEAGE_ONE), vec![uuid(LANE_A)]),
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
fn a_split_keeps_the_source_identity_and_makes_every_member_individual() {
    let lanes = [
        seated(LINEAGE_ONE, &[(LANE_A, "main-a"), (LANE_B, "main-b")]),
        seated(LINEAGE_TWO, &[(LANE_C, "main-c")]),
    ];

    assert_eq!(
        split_lineage(&topology(&lanes), uuid(LINEAGE_ONE)),
        Some(vec![
            (uuid(LINEAGE_ONE), vec![uuid(LANE_A)]),
            (uuid(LANE_B), vec![uuid(LANE_B)]),
            (uuid(LINEAGE_TWO), vec![uuid(LANE_C)]),
        ]),
        "the source remains restorable while every member becomes one team",
    );
    assert_eq!(
        split_lineage(&topology(&lanes), uuid(LINEAGE_TWO)),
        None,
        "an individual team has nothing to split",
    );
    assert_eq!(
        split_lineage(&topology(&lanes), uuid(LINEAGE_THREE)),
        None,
        "a retired team cannot be split from a stale menu",
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
    // Two live teams, two distinct identities, and a gesture the up-front
    // guards all admit: only the arrangement it produces shows there is nothing
    // to state. Inserting before the neighbour it already precedes rebuilds the
    // row the canvas is showing, and committing that would write a durable fact
    // recording no change.
    assert_eq!(
        arrangement(
            &topology(&lanes),
            uuid(LINEAGE_ONE),
            uuid(LINEAGE_TWO),
            LineageDrop::Reorder
        ),
        None,
        "a team dropped in front of the team it already precedes states nothing",
    );
    assert_eq!(
        arrangement(
            &topology(&lanes),
            uuid(LINEAGE_TWO),
            uuid(LINEAGE_ONE),
            LineageDrop::ReorderAfter
        ),
        None,
        "a team moved behind the team it already follows states nothing",
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
