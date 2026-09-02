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
        (index < self.count).then(|| PipPosition {
            column: self.columns - index / ROWS,
            row: self.rows() - index % ROWS,
        })
    }

    pub(crate) fn columns_from_left(self) -> impl Iterator<Item = PipColumn> {
        (0..self.columns).rev().map(move |column| {
            let lower = column * ROWS;
            PipColumn {
                upper: (lower + 1 < self.count).then_some(lower + 1),
                lower,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn fills_each_column_bottom_to_top_and_columns_right_to_left() {
        let grid = PipGrid::new(8);

        assert_eq!(grid.rows(), 2);
        assert_eq!(grid.columns, 4);
        assert_eq!(
            (0..8)
                .map(|index| grid.position(index).unwrap())
                .collect::<Vec<_>>(),
            vec![
                PipPosition { column: 4, row: 2 },
                PipPosition { column: 4, row: 1 },
                PipPosition { column: 3, row: 2 },
                PipPosition { column: 3, row: 1 },
                PipPosition { column: 2, row: 2 },
                PipPosition { column: 2, row: 1 },
                PipPosition { column: 1, row: 2 },
                PipPosition { column: 1, row: 1 },
            ],
        );
        assert_eq!(
            grid.columns_from_left().collect::<Vec<_>>(),
            vec![
                PipColumn {
                    upper: Some(7),
                    lower: 6,
                },
                PipColumn {
                    upper: Some(5),
                    lower: 4,
                },
                PipColumn {
                    upper: Some(3),
                    lower: 2,
                },
                PipColumn {
                    upper: Some(1),
                    lower: 0,
                },
            ],
        );
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
                for column in position.column + 1..=grid.columns {
                    for row in 1..=grid.rows() {
                        assert!(
                            positions.contains(&PipPosition { column, row }),
                            "count {count} filled column {} before rightward ({column}, {row})",
                            position.column,
                        );
                    }
                }
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
            Some(PipPosition { column: 1, row: 2 })
        );
        assert_eq!(grid.position(10_001), None);
        assert_eq!(grid.columns_from_left().count(), 5_001);
    }
}
