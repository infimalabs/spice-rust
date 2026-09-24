//! That the axis and Canvas destination registries are complete and stable.
//!
//! A registry cannot audit itself: a walk over `ALL` never visits a variant
//! omitted from `ALL`. So each vocabulary is also gathered here by exhaustive
//! descent, a successor chain whose `match` names every variant and fails to
//! compile while one is missing, and the two are compared at runtime.

use std::collections::BTreeSet;

use app::{Axis, CanvasDestination};

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

/// Every global Canvas destination, independent of its convenience registry.
fn descend_canvas_destinations() -> Vec<CanvasDestination> {
    std::iter::successors(
        Some(CanvasDestination::Info),
        |destination| match destination {
            CanvasDestination::Info => Some(CanvasDestination::Work),
            CanvasDestination::Work => Some(CanvasDestination::Chat),
            CanvasDestination::Chat => None,
        },
    )
    .collect()
}

#[test]
fn the_axis_registry_lists_every_axis_that_exists() {
    assert_eq!(descend_axes(), Axis::ALL.to_vec());
}

#[test]
fn every_global_destination_has_one_stable_route_value() {
    let destinations = descend_canvas_destinations();
    assert_eq!(destinations, CanvasDestination::ALL);

    let mut route_values = BTreeSet::new();
    for destination in destinations {
        let value = destination.route_value();
        assert!(
            route_values.insert(value),
            "{value:?} names two destinations"
        );
        assert_eq!(
            CanvasDestination::from_route_value(Some(value)),
            destination
        );
    }
}

#[test]
fn missing_or_invalid_routes_retain_the_legacy_chat_default() {
    assert_eq!(
        CanvasDestination::from_route_value(None),
        CanvasDestination::Chat
    );
    assert_eq!(
        CanvasDestination::from_route_value(Some("unknown")),
        CanvasDestination::Chat
    );
}

#[test]
fn global_surface_state_round_trips_through_every_destination() {
    for destination in descend_canvas_destinations() {
        let axis = destination.axis().unwrap_or(Axis::Topology);
        assert_eq!(
            CanvasDestination::from_surface(axis, destination == CanvasDestination::Chat),
            destination
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
