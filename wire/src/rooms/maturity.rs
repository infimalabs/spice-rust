//! Bounded advisory maturity for one Design.
//!
//! The accumulator consumes admitted organic notices and typed contributions in
//! journal order. It retains exact detail for a fixed recent statement window,
//! folds older heads into one aggregate, and caps every retained lane set. The
//! resulting evidence never changes task, claim, group, decision, or execution
//! authority.

use serde::{Deserialize, Serialize};

use super::{DesignNoticeProvenance, DesignNoticed, RoomContributed, RoomContributionKind};
use crate::{CanonicalUuid, Seq};

/// Statement heads retained individually in one maturity projection.
pub const DESIGN_MATURITY_HEAD_LIMIT: usize = 4;
/// Lane identities retained in each bounded maturity set.
pub const DESIGN_MATURITY_LANE_LIMIT: usize = 64;

/// Counts of each typed contribution kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignContributionCounts {
    /// Reasons to take the direction.
    pub reason_for: u64,
    /// Reasons against the direction.
    pub reason_against: u64,
    /// Observations bearing on the problem.
    pub evidence: u64,
    /// Open questions, counted but never required for maturity.
    pub question: u64,
}

impl DesignContributionCounts {
    /// Total typed contributions.
    #[must_use]
    pub const fn total(self) -> u64 {
        self.reason_for + self.reason_against + self.evidence + self.question
    }

    fn observe(&mut self, kind: RoomContributionKind) {
        let count = match kind {
            RoomContributionKind::ReasonFor => &mut self.reason_for,
            RoomContributionKind::ReasonAgainst => &mut self.reason_against,
            RoomContributionKind::Evidence => &mut self.evidence,
            RoomContributionKind::Question => &mut self.question,
        };
        *count = count.saturating_add(1);
    }
}

/// One recent statement head's invitation and contribution evidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignMaturityHead {
    /// Exact Design statement head shared by these records.
    pub statement: Seq,
    /// Organic notice records at this head, including repeat lanes.
    pub organic_notices: u64,
    /// Distinct invited lanes retained in sorted order.
    pub invited_lanes: Vec<CanonicalUuid>,
    /// More distinct invited lanes existed than the retained bound.
    pub invited_lanes_truncated: bool,
    /// Contribution records at this head, including repeat lanes.
    pub contributions: u64,
    /// Distinct contributing lanes retained in sorted order.
    pub contributing_lanes: Vec<CanonicalUuid>,
    /// More distinct contributing lanes existed than the retained bound.
    pub contributing_lanes_truncated: bool,
}

impl DesignMaturityHead {
    fn new(statement: Seq) -> Self {
        Self {
            statement,
            organic_notices: 0,
            invited_lanes: Vec::new(),
            invited_lanes_truncated: false,
            contributions: 0,
            contributing_lanes: Vec::new(),
            contributing_lanes_truncated: false,
        }
    }

    /// Number of retained lanes both invited and contributing at this head.
    #[must_use]
    pub fn invited_contributors(&self) -> usize {
        intersection_count(&self.invited_lanes, &self.contributing_lanes)
    }

    /// Whether the displayed intersection may omit lanes beyond a bound.
    #[must_use]
    pub const fn yield_truncated(&self) -> bool {
        self.invited_lanes_truncated || self.contributing_lanes_truncated
    }
}

/// Evidence folded from statement heads older than the exact recent window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignMaturityOlder {
    /// Distinct statement heads folded here.
    pub statement_heads: u64,
    /// Organic notice records across those heads.
    pub organic_notices: u64,
    /// Sum of each head's distinct invited lanes.
    pub invited_lane_heads: u64,
    /// Contribution records across those heads.
    pub contributions: u64,
    /// Sum of each head's distinct contributing lanes.
    pub contributing_lane_heads: u64,
    /// Sum of each head's invited/contributing lane intersection.
    pub invited_contributor_lane_heads: u64,
    /// At least one folded head exceeded a retained lane bound.
    pub truncated: bool,
}

/// Historical contributions written before statement attribution existed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnrecordedMaturityHead {
    /// Contributions with no recorded statement head.
    pub contributions: u64,
    /// Distinct lanes retained in sorted order.
    pub contributing_lanes: Vec<CanonicalUuid>,
    /// More distinct lanes existed than the retained bound.
    pub contributing_lanes_truncated: bool,
}

