use app::mosaic::{Candidate, Lifecycle};
use app::mosaic_plane::{MeasuredCard, PersistentLanePlane, Transition, arrange};

fn candidates(span: usize, rows: i32) -> Vec<Candidate> {
    vec![Candidate { span, rows }]
}

/// This is the production dependency witness, not another engine-unit test.
/// `mosaic_plane::arrange` is the same function each browser lane component
/// calls. Removing its last call into `app/src/mosaic.rs` either fails to build
/// or makes these engine-decided placements and lifecycle states go red.
#[test]
fn production_lane_plane_is_decided_by_the_mosaic_engine() {
    let plane = arrange(&[
        candidates(3, 2),
        candidates(3, 4),
        candidates(6, 3),
        candidates(12, 2),
    ]);

    let placements = plane
        .cards
        .iter()
        .map(|card| (card.track, card.span, card.base, card.rows, card.lifecycle))
        .collect::<Vec<_>>();
    assert_eq!(
        placements,
        vec![
            (0, 3, 0, 2, Lifecycle::Liquid),
            (3, 3, 0, 4, Lifecycle::Liquid),
            (6, 6, 0, 3, Lifecycle::Liquid),
            (0, 12, 4, 2, Lifecycle::Liquid),
        ]
    );
    assert_eq!((plane.max_row, plane.min_base, plane.rows()), (6, 0, 6));
}

#[test]
fn sibling_lane_planes_never_share_a_row_floor() {
    let busy = arrange(&[candidates(12, 5), candidates(12, 4), candidates(12, 3)]);
    let quiet = arrange(&[candidates(12, 2)]);

    assert_eq!(busy.max_row, 12);
    assert_eq!(quiet.cards[0].base, 0);
    assert_eq!(quiet.max_row, 2);
}

