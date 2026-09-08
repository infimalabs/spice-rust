//! Physical placement for the compact agent roster.

const ROWS: usize = 2;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct PipPosition {
    pub(crate) column: usize,
    pub(crate) row: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PipColumn {
    pub(crate) column: usize,
    pub(crate) upper: Option<usize>,
    pub(crate) lower: usize,
}

/// Two-row lane-header stack filled bottom-to-top, then right-to-left.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PipStack {
    count: usize,
    columns: usize,
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

    pub(crate) fn columns_from_left(self) -> impl Iterator<Item = PipColumn> {
        (1..=self.columns).map(move |column| {
            let lower = (self.columns - column) * ROWS;
            PipColumn {
                column,
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
        let stack = PipStack::new(8);

        assert_eq!(stack.rows(), 2);
        assert_eq!(stack.columns, 4);
        assert_eq!(
            (0..8)
                .map(|index| stack.position(index).unwrap())
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
            stack.columns_from_left().collect::<Vec<_>>(),
            vec![
                PipColumn {
                    column: 1,
                    upper: Some(7),
                    lower: 6,
                },
                PipColumn {
                    column: 2,
                    upper: Some(5),
                    lower: 4,
                },
                PipColumn {
                    column: 3,
                    upper: Some(3),
                    lower: 2,
                },
                PipColumn {
                    column: 4,
                    upper: Some(1),
                    lower: 0,
                },
            ],
        );
    }

    #[test]
    fn three_pips_leave_the_lower_left_floating_and_keep_the_first_at_lower_right() {
        let stack = PipStack::new(3);

        assert_eq!(
            (0..3)
                .map(|index| stack.position(index).unwrap())
                .collect::<Vec<_>>(),
            vec![
                PipPosition { column: 2, row: 2 },
                PipPosition { column: 2, row: 1 },
                PipPosition { column: 1, row: 2 },
            ],
        );
        assert_eq!(
            stack.columns_from_left().collect::<Vec<_>>(),
            vec![
                PipColumn {
                    column: 1,
                    upper: None,
                    lower: 2,
                },
                PipColumn {
                    column: 2,
                    upper: Some(1),
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
    fn every_partial_stack_has_unique_positions_and_exact_column_membership() {
        for count in 0..=257 {
            let stack = PipStack::new(count);
            let positions = (0..count)
                .map(|index| stack.position(index).unwrap())
                .collect::<BTreeSet<_>>();

            assert_eq!(positions.len(), count);
            assert_eq!(stack.columns_from_left().count(), stack.columns);
            for column in stack.columns_from_left() {
                assert_eq!(
                    stack.position(column.lower),
                    Some(PipPosition {
                        column: column.column,
                        row: stack.rows(),
                    }),
                );
                if let Some(upper) = column.upper {
                    assert_eq!(
                        stack.position(upper),
                        Some(PipPosition {
                            column: column.column,
                            row: 1,
                        }),
                    );
                }
            }
        }
    }

    #[test]
    fn placement_has_no_org_size_limit() {
        let stack = PipStack::new(10_001);

        assert_eq!(stack.rows(), 2);
        assert_eq!(stack.columns, 5_001);
        assert_eq!(
            stack.position(0),
            Some(PipPosition {
                column: 5_001,
                row: 2
            })
        );
        assert_eq!(
            stack.position(10_000),
            Some(PipPosition { column: 1, row: 2 })
        );
        assert_eq!(stack.position(10_001), None);
        assert_eq!(stack.columns_from_left().count(), 5_001);
    }
}
