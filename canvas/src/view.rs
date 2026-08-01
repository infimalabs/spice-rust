//! The four view axes and the only two admitted provenance frames.

use std::error::Error;
use std::fmt;

/// Complete derived-view vocabulary.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Axis {
    Magnitude,
    Flow,
    Topology,
    Chronology,
}

impl Axis {
    /// Every axis in presentation order.
    pub const ALL: &[Self] = &[
        Self::Magnitude,
        Self::Flow,
        Self::Topology,
        Self::Chronology,
    ];

    /// Route spelling for this axis.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Magnitude => "magnitude",
            Self::Flow => "flow",
            Self::Topology => "topology",
            Self::Chronology => "chronology",
        }
    }

    /// Fold that owns this derived answer.
    #[must_use]
    pub const fn fold(self) -> fold::FoldId {
        match self {
            Self::Magnitude => fold::FoldId::CanvasMagnitude,
            Self::Flow => fold::FoldId::CanvasFlow,
            Self::Topology => fold::FoldId::CanvasTopology,
            Self::Chronology => fold::FoldId::CanvasChronology,
        }
    }
}

/// Pack already-ranked rows without hiding overflow.
#[must_use]
pub fn pack_ranked<T>(
    mut rows: Vec<T>,
    limit: usize,
    horizon: wire::Seq,
    position: impl Fn(&T) -> wire::Seq,
) -> wire::Packed<T> {
    let overflow = (rows.len() > limit).then(|| wire::Overflow {
        omitted: u64::try_from(rows.len() - limit).expect("a vector length fits in u64"),
        continuation: position(&rows[limit]),
    });
    rows.truncate(limit);
    wire::Packed {
        rows,
        overflow,
        horizon,
    }
}

/// Head-direct answer. Its shape has no fold provenance field.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeadFrame<T>(wire::Packed<T>);

impl<T> HeadFrame<T> {
    #[must_use]
    pub const fn new(packed: wire::Packed<T>) -> Self {
        Self(packed)
    }

    #[must_use]
    pub const fn packed(&self) -> &wire::Packed<T> {
        &self.0
    }

    #[must_use]
    pub fn into_packed(self) -> wire::Packed<T> {
        self.0
    }
}

/// Fold-backed answer. Construction requires a published materialization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FoldFrame<T>(wire::Derived<T>);

impl<T> FoldFrame<T> {
    #[must_use]
    pub const fn derived(&self) -> &wire::Derived<T> {
        &self.0
    }

    #[must_use]
    pub fn into_derived(self) -> wire::Derived<T> {
        self.0
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_materialization(
        axis: Axis,
        materialization: &fold::Materialization,
        packed: wire::Packed<T>,
    ) -> Result<Self, FrameError> {
        if materialization.fold != axis.fold() {
            return Err(FrameError::FoldMismatch {
                axis,
                found: materialization.fold,
            });
        }
        let cursor = materialization
            .cursor
            .position()
            .ok_or(FrameError::EmptyCursor { axis })?;
        let horizon = materialization
            .horizon
            .position()
            .ok_or(FrameError::EmptyHorizon { axis })?;
        if packed.horizon != horizon {
            return Err(FrameError::HorizonMismatch {
                axis,
                materialized: horizon,
                packed: packed.horizon,
            });
        }
        let fold = materialization
            .fold
            .as_str()
            .parse()
            .map_err(|_| FrameError::InvalidFoldName { axis })?;
        Ok(Self(wire::Derived {
            fold,
            r#gen: materialization.generation,
            cursor,
            packed,
        }))
    }
}

/// Fold provenance and packed boundaries disagreed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrameError {
    FoldMismatch {
        axis: Axis,
        found: fold::FoldId,
    },
    EmptyCursor {
        axis: Axis,
    },
    EmptyHorizon {
        axis: Axis,
    },
    HorizonMismatch {
        axis: Axis,
        materialized: wire::Seq,
        packed: wire::Seq,
    },
    InvalidFoldName {
        axis: Axis,
    },
}

impl fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FoldMismatch { axis, found } => {
                write!(
                    formatter,
                    "{} view requires {}; found {found}",
                    axis.as_str(),
                    axis.fold()
                )
            }
            Self::EmptyCursor { axis } => write!(formatter, "{} fold has no cursor", axis.as_str()),
            Self::EmptyHorizon { axis } => {
                write!(formatter, "{} fold has no horizon", axis.as_str())
            }
            Self::HorizonMismatch {
                axis,
                materialized,
                packed,
            } => write!(
                formatter,
                "{} packed horizon {packed} disagrees with materialization {materialized}",
                axis.as_str()
            ),
            Self::InvalidFoldName { axis } => {
                write!(
                    formatter,
                    "{} fold has no admitted wire name",
                    axis.as_str()
                )
            }
        }
    }
}

impl Error for FrameError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq(value: u64) -> wire::Seq {
        wire::Seq::new(value).expect("non-zero fixture position")
    }

    #[test]
    fn every_axis_names_one_canvas_fold() {
        assert_eq!(Axis::ALL.len(), 4);
        assert_eq!(Axis::Magnitude.fold(), fold::FoldId::CanvasMagnitude);
        assert_eq!(Axis::Flow.fold(), fold::FoldId::CanvasFlow);
        assert_eq!(Axis::Topology.fold(), fold::FoldId::CanvasTopology);
        assert_eq!(Axis::Chronology.fold(), fold::FoldId::CanvasChronology);
    }

    #[test]
    fn ranked_packing_names_omission_continuation_and_horizon() {
        let packed = pack_ranked(vec![seq(1), seq(2), seq(3)], 2, seq(9), |row| *row);

        assert_eq!(packed.rows, vec![seq(1), seq(2)]);
        assert_eq!(
            packed.overflow,
            Some(wire::Overflow {
                omitted: 1,
                continuation: seq(3),
            })
        );
        assert_eq!(packed.horizon, seq(9));
    }

    #[test]
    fn head_direct_frames_remain_bare_packed_values() {
        let packed = wire::Packed {
            rows: vec!["head"],
            overflow: None,
            horizon: seq(4),
        };
        let frame = HeadFrame::new(packed.clone());

        assert_eq!(frame.into_packed(), packed);
    }
}
