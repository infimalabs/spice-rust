//! Physical placement for the compact agent roster.

const ROWS: usize = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PipGrid {
    count: usize,
    columns: usize,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct PipPosition {
    pub(crate) column: usize,
    pub(crate) row: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PipColumn {
    pub(crate) upper: Option<usize>,
    pub(crate) lower: usize,
}

/// Two-row lane-header stack filled bottom-to-top, then right-to-left.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PipStack {
    count: usize,
    columns: usize,
}

impl PipGrid {
    pub(crate) fn new(count: usize) -> Self {
        Self {
            count,
            columns: count / ROWS + count % ROWS,
        }
    }

    pub(crate) fn rows(self) -> usize {
        self.count.min(ROWS)
    }

    pub(crate) fn position(self, index: usize) -> Option<PipPosition> {
        (index < self.count).then(|| {
            if index < self.columns {
                PipPosition {
                    column: self.columns - index,
                    row: self.rows(),
                }
            } else {
                let full_columns = self.count / ROWS;
                PipPosition {
                    column: full_columns - (index - self.columns),
                    row: 1,
                }
            }
        })
    }

    pub(crate) fn columns_from_left(self) -> impl Iterator<Item = PipColumn> {
        let full_columns = self.count / ROWS;
        (1..=self.columns).map(move |column| PipColumn {
            upper: (column <= full_columns).then_some(self.columns + full_columns - column),
            lower: self.columns - column,
        })
    }
}

impl PipStack {
    pub(crate) fn new(count: usize) -> Self {
        Self {
            count,
            columns: count / ROWS + count % ROWS,
        }
    }

    pub(crate) fn rows(self) -> usize {
        self.count.min(ROWS)
    }

    pub(crate) fn position(self, index: usize) -> Option<PipPosition> {
        (index < self.count).then(|| PipPosition {
            column: self.columns - index / ROWS,
            row: self.rows() - index % ROWS,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn fills_bottom_to_top_and_right_to_left_with_a_left_heavy_footprint() {
        let grid = PipGrid::new(8);

        assert_eq!(grid.rows(), 2);
        assert_eq!(grid.columns, 4);
        assert_eq!(
            (0..8)
                .map(|index| grid.position(index).unwrap())
                .collect::<Vec<_>>(),
            vec![
                PipPosition { column: 4, row: 2 },
                PipPosition { column: 3, row: 2 },
                PipPosition { column: 2, row: 2 },
                PipPosition { column: 1, row: 2 },
                PipPosition { column: 4, row: 1 },
                PipPosition { column: 3, row: 1 },
                PipPosition { column: 2, row: 1 },
                PipPosition { column: 1, row: 1 },
            ],
        );
        assert_eq!(
            grid.columns_from_left().collect::<Vec<_>>(),
            vec![
                PipColumn {
                    upper: Some(7),
                    lower: 3,
                },
                PipColumn {
                    upper: Some(6),
                    lower: 2,
                },
                PipColumn {
                    upper: Some(5),
                    lower: 1,
                },
                PipColumn {
                    upper: Some(4),
                    lower: 0,
                },
            ],
        );
    }

    #[test]
    fn three_pips_complete_the_left_column_and_support_the_lower_right() {
        let grid = PipGrid::new(3);

        assert_eq!(
            (0..3)
                .map(|index| grid.position(index).unwrap())
                .collect::<Vec<_>>(),
            vec![
                PipPosition { column: 2, row: 2 },
                PipPosition { column: 1, row: 2 },
                PipPosition { column: 1, row: 1 },
            ],
        );
        assert_eq!(
            grid.columns_from_left().collect::<Vec<_>>(),
            vec![
                PipColumn {
                    upper: Some(2),
                    lower: 1,
                },
                PipColumn {
                    upper: None,
                    lower: 0,
                },
            ],
        );
    }

    #[test]
    fn lane_stack_fills_each_inside_column_before_growing_left() {
        let stack = PipStack::new(5);

        assert_eq!(stack.rows(), 2);
        assert_eq!(stack.columns, 3);
        assert_eq!(
            (0..5)
                .map(|index| stack.position(index).unwrap())
                .collect::<Vec<_>>(),
            vec![
                PipPosition { column: 3, row: 2 },
                PipPosition { column: 3, row: 1 },
                PipPosition { column: 2, row: 2 },
                PipPosition { column: 2, row: 1 },
                PipPosition { column: 1, row: 2 },
            ],
        );
    }

    #[test]
    fn lane_stack_keeps_one_and_two_member_headers_in_one_column() {
        let single = PipStack::new(1);
        let pair = PipStack::new(2);

        assert_eq!(single.position(0), Some(PipPosition { column: 1, row: 1 }));
        assert_eq!(pair.position(0), Some(PipPosition { column: 1, row: 2 }));
        assert_eq!(pair.position(1), Some(PipPosition { column: 1, row: 1 }));
    }

    #[test]
    fn every_partial_grid_has_a_lower_pip_beneath_each_upper_pip() {
        for count in 0..=257 {
            let grid = PipGrid::new(count);
            let positions = (0..count)
                .map(|index| grid.position(index).unwrap())
                .collect::<BTreeSet<_>>();

            assert_eq!(positions.len(), count);
            for position in &positions {
                if grid.rows() == ROWS && position.row < grid.rows() {
                    assert!(
                        positions.contains(&PipPosition {
                            column: position.column,
                            row: grid.rows(),
                        }),
                        "count {count} left an upper pip unsupported in column {}",
                        position.column,
                    );
                }
            }
            for column in 2..=grid.columns {
                let left_count = positions
                    .iter()
                    .filter(|position| position.column == column - 1)
                    .count();
                let right_count = positions
                    .iter()
                    .filter(|position| position.column == column)
                    .count();
                assert!(
                    left_count >= right_count,
                    "count {count} put {right_count} pips in column {column} but only \
                     {left_count} in the column to its left",
                );
            }
        }
    }

    #[test]
    fn placement_has_no_team_size_limit() {
        let grid = PipGrid::new(10_001);

        assert_eq!(grid.rows(), 2);
        assert_eq!(grid.columns, 5_001);
        assert_eq!(
            grid.position(0),
            Some(PipPosition {
                column: 5_001,
                row: 2
            })
        );
        assert_eq!(
            grid.position(10_000),
            Some(PipPosition { column: 1, row: 1 })
        );
        assert_eq!(grid.position(10_001), None);
        assert_eq!(grid.columns_from_left().count(), 5_001);
    }
}
