//! That the axis and app destination registries are complete and stable.
//!
//! A registry cannot audit itself: a walk over `ALL` never visits a variant
//! omitted from `ALL`. So each vocabulary is also gathered here by exhaustive
//! descent, a successor chain whose `match` names every variant and fails to
//! compile while one is missing, and the two are compared at runtime.

use std::collections::BTreeSet;

use app::{AppDestination, Axis};

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

/// Every global app destination, independent of its convenience registry.
fn descend_app_destinations() -> Vec<AppDestination> {
    std::iter::successors(
        Some(AppDestination::Dash),
        |destination| match destination {
            AppDestination::Dash => Some(AppDestination::Work),
            AppDestination::Work => Some(AppDestination::Chat),
            AppDestination::Chat => None,
        },
    )
    .collect()
}

#[test]
fn the_axis_registry_lists_every_axis_that_exists() {
    assert_eq!(descend_axes(), Axis::ALL.to_vec());
}

#[test]
fn every_global_destination_has_one_exact_hash_route() {
    let routes = [
        (AppDestination::Dash, "/dash"),
        (AppDestination::Work, "/work"),
        (AppDestination::Chat, "/chat"),
    ];
    assert_eq!(descend_app_destinations(), AppDestination::ALL);

    let mut paths = BTreeSet::new();
    for (destination, path) in routes {
        assert!(paths.insert(path), "{path:?} names two destinations");
        assert_eq!(destination.path(), path);
        assert_eq!(AppDestination::from_path(path), Some(destination));
    }
}

#[test]
fn unrecognized_route_paths_are_refused() {
    for path in ["", "/", "/unknown", "/work/", "/dash/", "/work?scope=all"] {
        assert_eq!(
            AppDestination::from_path(path),
            None,
            "{path:?} unexpectedly named an app destination"
        );
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
