//! What a pane is allowed to show.
//!
//! The fixtures here are encoded documents for the same reason the transport
//! fixtures are: a pane renders what the socket delivered, and the socket
//! delivers bytes. Their rows are [`wire::body::TaskPhased`] rather than a
//! shape declared here, so these panes are proven against a row the server
//! actually mints instead of against a local stand-in that agrees with them.
//!
//! Nothing below inspects rendered text. A pane's observable surface is its
//! state — rows, overflow, horizon, and the three provenance fields — and
//! asserting against markup would pin the styling rather than the contract.

use app::{Answer, Axis, Pane, PaneState};
use wire::{Derived, Overflow, Packed, Seq, body};

/// Two ranked rows, complete through the horizon they declare.
const COMPLETE: &str = r#"{
  "rows": [
    { "task": "SPICE-aBcD", "phase": "verify" },
    { "task": "SPICE-eFgH", "phase": "review" }
  ],
  "horizon": 48213
}"#;

/// One ranked row out of many, naming what it omitted and where to resume.
const OVERFLOWING: &str = r#"{
  "rows": [
    { "task": "SPICE-aBcD", "phase": "verify" }
  ],
  "overflow": { "omitted": 41, "continuation": 48250 },
  "horizon": 48213
}"#;

/// A fold-backed answer carrying exactly [`COMPLETE`]'s rows.
///
/// `cursor` and `horizon` are spelled differently here on purpose. They answer
/// different questions — how far this generation applied, and how far the
/// declared inputs are complete — so a fixture that gave them one value would
/// let a pane read either for the other and still pass.
const DERIVED: &str = r#"{
  "fold": "board::backlog",
  "gen": 17,
  "cursor": 48200,
  "packed": {
    "rows": [
      { "task": "SPICE-aBcD", "phase": "verify" },
      { "task": "SPICE-eFgH", "phase": "review" }
    ],
    "horizon": 48213
  }
}"#;

/// A position, which every fixture spells above zero.
fn seq(value: u64) -> Seq {
    Seq::new(value).expect("fixture position is non-zero")
}

/// One packed answer, decoded off the wire rather than built in memory.
fn packed(document: &str) -> Packed<body::TaskPhased> {
    wire::decode(document.as_bytes()).expect("the fixture packed answer decodes")
}

/// One fold-backed answer, decoded off the wire.
fn derived(document: &str) -> Derived<body::TaskPhased> {
    wire::decode(document.as_bytes()).expect("the fixture derived answer decodes")
}

/// A pane holding a head-direct read of `document`.
fn head_direct(pane: Pane, document: &str) -> PaneState<body::TaskPhased> {
    PaneState::new(pane, Answer::HeadDirect(packed(document)))
}

/// A pane holding a fold-backed read of `document`.
fn fold_backed(pane: Pane, document: &str) -> PaneState<body::TaskPhased> {
    PaneState::new(pane, Answer::Derived(derived(document)))
}

/// Every pane, gathered by exhaustive descent rather than by reading `ALL`.
///
/// The match is exhaustive over [`Pane`], so a variant added to the registry
/// leaves this file failing to compile until it is placed in the chain.
/// Comparing the result against [`Pane::ALL`] is then what proves `ALL` is the
/// whole registry: a pane the enum declares but the list omits would never be
/// rendered, audited, or missed.
fn descend_panes() -> Vec<Pane> {
    std::iter::successors(Some(Pane::Backlog), |pane| match pane {
        Pane::Backlog => Some(Pane::Trust),
        Pane::Trust => Some(Pane::Phases),
        Pane::Phases => Some(Pane::Claims),
        Pane::Claims => Some(Pane::Depends),
        Pane::Depends => Some(Pane::Lanes),
        Pane::Lanes => Some(Pane::Journal),
        Pane::Journal => Some(Pane::Steer),
        Pane::Steer => None,
    })
    .collect()
}

/// Every axis, gathered the same exhaustive way for the same reason.
fn descend_axes() -> Vec<Axis> {
    std::iter::successors(Some(Axis::Magnitude), |axis| match axis {
        Axis::Magnitude => Some(Axis::Flow),
        Axis::Flow => Some(Axis::Topology),
        Axis::Topology => Some(Axis::Chronology),
        Axis::Chronology => None,
    })
    .collect()
}

#[test]
fn the_registry_lists_every_pane_the_vocabulary_declares() {
    assert_eq!(descend_panes(), Pane::ALL);
}

#[test]
fn the_view_vocabulary_is_the_four_axes_and_nothing_else() {
    assert_eq!(descend_axes(), Axis::ALL);
    assert_eq!(Axis::ALL.len(), 4);
}

#[test]
fn every_pane_stands_on_exactly_one_of_the_four_axes() {
    for pane in Pane::ALL {
        let axis = pane.axis();

        // `axis()` returns one `Axis` rather than a set, so "exactly one" is
        // carried by the type. What remains to check is that the one it names
        // is a word the vocabulary actually admits.
        assert!(
            Axis::ALL.contains(&axis),
            "{} stands on {:?}, which is not one of the four axes",
            pane.as_str(),
            axis
        );
    }
}

