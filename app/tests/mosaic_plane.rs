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
fn incremental_frames_preserve_order_and_freeze_without_repacking() {
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
    assert_eq!(reordered_frame, plane, "source-vector churn is a no-op");
    assert_eq!(
        reordered_frame
            .cards()
            .iter()
            .map(|entry| entry.key)
            .collect::<Vec<_>>(),
        vec!["a", "b", "c"],
        "the committed creation order owns rendering order",
    );

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

/// The plane renders arrival order, not the order `activity_cards` chains.
///
/// `activity_cards` builds its vector as every agent message and then every
/// pending directive, so a message arriving while a directive is pending sits
/// *between* committed cards in that source order. The plane appends it to the
/// tail instead. That is the decided behavior, not a defect to refile: the
/// source order is only how unseen keys reach the plane, while
/// `render_activity_cards` renders by `cards()`, so what the operator sees is
/// arrival order and the DOM order still matches the creation order.
///
/// Honoring the source order instead would repack on the ordinary path — an
/// operator steers, the agent answers — which is what a persistent plane exists
/// to avoid, and there is no durable key to interleave the two kinds by: a
/// directive carries the commit instant of the fact that authored it, while a
/// message carries only a tape position.
///
/// The cost, accepted here rather than hidden: until the directive clears, the
/// later message renders below it, and the removal's full replay then moves it
/// back above.
#[test]
fn a_message_measured_between_committed_cards_takes_the_tail_by_decision() {
    let initial = [
        measured("message-a", candidates(6, 3)),
        measured("directive-x", candidates(6, 4)),
    ];
    let plane = PersistentLanePlane::new(900, &initial);
    let committed = plane.cards().to_vec();

    let inserted = plane.reconcile(
        900,
        &[
            initial[0].clone(),
            measured("message-b", candidates(12, 2)),
            initial[1].clone(),
        ],
    );

    assert_eq!(
        inserted
            .cards()
            .iter()
            .map(|entry| entry.key)
            .collect::<Vec<_>>(),
        vec!["message-a", "directive-x", "message-b"],
        "the later message takes the lattice tail, not the measured middle"
    );
    assert_eq!(&inserted.cards()[..2], committed);
    assert_eq!(inserted.transition(), Transition::Append);
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
}