/// One Design's bounded, advisory maturity evidence.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignMaturity {
    /// Counts across every contribution, including historical unattributed rows.
    pub contributions: DesignContributionCounts,
    /// Distinct contributing lanes retained in sorted order.
    pub contributing_lanes: Vec<CanonicalUuid>,
    /// More distinct contributing lanes existed than the retained bound.
    pub contributing_lanes_truncated: bool,
    /// Organic notice records across all statement heads.
    pub organic_notices: u64,
    /// Distinct organically invited lanes retained in sorted order.
    pub organic_lanes: Vec<CanonicalUuid>,
    /// More distinct organically invited lanes existed than the retained bound.
    pub organic_lanes_truncated: bool,
    /// Exact recent statement heads, oldest first.
    pub recent_heads: Vec<DesignMaturityHead>,
    /// Fixed-size aggregate of older statement heads.
    pub older_heads: DesignMaturityOlder,
    /// Explicit bucket for historical contributions with no recorded head.
    pub unrecorded_head: UnrecordedMaturityHead,
}

/// One unmet substantive maturity dimension.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DesignMaturityRequirement {
    /// Fewer than three distinct lanes contributed.
    ThreeContributingLanes,
    /// No reason-for contribution exists.
    ReasonFor,
    /// No reason-against contribution exists.
    ReasonAgainst,
    /// No evidence contribution exists.
    Evidence,
}

impl DesignMaturityRequirement {
    /// Operator-facing words for this unmet dimension.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::ThreeContributingLanes => "three distinct contributing lanes",
            Self::ReasonFor => "a reason for",
            Self::ReasonAgainst => "a reason against",
            Self::Evidence => "evidence",
        }
    }
}

/// Advisory state derived only from substantive contribution breadth and kinds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum DesignMaturityState {
    /// No contribution exists.
    Fresh,
    /// Some contribution exists but one or more substantive dimensions remain.
    Steeping {
        /// Every dimension still required for `Steeped`.
        unmet: Vec<DesignMaturityRequirement>,
    },
    /// Three lanes plus reason-for, reason-against, and evidence are present.
    Steeped,
}

impl DesignMaturity {
    /// Fold one admitted notice. Seeded notices are deliberately excluded: the
    /// readout describes organic reach only.
    pub fn observe_notice(&mut self, notice: &DesignNoticed) {
        if notice.provenance != DesignNoticeProvenance::Organic {
            return;
        }
        self.organic_notices = self.organic_notices.saturating_add(1);
        insert_bounded(
            &mut self.organic_lanes,
            &mut self.organic_lanes_truncated,
            notice.lane,
        );
        self.observe_head(notice.statement, |head| {
            head.organic_notices = head.organic_notices.saturating_add(1);
            insert_bounded(
                &mut head.invited_lanes,
                &mut head.invited_lanes_truncated,
                notice.lane,
            );
        });
    }

    /// Fold one admitted contribution.
    pub fn observe_contribution(&mut self, contribution: &RoomContributed) {
        self.contributions.observe(contribution.contribution);
        insert_bounded(
            &mut self.contributing_lanes,
            &mut self.contributing_lanes_truncated,
            contribution.lane,
        );
        let Some(statement) = contribution.statement else {
            self.unrecorded_head.contributions =
                self.unrecorded_head.contributions.saturating_add(1);
            insert_bounded(
                &mut self.unrecorded_head.contributing_lanes,
                &mut self.unrecorded_head.contributing_lanes_truncated,
                contribution.lane,
            );
            return;
        };
        self.observe_head(statement, |head| {
            head.contributions = head.contributions.saturating_add(1);
            insert_bounded(
                &mut head.contributing_lanes,
                &mut head.contributing_lanes_truncated,
                contribution.lane,
            );
        });
    }

    /// Current advisory maturity state. Invitation reach and questions never
    /// satisfy a dimension.
    #[must_use]
    pub fn state(&self) -> DesignMaturityState {
        if self.contributions.total() == 0 {
            return DesignMaturityState::Fresh;
        }
        let mut unmet = Vec::with_capacity(4);
        if self.contributing_lanes.len() < 3 {
            unmet.push(DesignMaturityRequirement::ThreeContributingLanes);
        }
        if self.contributions.reason_for == 0 {
            unmet.push(DesignMaturityRequirement::ReasonFor);
        }
        if self.contributions.reason_against == 0 {
            unmet.push(DesignMaturityRequirement::ReasonAgainst);
        }
        if self.contributions.evidence == 0 {
            unmet.push(DesignMaturityRequirement::Evidence);
        }
        if unmet.is_empty() {
            DesignMaturityState::Steeped
        } else {
            DesignMaturityState::Steeping { unmet }
        }
    }

