//! That every pane answers on exactly one of the two axes.
//!
//! The registry cannot audit itself. A test that walked `Pane::ALL` to check
//! that every pane has an axis would pass forever, because a pane omitted from
//! `ALL` is a pane the walk never visits. So the panes are gathered here by
//! exhaustive descent instead: a successor chain whose `match` names every
//! variant, which the compiler refuses to accept while a variant is missing.
//!
//! That gives two independent oracles pointed at each other. The chain fails to
//! compile when a variant is added without a place in it, and comparing the
//! chain against `ALL` fails at runtime when a variant reaches the chain but
//! not the registry. Neither half is sufficient alone, which is the whole
//! reason both are here.

use std::collections::{BTreeMap, BTreeSet};

use app::{Axis, Pane};

/// Every axis, gathered by exhaustive descent rather than by reading `ALL`.
///
/// The `match` is exhaustive over [`Axis`], so a third axis is a compile error
/// here before it is a test failure anywhere.
fn descend_axes() -> Vec<Axis> {
    std::iter::successors(Some(Axis::Flow), |axis| match axis {
        Axis::Flow => Some(Axis::Topology),
        Axis::Topology => None,
    })
    .collect()
}

/// Every pane, gathered by exhaustive descent rather than by reading `ALL`.
///
/// Adding a variant to [`Pane`] without adding it here is `E0004`, so this
/// chain is complete by construction rather than by review.
fn descend_panes() -> Vec<Pane> {
    std::iter::successors(Some(Pane::Tasks), |pane| match pane {
        Pane::Tasks => Some(Pane::Claims),
        Pane::Claims => Some(Pane::Deferrals),
        Pane::Deferrals => Some(Pane::Reviews),
        Pane::Reviews => Some(Pane::Lanes),
        Pane::Lanes => Some(Pane::Worktrees),
        Pane::Worktrees => Some(Pane::Lineage),
        Pane::Lineage => Some(Pane::Occupancy),
        Pane::Occupancy => Some(Pane::Residents),
        Pane::Residents => None,
    })
    .collect()
}

#[test]
fn the_axis_registry_lists_every_axis_that_exists() {
    assert_eq!(descend_axes(), Axis::ALL.to_vec());
}

#[test]
fn the_pane_registry_lists_every_pane_that_exists() {
    assert_eq!(descend_panes(), Pane::ALL.to_vec());
}

#[test]
fn every_pane_maps_to_one_of_the_two_axes() {
    let axes: BTreeSet<Axis> = descend_axes().into_iter().collect();

    for pane in descend_panes() {
        assert!(
            axes.contains(&pane.axis()),
            "{} answers on an axis outside the vocabulary",
            pane.as_str()
        );
    }
}

#[test]
fn every_axis_carries_at_least_one_pane() {
    let mut byaxis: BTreeMap<Axis, Vec<Pane>> = BTreeMap::new();
    for pane in descend_panes() {
        byaxis.entry(pane.axis()).or_default().push(pane);
    }

    for axis in descend_axes() {
        assert!(
            byaxis.get(&axis).is_some_and(|panes| !panes.is_empty()),
            "the {} axis carries no pane",
            axis.as_str()
        );
    }
}

#[test]
fn each_axis_lists_exactly_the_panes_that_name_it() {
    // `Axis::panes` reads the registry; the descent does not. Comparing them
    // per axis is what keeps the convenient direction honest, because a pane
    // dropped from `ALL` would vanish from `panes` while still naming its axis.
    for axis in descend_axes() {
        let listed = axis.panes().collect::<Vec<_>>();
        let naming = descend_panes()
            .into_iter()
            .filter(|pane| pane.axis() == axis)
            .collect::<Vec<_>>();

        assert_eq!(
            listed,
            naming,
            "the {} axis lists the wrong panes",
            axis.as_str()
        );
        assert!(
            !listed.is_empty(),
            "the {} axis lists nothing",
            axis.as_str()
        );
    }
}

#[test]
fn walking_the_axes_in_order_visits_the_whole_registry_once() {
    // Concatenating the axes' pane lists reproduces the registry exactly, which
    // says two things at once: no pane is reachable through two axes, and the
    // registry is written grouped by axis rather than merely claiming to be.
    let gathered = descend_axes()
        .into_iter()
        .flat_map(Axis::panes)
        .collect::<Vec<_>>();

    assert_eq!(gathered, descend_panes());
}

#[test]
fn claims_divide_across_the_remaining_axes() {
    assert_eq!(Pane::Claims.axis(), Axis::Flow);
    assert_eq!(Pane::Occupancy.axis(), Axis::Topology);
}

#[test]
fn every_pane_spells_itself_once() {
    let mut seen = BTreeSet::new();
    for pane in descend_panes() {
        let spelling = pane.as_str();
        assert!(!spelling.is_empty(), "{pane:?} renders as the empty string");
        assert!(
            spelling
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'-'),
            "{spelling:?} is not lower-case kebab"
        );
        assert!(seen.insert(spelling), "{spelling:?} names two panes");
    }
}

#[test]
fn every_axis_spells_itself_once_and_states_its_own_question() {
    let mut spellings = BTreeSet::new();
    let mut questions = BTreeSet::new();

    for axis in descend_axes() {
        assert!(
            axis.as_str().bytes().all(|byte| byte.is_ascii_lowercase()),
            "{:?} is not a lower-case single word",
            axis.as_str()
        );
        assert!(
            spellings.insert(axis.as_str()),
            "{:?} names two axes",
            axis.as_str()
        );
        assert!(
            questions.insert(axis.question()),
            "{:?} asks a question another axis already asks",
            axis.as_str()
        );
    }
}
