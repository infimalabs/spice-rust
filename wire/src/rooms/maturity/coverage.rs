//! Current-statement question coverage, folded from the same item series that
//! maturity weighs but judged apart from it.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use super::{DesignItemState, DesignItems};
use crate::rooms::RoomQuestion;

/// One current statement's question coverage, in statement order. A question
/// is covered once an item-specific nonzero charge weighed it; coverage proves
/// neither concurrence nor the quality of the research behind the charge.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignCoverage {
    /// Questions some item-specific nonzero charge weighed.
    pub covered: Vec<RoomQuestion>,
    /// Questions no item-specific nonzero charge has weighed.
    pub missing: Vec<RoomQuestion>,
}

impl DesignCoverage {
    /// Whether every current question is covered; a statement naming none
    /// is.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.missing.is_empty()
    }

    /// Current-question research instruction, separate from pass readiness.
    #[must_use]
    pub fn research_guidance(&self, questions: &[String], design: &crate::TaskHandle) -> String {
        if self.is_full() {
            return "Design research: all current questions are covered; verify the findings before proposing. Coverage is not concurrence or proof of research quality.".to_owned();
        }
        let mut guidance =
            String::from("Design research: investigate remaining questions before proposing: ");
        for (offset, question) in self.missing.iter().enumerate() {
            if offset != 0 {
                guidance.push_str("; ");
            }
            write!(
                guidance,
                "{question}: {}",
                question
                    .item(questions)
                    .expect("coverage belongs to the current statement")
            )
            .expect("writing to a String succeeds");
        }
        write!(
            guidance,
            ". Record source-backed findings with `spice room contribute {design} --question N \
             --charge=C --cite SOURCE` (nonzero charge, either sign; coverage is not concurrence \
             or proof of research quality)."
        )
        .expect("writing to a String succeeds");
        guidance
    }
}

impl DesignItems {
    /// Which current items of `acceptance` an item-specific nonzero charge
    /// has weighed, in statement order. A covered item may still be contested
    /// or unopposed: coverage is not concurrence.
    #[must_use]
    pub fn coverage(&self, acceptance: &[String]) -> DesignCoverage {
        let mut coverage = DesignCoverage::default();
        for (offset, state) in self.states(acceptance).into_iter().enumerate() {
            let Some(question) = u16::try_from(offset + 1).ok().and_then(RoomQuestion::new) else {
                continue;
            };
            if state == DesignItemState::Quiet {
                coverage.missing.push(question);
            } else {
                coverage.covered.push(question);
            }
        }
        coverage
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{ITEMS, acceptance, charge, contribution};
    use super::*;
    use crate::rooms::{RoomContributed, RoomContributionKind, RoomQuestionItem};

    fn item_state(items: &DesignItems, item: &str) -> DesignItemState {
        items.states(&[item.to_owned()])[0]
    }

    #[test]
    fn coverage_counts_each_current_question_some_nonzero_charge_weighed() {
        let question = |position| RoomQuestion::new(position).expect("one-based question");
        let mut items = DesignItems::default();
        assert!(
            items.coverage(&[]).is_full(),
            "a statement naming no question is covered"
        );
        let current = acceptance(&ITEMS);
        assert_eq!(
            items.coverage(&current),
            DesignCoverage {
                covered: Vec::new(),
                missing: vec![question(1), question(2)],
            }
        );

        // An item-specific ask and a zero charge enter no series.
        items.observe(&RoomContributed {
            item: Some(RoomQuestionItem::of(ITEMS[0])),
            ..contribution(Some(10), 1, RoomContributionKind::Question)
        });
        items.observe(&charge(1, ITEMS[0], 0));
        assert_eq!(items.coverage(&current).missing, [question(1), question(2)]);

        // A charge of either sign covers; a contested item stays covered.
        items.observe(&charge(1, ITEMS[1], -1));
        assert_eq!(items.coverage(&current).missing, [question(1)]);
        items.observe(&charge(1, ITEMS[0], 1));
        items.observe(&charge(2, ITEMS[0], -1));
        assert_eq!(item_state(&items, ITEMS[0]), DesignItemState::Contested);
        assert_eq!(
            items.coverage(&current),
            DesignCoverage {
                covered: vec![question(1), question(2)],
                missing: Vec::new(),
            }
        );

        // Coverage follows the current statement's items and positions.
        let edited = acceptance(&["Who reviews it?", ITEMS[1], ITEMS[0]]);
        assert_eq!(
            items.coverage(&edited),
            DesignCoverage {
                covered: vec![question(2), question(3)],
                missing: vec![question(1)],
            }
        );
    }
}