fn measured(key: &'static str, candidates: Vec<Candidate>) -> MeasuredCard<&'static str> {
    MeasuredCard {
        key,
        candidates,
        latched: None,
        content_revision: 0,
        boundary: false,
    }
}

fn boundary(key: &'static str, rows: i32) -> MeasuredCard<&'static str> {
    MeasuredCard {
        boundary: true,
        ..measured(key, candidates(12, rows))
    }
}

#[test]
fn persistent_noop_and_append_never_repack_committed_cards() {
    let initial = [
        measured("a", candidates(3, 3)),
        measured("b", candidates(3, 4)),
    ];
    let plane = PersistentLanePlane::new(900, &initial);
    let committed = plane.cards().to_vec();

    let noop = plane.reconcile(900, &initial);
    assert_eq!(noop.cards(), committed);
    assert_eq!(noop.commits(), plane.commits());
    assert_eq!(noop.transition(), plane.transition());

    let appended = plane.reconcile(
        900,
        &[
            initial[0].clone(),
            initial[1].clone(),
            measured("c", candidates(12, 5)),
        ],
    );
    assert_eq!(&appended.cards()[..2], committed);
    assert_eq!(appended.cards()[2].key, "c");
    assert_eq!(appended.transition(), Transition::Append);
}

#[test]
fn retained_order_changes_replay_while_tail_appends_preserve_geometry() {
    let initial = [
        measured("a", candidates(12, 2)),
        measured("b", candidates(12, 2)),
        measured("c", candidates(12, 2)),
    ];
    let plane = PersistentLanePlane::new(900, &initial);
    let reordered_frame = plane.reconcile(
        900,
        &[initial[2].clone(), initial[0].clone(), initial[1].clone()],
    );
    assert_eq!(
        reordered_frame
            .cards()
            .iter()
            .map(|entry| entry.key)
            .collect::<Vec<_>>(),
        vec!["c", "a", "b"],
        "a retained-order change adopts the complete chronological snapshot",
    );
    assert_eq!(reordered_frame.transition(), Transition::FullReplay);

    let committed_geometry = plane
        .cards()
        .iter()
        .map(|entry| {
            (
                entry.card.track,
                entry.card.span,
                entry.card.base,
                entry.card.rows,
            )
        })
        .collect::<Vec<_>>();
    let appended = plane.reconcile(
        900,
        &[
            initial[0].clone(),
            initial[1].clone(),
            initial[2].clone(),
            measured("d", candidates(12, 2)),
        ],
    );
    assert_eq!(
        appended.cards()[..initial.len()]
            .iter()
            .map(|entry| {
                (
                    entry.card.track,
                    entry.card.span,
                    entry.card.base,
                    entry.card.rows,
                )
            })
            .collect::<Vec<_>>(),
        committed_geometry,
        "a later lane update cannot move committed geometry",
    );
    assert!(
        appended.cards()[..initial.len()]
            .iter()
            .zip(plane.cards())
            .all(|(next, prior)| {
                prior.card.lifecycle != Lifecycle::Frozen
                    || next.card.lifecycle == Lifecycle::Frozen
            }),
        "a frozen card never returns to liquid",
    );
    assert!(
        appended.cards()[..initial.len()]
            .iter()
            .zip(plane.cards())
            .any(|(next, prior)| {
                prior.card.lifecycle == Lifecycle::Liquid
                    && next.card.lifecycle == Lifecycle::Frozen
            }),
        "the append deterministically advances one buried liquid card",
    );
}

#[test]
fn a_key_discovered_between_committed_cards_replays_true_chronology() {
    let initial = [
        measured("message-a", candidates(6, 3)),
        measured("message-c", candidates(6, 4)),
    ];
    let plane = PersistentLanePlane::new(900, &initial);
    let complete = [
        initial[0].clone(),
        measured("message-b", candidates(12, 2)),
        initial[1].clone(),
    ];
    let inserted = plane.reconcile(900, &complete);
    let one_shot = PersistentLanePlane::new(900, &complete);

    assert_eq!(
        inserted
            .cards()
            .iter()
            .map(|entry| entry.key)
            .collect::<Vec<_>>(),
        vec!["message-a", "message-b", "message-c"],
        "the plane renders the complete chronological snapshot"
    );
    assert_eq!(inserted.cards(), one_shot.cards());
    assert_eq!(inserted.transition(), Transition::FullReplay);
}

#[test]
fn staged_edge_batches_converge_to_one_shot_before_a_middle_replay() {
    let b = measured("b", candidates(6, 3));
    let d = measured("d", candidates(6, 4));
    let initial = PersistentLanePlane::new(900, &[b.clone(), d.clone()]);
    let committed = initial.cards().to_vec();
    let edge_snapshot = [
        measured("a", candidates(12, 2)),
        b.clone(),
        d.clone(),
        measured("e", candidates(3, 3)),
    ];
    let edged = initial.reconcile(900, &edge_snapshot);
    assert_eq!(
        edged.cards()[1..3]
            .iter()
            .map(|entry| {
                (
                    entry.card.track,
                    entry.card.span,
                    entry.card.base,
                    entry.card.rows,
                )
            })
            .collect::<Vec<_>>(),
        committed
            .iter()
            .map(|entry| {
                (
                    entry.card.track,
                    entry.card.span,
                    entry.card.base,
                    entry.card.rows,
                )
            })
            .collect::<Vec<_>>(),
        "edge additions retain committed placement geometry",
    );
    assert_eq!(edged.transition(), Transition::Append);

    let complete = [
        edge_snapshot[0].clone(),
        b,
        measured("c", candidates(3, 5)),
        d,
        edge_snapshot[3].clone(),
    ];
    let settled = edged.reconcile(900, &complete);
    let one_shot = PersistentLanePlane::new(900, &complete);
    assert_eq!(settled.cards(), one_shot.cards());
    assert_eq!(settled.transition(), Transition::FullReplay);
    assert_eq!(settled.reconcile(900, &complete), settled);
}

#[test]
fn interleaved_hydration_replays_deterministically_across_a_compaction_epoch() {
    let old = measured("old", candidates(12, 2));
    let compact = boundary("compact", 1);
    let recent = measured("recent", candidates(6, 3));
    let initial = PersistentLanePlane::new(900, &[old.clone(), compact.clone(), recent.clone()]);
    let complete = [
        old,
        MeasuredCard {
            content_revision: 1,
            ..compact
        },
        measured("middle-a", candidates(3, 4)),
        measured("middle-b", candidates(3, 2)),
        recent,
    ];

    let staged = initial.reconcile(900, &complete);
    let one_shot = PersistentLanePlane::new(900, &complete);

    assert_eq!(staged.transition(), Transition::EpochReplay);
    assert_eq!(&staged.cards()[..1], &initial.cards()[..1]);
    assert_eq!(staged.cards()[1].card, initial.cards()[1].card);
    assert_eq!(staged.cards()[1].content_revision, 1);
    assert_eq!(
        staged
            .cards()
            .iter()
            .map(|entry| {
                (
                    entry.key,
                    entry.card.track,
                    entry.card.span,
                    entry.card.base,
                    entry.card.rows,
                )
            })
            .collect::<Vec<_>>(),
        one_shot
            .cards()
            .iter()
            .map(|entry| {
                (
                    entry.key,
                    entry.card.track,
                    entry.card.span,
                    entry.card.base,
                    entry.card.rows,
                )
            })
            .collect::<Vec<_>>(),
        "epoch replay reaches the same deterministic positions and extents as chronological insertion",
    );
    let divider = &staged.cards()[1].card;
    assert!(
        staged.cards()[2..]
            .iter()
            .all(|entry| entry.card.base >= divider.base + divider.rows),
        "the replayed middle batch stays inside the post-compaction epoch",
    );
    assert_eq!(staged.reconcile(900, &complete), staged);
}

#[test]
fn an_overfull_stable_epoch_falls_back_to_complete_replay() {
    let head = measured("head", candidates(12, 2));
    let compact = boundary("compact", 1);
    let recent = measured("recent", candidates(6, 3));
    let initial = PersistentLanePlane::new(900, &[head.clone(), compact.clone(), recent.clone()]);
    let older = [
        "older-a", "older-b", "older-c", "older-d", "older-e", "older-f", "older-g", "older-h",
    ]
    .into_iter()
    .map(|key| measured(key, candidates(6, 2)))
    .collect::<Vec<_>>();
    let expanded_snapshot = older
        .iter()
        .cloned()
        .chain([head.clone(), compact.clone(), recent.clone()])
        .collect::<Vec<_>>();
    let expanded = initial.reconcile(900, &expanded_snapshot);
    assert_eq!(expanded.transition(), Transition::Backfill);

    let mut complete = expanded_snapshot;
    complete.insert(older.len(), measured("interleaved", candidates(12, 4)));
    let replayed = expanded.reconcile(900, &complete);
    let one_shot = PersistentLanePlane::new(900, &complete);

    assert_eq!(replayed.transition(), Transition::FullReplay);
    assert_eq!(replayed.cards(), one_shot.cards());
    assert_eq!(
        (replayed.max_row(), replayed.rows()),
        (one_shot.max_row(), one_shot.rows())
    );
}

#[test]
fn replacing_one_epoch_preserves_every_outside_position_and_extent() {
    let old = measured("old", candidates(12, 2));
    let start = boundary("compact-a", 1);
    let prior_epoch = measured("prior-epoch", candidates(12, 4));
    let end = boundary("compact-b", 1);
    let recent = measured("recent", candidates(6, 3));
    let initial_snapshot = [
        old.clone(),
        start.clone(),
        prior_epoch,
        end.clone(),
        recent.clone(),
    ];
    let initial = PersistentLanePlane::new(900, &initial_snapshot);
    let before_start = initial.cards()[..=1].to_vec();
    let after_end = initial.cards()[3..].to_vec();
    let complete = [
        old,
        start,
        measured("unavailable-terminal", candidates(12, 1)),
        measured("epoch-left", candidates(6, 2)),
        measured("epoch-right", candidates(6, 2)),
        end,
        recent,
    ];

    let replayed = initial.reconcile(900, &complete);

    assert_eq!(replayed.transition(), Transition::EpochReplay);
    assert_eq!(&replayed.cards()[..=1], before_start);
    assert_eq!(&replayed.cards()[5..], after_end);
    assert!(!replayed.cards()[2].boundary);
    assert_eq!(
        (replayed.max_row(), replayed.rows()),
        (initial.max_row(), initial.rows()),
        "an epoch replacement cannot change the outside plane extent",
    );
    let lower = replayed.cards()[1].card.creation_index;
    let upper = replayed.cards()[5].card.creation_index;
    assert!(
        replayed.cards()[2..5].iter().all(|entry| {
            entry.card.creation_index > lower && entry.card.creation_index < upper
        }),
        "replacement cards retain creation order between their stable dividers",
    );
    assert_eq!(
        replayed.cards()[2..5]
            .iter()
            .map(|entry| (
                entry.key,
                entry.card.track,
                entry.card.base,
                entry.card.rows
            ))
            .collect::<Vec<_>>(),
        vec![
            ("unavailable-terminal", 0, 3, 1),
            ("epoch-left", 0, 4, 2),
            ("epoch-right", 6, 4, 2),
        ],
        "the replacement starts from the older divider skyline in creation order",
    );
    assert_eq!(initial.reconcile(900, &complete), replayed);
    assert_eq!(replayed.reconcile(900, &complete), replayed);
}

#[test]
fn content_resize_latches_horizontal_placement_and_uses_fresh_rows() {
    let plane = PersistentLanePlane::new(
        900,
        &[
            measured("a", candidates(3, 3)),
            measured("b", candidates(3, 3)),
        ],
    );
    let before = plane.cards()[0].card.clone();
    let resized = plane.reconcile(
        900,
        &[
            MeasuredCard {
                key: "a",
                candidates: vec![
                    Candidate { span: 3, rows: 7 },
                    Candidate { span: 6, rows: 2 },
                ],
                latched: None,
                content_revision: 0,
                boundary: false,
            },
            measured("b", candidates(3, 3)),
        ],
    );
    let after = &resized.cards()[0].card;
    assert_eq!((after.track, after.span), (before.track, before.span));
    assert_eq!(after.rows, 7);
    assert_eq!(resized.transition(), Transition::ContentResize);
}

#[test]
fn a_latched_span_measurement_is_part_of_content_identity() {
    let candidates = vec![
        Candidate { span: 3, rows: 3 },
        Candidate { span: 6, rows: 3 },
    ];
    let plane = PersistentLanePlane::new(900, &[measured("a", candidates.clone())]);
    assert_eq!(plane.cards()[0].card.span, 6);
    let resized = plane.reconcile(
        900,
        &[MeasuredCard {
            key: "a",
            candidates,
            latched: Some(Candidate { span: 6, rows: 5 }),
            content_revision: 0,
            boundary: false,
        }],
    );
    assert_eq!(resized.cards()[0].card.rows, 5);
    assert_eq!(resized.transition(), Transition::ContentResize);
}

#[test]
fn a_content_revision_commits_even_when_the_lattice_rows_are_unchanged() {
    let initial = measured("a", candidates(3, 3));
    let plane = PersistentLanePlane::new(900, std::slice::from_ref(&initial));
    let resolved = plane.reconcile(
        900,
        &[MeasuredCard {
            content_revision: 1,
            ..initial
        }],
    );
    assert_eq!(resolved.cards()[0].card, plane.cards()[0].card);
    assert_eq!(resolved.commits(), plane.commits() + 1);
    assert_eq!(resolved.transition(), Transition::ContentResize);
}

#[test]
fn compaction_boundary_preserves_the_upward_append_coordinate() {
    let initial = [
        measured("a", candidates(12, 2)),
        measured("b", candidates(12, 2)),
        measured("c", candidates(12, 2)),
        measured("d", candidates(12, 2)),
    ];
    let plane = PersistentLanePlane::new(900, &initial);
    assert_eq!(plane.cards()[0].card.lifecycle, Lifecycle::Frozen);
    let frozen = plane.cards()[..2].to_vec();
    let committed_geometry = plane
        .cards()
        .iter()
        .map(|entry| {
            (
                entry.card.track,
                entry.card.span,
                entry.card.base,
                entry.card.rows,
            )
        })
        .collect::<Vec<_>>();

    let with_boundary = plane.reconcile(
        900,
        &[
            initial[0].clone(),
            initial[1].clone(),
            initial[2].clone(),
            initial[3].clone(),
            boundary("compact", 1),
        ],
    );
    assert_eq!(&with_boundary.cards()[..frozen.len()], frozen);
    assert_eq!(
        with_boundary.cards()[..initial.len()]
            .iter()
            .map(|entry| {
                (
                    entry.card.track,
                    entry.card.span,
                    entry.card.base,
                    entry.card.rows,
                )
            })
            .collect::<Vec<_>>(),
        committed_geometry
    );
    let divider = &with_boundary.cards()[4].card;
    assert!(with_boundary.cards()[4].boundary);
    assert_eq!((divider.track, divider.span), (0, 12));
    assert_eq!(divider.base, 8);
    assert_eq!(with_boundary.max_row(), divider.base + divider.rows);

    let after = with_boundary.reconcile(
        900,
        &[
            initial[0].clone(),
            initial[1].clone(),
            initial[2].clone(),
            initial[3].clone(),
            boundary("compact", 1),
            measured("after", candidates(3, 3)),
        ],
    );
    assert_eq!(&after.cards()[..frozen.len()], frozen);
    assert_eq!(&after.cards()[4].card, divider);
    assert!(after.cards()[5].card.base >= divider.base + divider.rows);
    assert_eq!(
        after.max_row(),
        after.cards()[5].card.base + after.cards()[5].card.rows,
        "the plane frontier advances with the appended card",
    );
    assert_eq!(after.transition(), Transition::Append);

    let replayed = after.reconcile(
        720,
        &[
            initial[0].clone(),
            initial[1].clone(),
            initial[2].clone(),
            initial[3].clone(),
            boundary("compact", 1),
            measured("after", candidates(3, 3)),
        ],
    );
    assert_eq!(replayed.transition(), Transition::FullReplay);
    assert!(
        replayed.cards()[5].card.base
            >= replayed.cards()[4].card.base + replayed.cards()[4].card.rows,
        "a full replay keeps post-compaction cards on the same upward coordinate",
    );
}

#[test]
fn geometry_replay_preserves_retained_creation_indexes_after_staged_history() {
    let head = measured("head", candidates(12, 2));
    let compact = boundary("compact", 1);
    let recent = measured("recent", candidates(6, 3));
    let initial = PersistentLanePlane::new(900, &[head.clone(), compact.clone(), recent.clone()]);
    let staged_snapshot = [
        measured("older", candidates(12, 2)),
        head,
        compact,
        recent,
        measured("live", candidates(3, 2)),
    ];
    let staged = initial.reconcile(900, &staged_snapshot);
    let creation_indexes = staged
        .cards()
        .iter()
        .map(|entry| (entry.key, entry.card.creation_index))
        .collect::<Vec<_>>();
    assert_ne!(
        creation_indexes[0].1,
        1usize << (usize::BITS - 1),
        "staged history receives an index below the original chronology",
    );

    let resized = staged.reconcile(720, &staged_snapshot);

    assert_eq!(resized.transition(), Transition::FullReplay);
    assert_eq!(resized.geometry(), 720);
    assert_eq!(
        resized
            .cards()
            .iter()
            .map(|entry| (entry.key, entry.card.creation_index))
            .collect::<Vec<_>>(),
        creation_indexes,
        "geometry replay preserves every retained semantic creation index",
    );
}

#[test]
fn content_resize_preserves_a_boundary_below_a_later_frozen_subset() {
    let initial = [
        measured("a", candidates(12, 2)),
        measured("b", candidates(12, 2)),
        measured("c", candidates(12, 2)),
        measured("d", candidates(12, 2)),
    ];
    let epoch = [
        initial[0].clone(),
        initial[1].clone(),
        initial[2].clone(),
        initial[3].clone(),
        boundary("compact", 1),
        measured("post-a", candidates(6, 1)),
        measured("post-b", candidates(6, 100)),
        measured("post-c", candidates(6, 1)),
        measured("post-d", candidates(6, 1)),
    ];
    let plane = PersistentLanePlane::new(900, &initial).reconcile(900, &epoch);
    assert!(
        plane.cards()[..=4]
            .iter()
            .all(|entry| entry.card.lifecycle == Lifecycle::Frozen),
        "the boundary latches its completed chronological epoch"
    );
    assert_eq!(plane.cards()[5].card.lifecycle, Lifecycle::Frozen);
    let boundary_geometry = plane.cards()[4].card.clone();

    let resized = plane.reconcile(
        900,
        &[
            epoch[0].clone(),
            epoch[1].clone(),
            epoch[2].clone(),
            epoch[3].clone(),
            epoch[4].clone(),
            epoch[5].clone(),
            epoch[6].clone(),
            epoch[7].clone(),
            measured("post-d", candidates(6, 2)),
        ],
    );

    assert_eq!(resized.cards()[4].card, boundary_geometry);
    assert!(
        resized.cards()[5].card.base >= resized.cards()[4].card.base + resized.cards()[4].card.rows
    );
    assert_eq!(resized.transition(), Transition::ContentResize);
}

#[test]
fn frozen_content_resolution_keeps_shrink_reservation_and_ripples_growth() {
    let initial = [
        measured("a", candidates(12, 2)),
        measured("b", candidates(12, 2)),
        measured("c", candidates(12, 2)),
        measured("d", candidates(12, 2)),
    ];
    let plane = PersistentLanePlane::new(900, &initial);
    assert_eq!(plane.cards()[1].card.lifecycle, Lifecycle::Frozen);

    let shrunk = plane.reconcile(
        900,
        &[
            initial[0].clone(),
            measured("b", candidates(12, 1)),
            initial[2].clone(),
            initial[3].clone(),
        ],
    );
    assert_eq!(shrunk.cards()[1].card.rows, 2);
    assert_eq!(shrunk.cards()[0].card.base, plane.cards()[0].card.base);

    let grown = plane.reconcile(
        900,
        &[
            initial[0].clone(),
            measured("b", candidates(12, 4)),
            initial[2].clone(),
            initial[3].clone(),
        ],
    );
    assert_eq!(grown.cards()[1].card.rows, 4);
    assert!(grown.cards()[0].card.base < plane.cards()[0].card.base);
    assert_eq!(grown.cards()[2].card.base, plane.cards()[2].card.base);
}

#[test]
fn prefix_backfill_lands_below_without_moving_committed_geometry() {
    let initial = [
        measured("b", candidates(6, 3)),
        measured("c", candidates(6, 4)),
    ];
    let plane = PersistentLanePlane::new(900, &initial);
    let committed = plane
        .cards()
        .iter()
        .map(|entry| {
            (
                entry.card.track,
                entry.card.span,
                entry.card.base,
                entry.card.rows,
            )
        })
        .collect::<Vec<_>>();
    let backfilled = plane.reconcile(
        900,
        &[
            measured("a", candidates(12, 2)),
            initial[0].clone(),
            initial[1].clone(),
        ],
    );
    let retained = backfilled.cards()[1..]
        .iter()
        .map(|entry| {
            (
                entry.card.track,
                entry.card.span,
                entry.card.base,
                entry.card.rows,
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(retained, committed);
    assert!(backfilled.cards()[0].card.base < 0);
    assert_eq!(backfilled.transition(), Transition::Backfill);

    let first_page = backfilled.cards().to_vec();
    let twice_backfilled = backfilled.reconcile(
        900,
        &[
            measured("z", candidates(3, 1)),
            measured("a", candidates(12, 2)),
            initial[0].clone(),
            initial[1].clone(),
        ],
    );
    assert_eq!(
        &twice_backfilled.cards()[1..],
        first_page,
        "later history pages preserve every previously committed creation label and placement",
    );
}
