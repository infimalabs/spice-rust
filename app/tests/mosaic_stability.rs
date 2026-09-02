use std::collections::{BTreeMap, BTreeSet};

use app::mosaic::Candidate;
use app::mosaic_plane::{MeasuredCard, PersistentLanePlane, Transition};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PlacementTuple {
    track: usize,
    base_row: i32,
    span: usize,
    extent: i32,
}

type PlacementSnapshot = BTreeMap<&'static str, PlacementTuple>;

#[derive(Clone, Copy)]
struct ExpectedMovement {
    key: &'static str,
    placement: PlacementTuple,
}

struct MovementDeclaration {
    targets: &'static [ExpectedMovement],
    cause: &'static str,
}

fn measured(key: &'static str, span: usize, extent: i32) -> MeasuredCard<&'static str> {
    MeasuredCard {
        key,
        candidates: vec![Candidate { rows: extent, span }],
        latched: None,
        content_revision: 0,
        boundary: false,
    }
}

fn boundary(key: &'static str) -> MeasuredCard<&'static str> {
    MeasuredCard {
        boundary: true,
        ..measured(key, 12, 1)
    }
}

fn placements(
    plane: &PersistentLanePlane<&'static str>,
    selected: &[&'static str],
) -> PlacementSnapshot {
    let selected = selected.iter().copied().collect::<BTreeSet<_>>();
    let snapshot = plane
        .cards()
        .iter()
        .filter(|entry| selected.is_empty() || selected.contains(entry.key))
        .map(|entry| {
            (
                entry.key,
                PlacementTuple {
                    track: entry.card.track,
                    base_row: entry.card.base,
                    span: entry.card.span,
                    extent: entry.card.rows,
                },
            )
        })
        .collect::<PlacementSnapshot>();
    assert!(
        !snapshot.is_empty(),
        "a canonical placement snapshot cannot be empty"
    );
    if !selected.is_empty() {
        assert_eq!(
            snapshot.keys().copied().collect::<BTreeSet<_>>(),
            selected,
            "a canonical placement snapshot contains every selected identity",
        );
    }
    snapshot
}

fn assert_stable(
    before: &PlacementSnapshot,
    after: &PlacementSnapshot,
    added: &[&'static str],
    label: &str,
) {
    let additions = added.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(
        additions.len(),
        added.len(),
        "{label} declares each addition once"
    );
    for (key, prior) in before {
        let Some(current) = after.get(key) else {
            panic!("{label}: card {key:?} disappeared; before={prior:?}");
        };
        assert_eq!(
            current, prior,
            "{label}: card {key:?} moved unexpectedly; before={prior:?} after={current:?}",
        );
    }
    for (key, current) in after {
        assert!(
            before.contains_key(key) || additions.contains(key),
            "{label}: undeclared card {key:?} appeared; after={current:?}",
        );
    }
    for key in additions {
        assert!(
            !before.contains_key(key),
            "{label}: addition {key:?} existed before"
        );
        assert!(
            after.contains_key(key),
            "{label}: addition {key:?} did not appear"
        );
    }
}

fn assert_declared_movement(
    before: &PlacementSnapshot,
    after: &PlacementSnapshot,
    declaration: &MovementDeclaration,
) {
    assert!(
        !declaration.cause.is_empty(),
        "movement declares its governing cause"
    );
    let targets = declaration
        .targets
        .iter()
        .map(|target| (target.key, target.placement))
        .collect::<BTreeMap<_, _>>();
    assert!(!targets.is_empty(), "movement declares at least one target");
    assert_eq!(
        targets.len(),
        declaration.targets.len(),
        "movement declares each target once"
    );
    assert_eq!(
        before.keys().collect::<Vec<_>>(),
        after.keys().collect::<Vec<_>>()
    );
    for (key, prior) in before {
        let current = &after[key];
        if let Some(expected) = targets.get(key) {
            assert_ne!(
                expected, prior,
                "target {key:?} declares no movement for {}; tuple={prior:?}",
                declaration.cause,
            );
            assert_eq!(
                current, expected,
                "target {key:?} moved to an unexpected placement for {}; expected={expected:?} actual={current:?}",
                declaration.cause,
            );
        } else {
            assert_eq!(
                current, prior,
                "card {key:?} moved unexpectedly for {}; before={prior:?} after={current:?}",
                declaration.cause,
            );
        }
    }
    assert!(targets.keys().all(|key| before.contains_key(key)));
}

#[test]
fn noops_content_only_updates_and_sibling_activity_preserve_committed_tuples() {
    let initial = [
        measured("a", 12, 2),
        measured("b", 12, 2),
        measured("c", 12, 2),
        measured("d", 12, 2),
    ];
    let plane = PersistentLanePlane::new(900, &initial);
    let baseline = placements(&plane, &[]);

    let noop = plane.reconcile(900, &initial);
    assert_stable(&baseline, &placements(&noop, &[]), &[], "a no-op render");

    let revised = noop.reconcile(
        900,
        &[
            initial[0].clone(),
            initial[1].clone(),
            initial[2].clone(),
            MeasuredCard {
                content_revision: 1,
                ..initial[3].clone()
            },
        ],
    );
    assert_eq!(revised.transition(), Transition::ContentResize);
    assert_stable(
        &baseline,
        &placements(&revised, &[]),
        &[],
        "a content-only revision with unchanged extent",
    );

    let sibling = measured("sibling-agent-message", 6, 3);
    let appended = revised.reconcile(
        900,
        &[
            initial[0].clone(),
            initial[1].clone(),
            initial[2].clone(),
            MeasuredCard {
                content_revision: 1,
                ..initial[3].clone()
            },
            sibling,
        ],
    );
    assert_stable(
        &placements(&revised, &[]),
        &placements(&appended, &[]),
        &["sibling-agent-message"],
        "activity from a sibling agent",
    );
    assert_stable(
        &placements(&appended, &[]),
        &placements(
            &appended.reconcile(
                900,
                &[
                    initial[0].clone(),
                    initial[1].clone(),
                    initial[2].clone(),
                    MeasuredCard {
                        content_revision: 1,
                        ..initial[3].clone()
                    },
                    measured("sibling-agent-message", 6, 3),
                ],
            ),
            &[],
        ),
        &[],
        "an unchanged replay",
    );
}

#[test]
fn pagination_outside_an_epoch_preserves_the_selected_committed_tuples() {
    let initial = [
        measured("old", 12, 2),
        boundary("compact-a"),
        measured("prior-epoch", 12, 4),
        boundary("compact-b"),
        measured("recent", 6, 3),
    ];
    let plane = PersistentLanePlane::new(900, &initial);
    let outside = ["old", "compact-a", "compact-b", "recent"];
    let before = placements(&plane, &outside);
    let replaced = plane.reconcile(
        900,
        &[
            initial[0].clone(),
            initial[1].clone(),
            measured("epoch-left", 6, 2),
            measured("epoch-right", 6, 2),
            initial[3].clone(),
            initial[4].clone(),
        ],
    );
    assert_eq!(replaced.transition(), Transition::EpochReplay);
    assert_stable(
        &before,
        &placements(&replaced, &outside),
        &[],
        "an epoch replay outside its affected interval",
    );

    let first_page = replaced.reconcile(
        900,
        &[
            measured("older", 12, 2),
            initial[0].clone(),
            initial[1].clone(),
            measured("epoch-left", 6, 2),
            measured("epoch-right", 6, 2),
            initial[3].clone(),
            initial[4].clone(),
        ],
    );
    let committed = placements(&first_page, &[]);
    let second_page = first_page.reconcile(
        900,
        &[
            measured("oldest", 3, 1),
            measured("older", 12, 2),
            initial[0].clone(),
            initial[1].clone(),
            measured("epoch-left", 6, 2),
            measured("epoch-right", 6, 2),
            initial[3].clone(),
            initial[4].clone(),
        ],
    );
    assert_stable(
        &committed,
        &placements(&second_page, &[]),
        &["oldest"],
        "pagination before the affected epoch",
    );
}

#[test]
fn required_growth_declares_every_moving_card_and_preserves_the_rest() {
    let initial = [
        measured("a", 12, 2),
        measured("b", 12, 2),
        measured("c", 12, 2),
        measured("d", 12, 2),
    ];
    let plane = PersistentLanePlane::new(900, &initial);
    let before = placements(&plane, &[]);
    let movement = MovementDeclaration {
        targets: &[
            ExpectedMovement {
                key: "a",
                placement: PlacementTuple {
                    track: 0,
                    base_row: -2,
                    span: 12,
                    extent: 2,
                },
            },
            ExpectedMovement {
                key: "b",
                placement: PlacementTuple {
                    track: 0,
                    base_row: 0,
                    span: 12,
                    extent: 4,
                },
            },
        ],
        cause: "frozen card b grows downward through overlapping card a",
    };
    let grown = plane.reconcile(
        900,
        &[
            initial[0].clone(),
            measured("b", 12, 4),
            initial[2].clone(),
            initial[3].clone(),
        ],
    );
    let after = placements(&grown, &[]);
    assert_declared_movement(&before, &after, &movement);

    let mut arbitrary = after;
    arbitrary.insert(
        "a",
        PlacementTuple {
            track: 99,
            base_row: -99,
            span: 12,
            extent: 999,
        },
    );
    assert!(
        std::panic::catch_unwind(|| assert_declared_movement(&before, &arbitrary, &movement))
            .is_err(),
        "an arbitrary but different target tuple must not satisfy the movement declaration",
    );
}