    fn observe_head(&mut self, statement: Seq, observe: impl FnOnce(&mut DesignMaturityHead)) {
        let index = match self
            .recent_heads
            .binary_search_by_key(&statement, |head| head.statement)
        {
            Ok(index) => index,
            Err(index) => {
                self.recent_heads
                    .insert(index, DesignMaturityHead::new(statement));
                index
            }
        };
        observe(&mut self.recent_heads[index]);
        if self.recent_heads.len() > DESIGN_MATURITY_HEAD_LIMIT {
            let older = self.recent_heads.remove(0);
            self.fold_older(older);
        }
    }

    fn fold_older(&mut self, head: DesignMaturityHead) {
        let older = &mut self.older_heads;
        older.statement_heads = older.statement_heads.saturating_add(1);
        older.organic_notices = older.organic_notices.saturating_add(head.organic_notices);
        older.invited_lane_heads = older
            .invited_lane_heads
            .saturating_add(head.invited_lanes.len() as u64);
        older.contributions = older.contributions.saturating_add(head.contributions);
        older.contributing_lane_heads = older
            .contributing_lane_heads
            .saturating_add(head.contributing_lanes.len() as u64);
        older.invited_contributor_lane_heads = older
            .invited_contributor_lane_heads
            .saturating_add(head.invited_contributors() as u64);
        older.truncated |= head.yield_truncated();
    }
}

fn insert_bounded(lanes: &mut Vec<CanonicalUuid>, truncated: &mut bool, lane: CanonicalUuid) {
    match lanes.binary_search(&lane) {
        Ok(_) => {}
        Err(index) if lanes.len() < DESIGN_MATURITY_LANE_LIMIT => lanes.insert(index, lane),
        Err(_) => *truncated = true,
    }
}