#[test]
fn every_axis_is_rendered_by_at_least_one_pane() {
    for axis in Axis::ALL {
        assert!(
            Pane::ALL.iter().any(|pane| pane.axis() == *axis),
            "{} is a declared axis that no pane renders",
            axis.as_str()
        );
    }
}

#[test]
fn no_two_panes_share_a_spelling() {
    for (rank, pane) in Pane::ALL.iter().enumerate() {
        for other in &Pane::ALL[rank + 1..] {
            assert_ne!(
                pane.as_str(),
                other.as_str(),
                "{pane:?} and {other:?} are two panes under one name"
            );
        }
    }
}

#[test]
fn no_two_axes_share_a_spelling() {
    for (rank, axis) in Axis::ALL.iter().enumerate() {
        for other in &Axis::ALL[rank + 1..] {
            assert_ne!(
                axis.as_str(),
                other.as_str(),
                "{axis:?} and {other:?} are two axes under one name"
            );
        }
    }
}

#[test]
fn a_pane_renders_the_rows_the_answer_carried_in_the_order_they_were_ranked() {
    let state = head_direct(Pane::Phases, COMPLETE);

    let tasks = state
        .rows()
        .iter()
        .map(|row| row.task.as_str())
        .collect::<Vec<_>>();

    assert_eq!(tasks, ["SPICE-aBcD", "SPICE-eFgH"]);
    assert_eq!(state.rows()[0].phase.as_str(), "verify");
    assert_eq!(state.horizon(), seq(48213));
}

#[test]
fn a_pane_renders_the_overflow_marker_the_answer_declared() {
    let state = head_direct(Pane::Backlog, OVERFLOWING);

    assert!(state.is_overflowing());
    assert_eq!(
        state.overflow(),
        Some(Overflow {
            omitted: 41,
            continuation: seq(48250),
        })
    );
}

#[test]
fn an_answer_that_omitted_nothing_declares_no_overflow() {
    let state = head_direct(Pane::Backlog, COMPLETE);

    assert!(!state.is_overflowing());
    assert_eq!(state.overflow(), None);
    assert_eq!(state.rows().len(), 2);
}

#[test]
fn truncation_is_read_from_the_answer_rather_than_inferred_from_row_count() {
    let short = head_direct(Pane::Backlog, OVERFLOWING);
    let long = head_direct(Pane::Backlog, COMPLETE);

    // The overflowing answer carries fewer rows than the complete one, so any
    // pane that guessed at truncation by comparing a row count against a page
    // size would have these two backwards.
    assert!(short.rows().len() < long.rows().len());
    assert!(short.is_overflowing());
    assert!(!long.is_overflowing());
}

#[test]
fn a_derived_answer_renders_its_fold_generation_and_cursor() {
    let state = fold_backed(Pane::Backlog, DERIVED);

    assert_eq!(
        state.fold().map(wire::FoldName::as_str),
        Some("board::backlog")
    );
    assert_eq!(state.generation(), Some(17));
    assert_eq!(state.cursor(), Some(seq(48200)));
    assert!(state.answer().is_derived());
}

#[test]
fn a_derived_answer_keeps_its_cursor_distinct_from_its_horizon() {
    let state = fold_backed(Pane::Backlog, DERIVED);

    // How far the generation applied and how far the inputs are complete are
    // separate claims. A pane that showed one in place of the other would let
    // a stale generation look current.
    assert_eq!(state.cursor(), Some(seq(48200)));
    assert_eq!(state.horizon(), seq(48213));
    assert_ne!(state.cursor(), Some(state.horizon()));
}

#[test]
fn a_head_direct_answer_renders_no_fold_generation_or_cursor() {
    let state = head_direct(Pane::Backlog, COMPLETE);

    assert_eq!(state.fold(), None);
    assert_eq!(state.generation(), None);
    assert_eq!(state.cursor(), None);
    assert!(!state.answer().is_derived());
}

#[test]
fn a_head_direct_answer_still_renders_the_rows_and_horizon_it_was_given() {
    let state = head_direct(Pane::Journal, COMPLETE);

    // Missing provenance is not missing content. The head-direct arm withholds
    // exactly the three fields it was never given and nothing else.
    assert_eq!(state.rows().len(), 2);
    assert_eq!(state.horizon(), seq(48213));
}

#[test]
fn the_same_rows_read_two_ways_differ_only_in_their_provenance() {
    let direct = head_direct(Pane::Backlog, COMPLETE);
    let backed = fold_backed(Pane::Backlog, DERIVED);

    assert_eq!(direct.rows(), backed.rows());
    assert_eq!(direct.overflow(), backed.overflow());
    assert_eq!(direct.horizon(), backed.horizon());

    assert_eq!(direct.fold(), None);
    assert!(backed.fold().is_some());
}

#[test]
fn a_pane_reports_the_axis_its_registry_row_names() {
    let state = head_direct(Pane::Journal, COMPLETE);

    assert_eq!(state.pane(), Pane::Journal);
    assert_eq!(state.axis(), Axis::Chronology);
    assert_eq!(state.axis(), state.pane().axis());
}
