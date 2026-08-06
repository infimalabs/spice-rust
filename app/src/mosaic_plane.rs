//! The production bridge from one live lane feed to the pure Mosaic engine.
//!
//! Browser rendering owns measurement and markup. This module owns the one
//! boundary that must not disappear during view rewrites: every lane submits
//! its measured candidates to [`crate::mosaic::pack`] and receives a plane
//! whose extent comes from the same engine result.

use crate::mosaic::{self, Candidate, Card, TRACK_COUNT};

/// One independently packed lane plane.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LanePlane {
    /// Cards placed in stream creation order.
    pub cards: Vec<Card>,
    /// Highest occupied lattice row.
    pub max_row: i32,
    /// Lowest occupied lattice base.
    pub min_base: i32,
}

impl LanePlane {
    /// Number of rows the CSS grid must expose, including a negative ripple.
    #[must_use]
    pub fn rows(&self) -> i32 {
        (self.max_row - self.min_base).max(1)
    }
}

/// Pack one lane without carrying a skyline in from any sibling lane.
#[must_use]
pub fn arrange(candidate_sets: &[Vec<Candidate>]) -> LanePlane {
    let cards = mosaic::pack(candidate_sets, TRACK_COUNT);
    let (max_row, min_base) = mosaic::extent(&cards);
    LanePlane {
        cards,
        max_row,
        min_base,
    }
}
