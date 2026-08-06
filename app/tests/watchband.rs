use app::runtime::LaneRow;
use app::watchband::{
    ACCENTS, FIELD_BUDGET, SHORT_LEN, accent, badge, elapsed_label, leaf, one_line, short,
    standings, ticker_line, truncate,
};
use wire::CanonicalUuid;
use wire::intent::{BoardTask, LaneInbox};

const LANE_A: &str = "11111111-1111-4111-8111-111111111111";
const LANE_B: &str = "22222222-2222-4222-8222-222222222222";
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
        worktree: worktree.to_owned(),
        life: "docker".to_owned(),
        vendor: "codex".parse().expect("the fixture spells a vendor name"),
        occupant: Some(uuid(OCCUPANT)),
        disposition: "seated".to_owned(),
        // A seat that has run: `bound` records that a resumable session was once
        // declared for it, which is true of every lane this fixture builds
        // because each one is occupied. No standing reads the field, so the
        // value is the fixture's stance rather than anything asserted below --
        // spelled out because a bare `true` reads as load-bearing when it is not.
        bound: true,
    }
}

fn inbox(id: &str, texts: &[&str]) -> LaneInbox {
    let pending = texts
        .iter()
        .enumerate()
        .map(|(index, text)| {
            serde_json::json!({
                "directive": OCCUPANT,
                "key": format!("key{index}"),
                "text": text,
                "expires": spell(600_000),
            })
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
    let waiting = [inbox(LANE_A, &["first direction", "second direction"])];
    let tasks = [task("FIDELIT-1kKRSfyV", "review", LANE_A, 7, "active")];

    let rows = standings(&lanes, &waiting, &tasks, at(0));

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].worktree, "main-c");
    assert_eq!(rows[0].vendor, "codex");
    assert_eq!(rows[0].life, "docker");
    assert_eq!(rows[0].disposition, "seated");
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
fn the_headline_is_the_newest_waiting_directive_on_one_line() {
    let lanes = [lane(LANE_A, "main-c")];
    let waiting = [inbox(LANE_A, &["older direction", "newer\ndirection"])];

    let rows = standings(&lanes, &waiting, &[], at(0));

    assert_eq!(rows[0].headline.as_deref(), Some("newer\\ndirection"));
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
fn a_pill_wears_no_badge_at_zero() {
    assert_eq!(badge(0), None);
    assert_eq!(badge(4).as_deref(), Some("4"));
    assert_eq!(badge(27).as_deref(), Some("27"));
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