fn intersection_count(left: &[CanonicalUuid], right: &[CanonicalUuid]) -> usize {
    let (mut left_index, mut right_index, mut matched) = (0, 0, 0);
    while left_index < left.len() && right_index < right.len() {
        match left[left_index].cmp(&right[right_index]) {
            std::cmp::Ordering::Less => left_index += 1,
            std::cmp::Ordering::Greater => right_index += 1,
            std::cmp::Ordering::Equal => {
                matched += 1;
                left_index += 1;
                right_index += 1;
            }
        }
    }
    matched
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rooms::RoomAuthorityStamp;
    use crate::{SequenceCode, TaskStamp};

    fn lane(last: u8) -> CanonicalUuid {
        format!("00000000-0000-4000-8000-{last:012}")
            .parse()
            .expect("fixture lane")
    }

    fn task(micros: i64) -> TaskStamp {
        TaskStamp::for_micros(micros).expect("fixture task")
    }

    fn notice(statement: u64, lane: u8, provenance: DesignNoticeProvenance) -> DesignNoticed {
        DesignNoticed {
            design: task(1),
            statement: Seq::new(statement).expect("statement"),
            provenance,
            lane: self::lane(lane),
            occupant: self::lane(lane.saturating_add(100)),
            task: task(2),
            claim: SequenceCode::new(statement.saturating_add(100)),
            terms: matches!(provenance, DesignNoticeProvenance::Organic)
                .then(|| {
                    vec![
                        "design".to_owned(),
                        "maturity".to_owned(),
                        "measure".to_owned(),
                    ]
                })
                .unwrap_or_default(),
        }
    }

    fn contribution(
        statement: Option<u64>,
        lane: u8,
        kind: RoomContributionKind,
    ) -> RoomContributed {
        RoomContributed {
            room: task(1),
            publication: RoomAuthorityStamp::for_micros(
                1_000 + i64::from(lane) + i64::try_from(statement.unwrap_or(0)).unwrap_or(0),
            )
            .expect("publication"),
            lane: self::lane(lane),
            contribution: kind,
            statement: statement.and_then(Seq::new),
            text: "Substantive contribution.".to_owned(),
            sources: Vec::new(),
        }
    }

    #[test]
    fn substantive_breadth_alone_crosses_steeped() {
        let mut maturity = DesignMaturity::default();
        for lane in 1..=12 {
            maturity.observe_notice(&notice(10, lane, DesignNoticeProvenance::Organic));
        }
        maturity.observe_notice(&notice(10, 20, DesignNoticeProvenance::Seeded));
        assert_eq!(maturity.state(), DesignMaturityState::Fresh);
        assert_eq!(maturity.organic_notices, 12);
        assert_eq!(maturity.organic_lanes.len(), 12);

        maturity.observe_contribution(&contribution(Some(10), 1, RoomContributionKind::Question));
        maturity.observe_contribution(&contribution(Some(10), 1, RoomContributionKind::Question));
        assert_eq!(
            maturity.state(),
            DesignMaturityState::Steeping {
                unmet: vec![
                    DesignMaturityRequirement::ThreeContributingLanes,
                    DesignMaturityRequirement::ReasonFor,
                    DesignMaturityRequirement::ReasonAgainst,
                    DesignMaturityRequirement::Evidence,
                ],
            }
        );
        maturity.observe_contribution(&contribution(Some(10), 1, RoomContributionKind::ReasonFor));
        maturity.observe_contribution(&contribution(
            Some(10),
            2,
            RoomContributionKind::ReasonAgainst,
        ));
        maturity.observe_contribution(&contribution(Some(10), 3, RoomContributionKind::Evidence));
        assert_eq!(maturity.state(), DesignMaturityState::Steeped);
        assert_eq!(maturity.contributing_lanes, vec![lane(1), lane(2), lane(3)]);
    }

    #[test]
    fn head_yield_history_and_memory_are_bounded() {
        let mut maturity = DesignMaturity::default();
        for statement in 1..=6 {
            maturity.observe_notice(&notice(statement, 1, DesignNoticeProvenance::Organic));
            maturity.observe_notice(&notice(statement, 2, DesignNoticeProvenance::Organic));
            maturity.observe_contribution(&contribution(
                Some(statement),
                2,
                RoomContributionKind::Evidence,
            ));
        }
        maturity.observe_contribution(&contribution(None, 9, RoomContributionKind::ReasonAgainst));

        assert_eq!(maturity.recent_heads.len(), DESIGN_MATURITY_HEAD_LIMIT);
        assert_eq!(
            maturity
                .recent_heads
                .iter()
                .map(|head| head.statement.get())
                .collect::<Vec<_>>(),
            vec![3, 4, 5, 6]
        );
        assert!(
            maturity
                .recent_heads
                .iter()
                .all(|head| head.invited_contributors() == 1)
        );
        assert_eq!(maturity.older_heads.statement_heads, 2);
        assert_eq!(maturity.older_heads.invited_contributor_lane_heads, 2);
        assert_eq!(maturity.unrecorded_head.contributions, 1);
        assert_eq!(maturity.unrecorded_head.contributing_lanes, vec![lane(9)]);
        for lane in 10..=100 {
            maturity.observe_notice(&notice(6, lane, DesignNoticeProvenance::Organic));
            maturity.observe_contribution(&contribution(
                Some(6),
                lane,
                RoomContributionKind::Question,
            ));
        }
        let current = maturity
            .recent_heads
            .last()
            .expect("current statement head");
        assert_eq!(current.invited_lanes.len(), DESIGN_MATURITY_LANE_LIMIT);
        assert_eq!(current.contributing_lanes.len(), DESIGN_MATURITY_LANE_LIMIT);
        assert!(current.invited_lanes_truncated);
        assert!(current.contributing_lanes_truncated);
        assert_eq!(
            maturity.contributing_lanes.len(),
            DESIGN_MATURITY_LANE_LIMIT
        );
        assert!(maturity.contributing_lanes_truncated);
    }

    #[test]
    fn historical_contribution_omission_decodes_explicitly() {
        let current = contribution(Some(10), 1, RoomContributionKind::Evidence);
        let mut value = serde_json::to_value(current).expect("encodes");
        value.as_object_mut().expect("object").remove("statement");
        let historical: RoomContributed = serde_json::from_value(value).expect("decodes");
        assert_eq!(historical.statement, None);

        let mut maturity = DesignMaturity::default();
        maturity.observe_contribution(&historical);
        assert_eq!(maturity.unrecorded_head.contributions, 1);
        assert!(maturity.recent_heads.is_empty());
    }
}
