//! Bounded advisory maturity for one Design.
//!
//! The accumulator consumes admitted Design requests and typed contributions
//! in journal order. It retains exact detail for a fixed recent statement window,
//! folds older heads into one aggregate, and caps every retained lane set. Each
//! charged question item keeps one fixed-size series keyed by its text
//! identity, so steeping is judged per item against the Design's current
//! statement. The resulting evidence never changes task, claim, group,
//! decision, or execution authority.

use serde::{Deserialize, Serialize};

use super::{
    DesignNoticed, RoomCharge, RoomContributed, RoomContributionKind, RoomLeft, RoomQuestion,
    RoomQuestionItem,
};
use crate::{CanonicalUuid, Seq};

/// Statement heads retained individually in one maturity projection.
pub const DESIGN_MATURITY_HEAD_LIMIT: usize = 4;
/// Lane identities retained in each bounded maturity set.
pub const DESIGN_MATURITY_LANE_LIMIT: usize = 64;
/// Question items whose charge series one Design retains; beyond it the least
/// recently charged item's series is dropped.
pub const DESIGN_MATURITY_ITEM_LIMIT: usize = 32;

/// Where one question item stands in its charge series.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DesignItemState {
    /// No charge has weighed the item.
    Quiet,
    /// Only positive charges have weighed the item.
    UnopposedFor,
    /// Only negative charges have weighed the item.
    UnopposedAgainst,
    /// Both signs have weighed the item and its latest run does not concur.
    Contested,
    /// Both signs have weighed the item and its latest one-signed run carries
    /// a concession or three lanes, unburied by a strength-2 opposing charge.
    Concurred,
}

impl DesignItemState {
    /// Operator-facing name of this state.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Quiet => "quiet",
            Self::UnopposedFor | Self::UnopposedAgainst => "unopposed",
            Self::Contested => "contested",
            Self::Concurred => "concurred",
        }
    }

    /// The input that would move the item toward concurrence; none once it
    /// concurs.
    #[must_use]
    pub const fn needs(self) -> Option<&'static str> {
        match self {
            Self::Quiet => Some("any charge"),
            Self::UnopposedFor => Some("a negative charge"),
            Self::UnopposedAgainst => Some("a positive charge"),
            Self::Contested => Some("a strength-2 re-charge"),
            Self::Concurred => None,
        }
    }
}

/// One lane's latest charge sign on one item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignItemLane {
    /// The charging lane.
    pub lane: CanonicalUuid,
    /// Its latest charge on the item was positive.
    pub positive: bool,
}

/// Fixed-size fold of one question item's charged posts in journal order.
///
/// The suffix is the trailing run of same-sign charges; the post just before
/// it is the losing sign's last.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignItemSeries {
    /// The item's text identity.
    pub item: RoomQuestionItem,
    /// Charged posts on the item.
    pub charges: u64,
    /// Some charge was positive.
    pub positive: bool,
    /// Some charge was negative.
    pub negative: bool,
    /// The suffix's sign is positive.
    pub suffix_positive: bool,
    /// Distinct suffix lanes, sorted and capped at
    /// [`DESIGN_MATURITY_LANE_LIMIT`], beyond every threshold.
    pub suffix_lanes: Vec<CanonicalUuid>,
    /// Some suffix charge had strength 2.
    pub suffix_strong: bool,
    /// Some suffix charge came from a lane whose previous charge had the
    /// opposite sign.
    pub suffix_conceded: bool,
    /// The losing sign's last charge had strength 2.
    pub displaced_strong: bool,
    /// The latest charge had strength 2.
    pub last_strong: bool,
    /// Each lane's latest sign, sorted by lane.
    pub lanes: Vec<DesignItemLane>,
    /// More distinct lanes charged than `lanes` retains; an unretained lane's
    /// earlier sign is unknown and concedes nothing.
    pub lanes_truncated: bool,
}

impl DesignItemSeries {
    fn new(item: RoomQuestionItem) -> Self {
        Self {
            item,
            charges: 0,
            positive: false,
            negative: false,
            suffix_positive: false,
            suffix_lanes: Vec::new(),
            suffix_strong: false,
            suffix_conceded: false,
            displaced_strong: false,
            last_strong: false,
            lanes: Vec::new(),
            lanes_truncated: false,
        }
    }

    /// Fold one nonzero charge by `lane`.
    fn observe(&mut self, lane: CanonicalUuid, charge: RoomCharge) {
        let positive = charge.get() > 0;
        let strong = charge.get().unsigned_abs() == 2;
        if self.charges == 0 || positive != self.suffix_positive {
            self.displaced_strong = self.charges != 0 && self.last_strong;
            self.suffix_positive = positive;
            self.suffix_lanes.clear();
            self.suffix_strong = false;
            self.suffix_conceded = false;
        }
        self.charges = self.charges.saturating_add(1);
        if positive {
            self.positive = true;
        } else {
            self.negative = true;
        }
        if let Err(index) = self.suffix_lanes.binary_search(&lane)
            && self.suffix_lanes.len() < DESIGN_MATURITY_LANE_LIMIT
        {
            self.suffix_lanes.insert(index, lane);
        }
        self.suffix_strong |= strong;
        self.last_strong = strong;
        match self.lanes.binary_search_by_key(&lane, |known| known.lane) {
            Ok(index) => {
                let known = &mut self.lanes[index];
                self.suffix_conceded |= known.positive != positive;
                known.positive = positive;
            }
            Err(index) if self.lanes.len() < DESIGN_MATURITY_LANE_LIMIT => {
                self.lanes.insert(index, DesignItemLane { lane, positive });
            }
            Err(_) => self.lanes_truncated = true,
        }
    }

    /// Where the item stands.
    #[must_use]
    pub fn state(&self) -> DesignItemState {
        match (self.positive, self.negative) {
            (false, false) => DesignItemState::Quiet,
            (true, false) => DesignItemState::UnopposedFor,
            (false, true) => DesignItemState::UnopposedAgainst,
            (true, true) => {
                let lanes = self.suffix_lanes.len();
                let agreed = lanes >= 3 || (lanes >= 2 && self.suffix_conceded);
                if agreed && (!self.displaced_strong || self.suffix_strong) {
                    DesignItemState::Concurred
                } else {
                    DesignItemState::Contested
                }
            }
        }
    }

    /// Distinct lanes that charged the item, as retained.
    #[must_use]
    pub fn lanes(&self) -> usize {
        self.lanes.len()
    }
}

/// Every charged question item's series, least recently charged first.
///
/// Only nonzero charges naming an item enter a series; questions, zero
/// charges, and retained Design-level charges never do.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DesignItems(Vec<DesignItemSeries>);

impl DesignItems {
    /// Fold one admitted contribution.
    pub fn observe(&mut self, contribution: &RoomContributed) {
        let (RoomContributionKind::Charge(charge), Some(item)) =
            (contribution.contribution, &contribution.item)
        else {
            return;
        };
        if charge.get() == 0 {
            return;
        }
        let mut series = match self.0.iter().position(|series| &series.item == item) {
            Some(index) => self.0.remove(index),
            None => {
                if self.0.len() == DESIGN_MATURITY_ITEM_LIMIT {
                    self.0.remove(0);
                }
                DesignItemSeries::new(item.clone())
            }
        };
        series.observe(contribution.lane, charge);
        self.0.push(series);
    }

    /// The series of `item`, if a charge weighed it.
    #[must_use]
    pub fn series(&self, item: &RoomQuestionItem) -> Option<&DesignItemSeries> {
        self.0.iter().find(|series| &series.item == item)
    }

    /// Each item of `acceptance`, the Design's current statement, with its
    /// state, in statement order. An item no longer in the statement leaves
    /// the gate.
    #[must_use]
    pub fn states(&self, acceptance: &[String]) -> Vec<DesignItemState> {
        RoomQuestionItem::each(acceptance)
            .map(|item| {
                self.series(&item)
                    .map_or(DesignItemState::Quiet, DesignItemSeries::state)
            })
            .collect()
    }

    /// Whether some current item concurs and none is contested.
    #[must_use]
    pub fn steeped(&self, acceptance: &[String]) -> bool {
        steeped(&self.states(acceptance))
    }

    /// Each current item's standing, in statement order.
    #[must_use]
    pub fn standings(&self, acceptance: &[String]) -> Vec<DesignItemStanding> {
        RoomQuestionItem::each(acceptance)
            .enumerate()
            .filter_map(|(offset, item)| {
                let question = u16::try_from(offset + 1).ok().and_then(RoomQuestion::new)?;
                let series = self.series(&item);
                Some(DesignItemStanding {
                    question,
                    state: series.map_or(DesignItemState::Quiet, DesignItemSeries::state),
                    charges: series.map_or(0, |series| series.charges),
                    lanes: series.map_or(0, DesignItemSeries::lanes),
                })
            })
            .collect()
    }
}

fn steeped(states: &[DesignItemState]) -> bool {
    states.contains(&DesignItemState::Concurred) && !states.contains(&DesignItemState::Contested)
}

/// One recent statement head's request and contribution evidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignMaturityHead {
    /// Exact Design statement head shared by these records.
    pub statement: Seq,
    /// Request records at this head, including repeat lanes.
    pub requests: u64,
    /// Distinct requested lanes retained in sorted order.
    pub requested_lanes: Vec<CanonicalUuid>,
    /// More distinct requested lanes existed than the retained bound.
    pub requested_lanes_truncated: bool,
    /// Contribution records at this head, including repeat lanes.
    pub contributions: u64,
    /// Distinct contributing lanes retained in sorted order.
    pub contributing_lanes: Vec<CanonicalUuid>,
    /// More distinct contributing lanes existed than the retained bound.
    pub contributing_lanes_truncated: bool,
    /// Lanes asked at this head that then left the room, in sorted order.
    pub declining_lanes: Vec<CanonicalUuid>,
    /// More distinct declining lanes existed than the retained bound.
    pub declining_lanes_truncated: bool,
}

impl DesignMaturityHead {
    fn new(statement: Seq) -> Self {
        Self {
            statement,
            requests: 0,
            requested_lanes: Vec::new(),
            requested_lanes_truncated: false,
            contributions: 0,
            contributing_lanes: Vec::new(),
            contributing_lanes_truncated: false,
            declining_lanes: Vec::new(),
            declining_lanes_truncated: false,
        }
    }

    /// Number of retained lanes both requested and contributing at this head.
    #[must_use]
    pub fn requested_contributors(&self) -> usize {
        intersection_count(&self.requested_lanes, &self.contributing_lanes)
    }

    /// Asked lanes that left the room without contributing at this head.
    #[must_use]
    pub fn requested_decliners(&self) -> usize {
        self.declining_lanes.len()
            - intersection_count(&self.declining_lanes, &self.contributing_lanes)
    }

    /// Asked lanes that have neither contributed nor declined at this head.
    #[must_use]
    pub fn requested_unanswered(&self) -> usize {
        self.requested_lanes
            .len()
            .saturating_sub(self.requested_contributors() + self.requested_decliners())
    }

    /// Whether the displayed counts may omit lanes beyond a bound.
    #[must_use]
    pub const fn yield_truncated(&self) -> bool {
        self.requested_lanes_truncated
            || self.contributing_lanes_truncated
            || self.declining_lanes_truncated
    }
}

/// Evidence folded from statement heads older than the exact recent window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignMaturityOlder {
    /// Distinct statement heads folded here.
    pub statement_heads: u64,
    /// Request records across those heads.
    pub requests: u64,
    /// Sum of each head's distinct requested lanes.
    pub requested_lane_heads: u64,
    /// Contribution records across those heads.
    pub contributions: u64,
    /// Sum of each head's distinct contributing lanes.
    pub contributing_lane_heads: u64,
    /// Sum of each head's requested/contributing lane intersection.
    pub requested_contributor_lane_heads: u64,
    /// Sum of each head's asked lanes that declined without contributing.
    pub requested_decliner_lane_heads: u64,
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
    /// Contribution records of every kind, including historical unattributed
    /// rows.
    pub contributions: u64,
    /// Each charged question item's series.
    pub items: DesignItems,
    /// Distinct contributing lanes retained in sorted order.
    pub contributing_lanes: Vec<CanonicalUuid>,
    /// More distinct contributing lanes existed than the retained bound.
    pub contributing_lanes_truncated: bool,
    /// Request records across all statement heads.
    pub requests: u64,
    /// Distinct requested lanes retained in sorted order.
    pub requested_lanes: Vec<CanonicalUuid>,
    /// More distinct requested lanes existed than the retained bound.
    pub requested_lanes_truncated: bool,
    /// Exact recent statement heads, oldest first.
    pub recent_heads: Vec<DesignMaturityHead>,
    /// Fixed-size aggregate of older statement heads.
    pub older_heads: DesignMaturityOlder,
    /// Explicit bucket for historical contributions with no recorded head.
    pub unrecorded_head: UnrecordedMaturityHead,
}

/// One current question item the Design still needs input on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignItemNeed {
    /// The item's position in the current statement.
    pub question: RoomQuestion,
    /// Where it stands.
    pub item: DesignItemState,
}

/// Advisory state judged per current question item.
///
/// Steeped iff some current item concurs and none is contested. Quiet and
/// unopposed items never block; both states count the items still quiet, so
/// a first pass knows what it leaves open.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum DesignMaturityState {
    /// No contribution exists.
    Fresh,
    /// No current item concurs, or one is contested.
    Steeping {
        /// The lowest-index contested item, else the lowest-index item that
        /// is quiet or unopposed; none when the statement names no item.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next: Option<DesignItemNeed>,
        /// Current items no charge has weighed.
        quiet: usize,
    },
    /// Some current item concurs and none is contested.
    Steeped {
        /// Current items no charge has weighed.
        quiet: usize,
    },
}

impl DesignMaturityState {
    /// One-line operator summary of where the Design stands.
    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            Self::Fresh => "fresh: no contribution yet".to_owned(),
            Self::Steeped { quiet } => format!("steeped; {} left quiet", quiet_items(*quiet)),
            Self::Steeping {
                next: Some(need),
                quiet,
            } => format!(
                "steeping; item {} is {} and needs {}; {} quiet",
                need.question,
                need.item.label(),
                need.item.needs().unwrap_or("nothing"),
                quiet_items(*quiet),
            ),
            Self::Steeping { next: None, .. } => {
                "steeping; the statement names no question item".to_owned()
            }
        }
    }
}

fn quiet_items(quiet: usize) -> String {
    match quiet {
        1 => "1 item".to_owned(),
        count => format!("{count} items"),
    }
}

/// One current question item's standing in its charge series.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignItemStanding {
    /// The item's position in the current statement.
    pub question: RoomQuestion,
    /// Where it stands.
    pub state: DesignItemState,
    /// Nonzero charges its series has seen.
    pub charges: u64,
    /// Distinct lanes that charged it, bounded.
    pub lanes: usize,
}

impl DesignMaturity {
    /// Fold one admitted Design request.
    pub fn observe_request(&mut self, notice: &DesignNoticed) {
        self.requests = self.requests.saturating_add(1);
        insert_bounded(
            &mut self.requested_lanes,
            &mut self.requested_lanes_truncated,
            notice.lane,
        );
        self.observe_head(notice.statement, |head| {
            head.requests = head.requests.saturating_add(1);
            insert_bounded(
                &mut head.requested_lanes,
                &mut head.requested_lanes_truncated,
                notice.lane,
            );
        });
    }

    /// Fold one admitted contribution.
    pub fn observe_contribution(&mut self, contribution: &RoomContributed) {
        self.contributions = self.contributions.saturating_add(1);
        self.items.observe(contribution);
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

    /// Fold one room leave. Only a lane asked at a retained statement head
    /// declines, at the newest head that asked it; any other leave is not an
    /// answer to a request and leaves maturity unchanged. A decline never
    /// changes [`Self::state`].
    pub fn observe_leave(&mut self, leave: &RoomLeft) {
        let Some(head) = self
            .recent_heads
            .iter_mut()
            .rev()
            .find(|head| head.requested_lanes.binary_search(&leave.lane).is_ok())
        else {
            return;
        };
        insert_bounded(
            &mut head.declining_lanes,
            &mut head.declining_lanes_truncated,
            leave.lane,
        );
    }

    /// Current advisory maturity state against `acceptance`, the Design's
    /// current statement items. Requested reach, questions, zero charges,
    /// and retained Design-level charges never move an item.
    #[must_use]
    pub fn state(&self, acceptance: &[String]) -> DesignMaturityState {
        if self.contributions == 0 {
            return DesignMaturityState::Fresh;
        }
        let states = self.items.states(acceptance);
        let quiet = states
            .iter()
            .filter(|state| **state == DesignItemState::Quiet)
            .count();
        if steeped(&states) {
            return DesignMaturityState::Steeped { quiet };
        }
        let next = states
            .iter()
            .position(|state| *state == DesignItemState::Contested)
            .or_else(|| {
                states
                    .iter()
                    .position(|state| *state != DesignItemState::Concurred)
            })
            .and_then(|offset| {
                let question = u16::try_from(offset + 1).ok().and_then(RoomQuestion::new)?;
                Some(DesignItemNeed {
                    question,
                    item: states[offset],
                })
            });
        DesignMaturityState::Steeping { next, quiet }
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
        older.requests = older.requests.saturating_add(head.requests);
        older.requested_lane_heads = older
            .requested_lane_heads
            .saturating_add(head.requested_lanes.len() as u64);
        older.contributions = older.contributions.saturating_add(head.contributions);
        older.contributing_lane_heads = older
            .contributing_lane_heads
            .saturating_add(head.contributing_lanes.len() as u64);
        older.requested_contributor_lane_heads = older
            .requested_contributor_lane_heads
            .saturating_add(head.requested_contributors() as u64);
        older.requested_decliner_lane_heads = older
            .requested_decliner_lane_heads
            .saturating_add(head.requested_decliners() as u64);
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
    use crate::TaskStamp;
    use crate::rooms::{
        DesignNoticeEvidence, DesignNoticeOpportunity, DesignSession, DesignSupport,
        RoomAuthorityStamp,
    };

    fn lane(last: u8) -> CanonicalUuid {
        format!("00000000-0000-4000-8000-{last:012}")
            .parse()
            .expect("fixture lane")
    }

    fn task(micros: i64) -> TaskStamp {
        TaskStamp::for_micros(micros).expect("fixture task")
    }

    fn notice(statement: u64, lane: u8) -> DesignNoticed {
        let evidence_task = task(2);
        let completed = Seq::new(statement.saturating_add(200)).expect("completion");
        DesignNoticed {
            design: task(1),
            statement: Seq::new(statement).expect("statement"),
            lane: self::lane(lane),
            occupant: self::lane(lane.saturating_add(100)),
            session: DesignSession {
                vendor: "omp".parse().expect("vendor"),
                identity: format!("session-{lane}"),
            },
            compaction: None,
            opportunity: DesignNoticeOpportunity::TaskCompletion {
                task: evidence_task.clone(),
                completed,
            },
            evidence: DesignNoticeEvidence::Task {
                task: evidence_task,
                statement: Seq::new(statement.saturating_add(1)).expect("task statement"),
                completed,
                support: DesignSupport::Implemented,
            },
            excerpt: "measured design maturity".to_owned(),
            terms: vec![
                "design".to_owned(),
                "maturity".to_owned(),
                "measure".to_owned(),
            ],
            stance: None,
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
            question: None,
            item: None,
            statement: statement.and_then(Seq::new),
            text: "Substantive contribution.".to_owned(),
            sources: Vec::new(),
        }
    }

    const ITEMS: [&str; 2] = ["Which input steeps a Design?", "Who answers it?"];

    /// Lane `lane`'s charge `value` on the item whose text is `item`.
    fn charge(lane: u8, item: &str, value: i8) -> RoomContributed {
        RoomContributed {
            question: RoomQuestion::new(1),
            item: Some(RoomQuestionItem::of(item)),
            ..contribution(Some(10), lane, RoomContributionKind::charge(value))
        }
    }

    fn item_state(maturity: &DesignMaturity, item: &str) -> DesignItemState {
        maturity.items.states(&[item.to_owned()])[0]
    }

    fn acceptance(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    fn need(question: u16, item: DesignItemState) -> Option<DesignItemNeed> {
        Some(DesignItemNeed {
            question: RoomQuestion::new(question).expect("item"),
            item,
        })
    }

    fn leave(lane: u8) -> RoomLeft {
        RoomLeft {
            room: task(1),
            publication: RoomAuthorityStamp::for_micros(5_000 + i64::from(lane))
                .expect("publication"),
            lane: self::lane(lane),
            clause: "nothing to add".to_owned(),
        }
    }

    /// `(statement, asked, contributed, declined, unanswered)` at each
    /// retained head.
    fn answers(maturity: &DesignMaturity) -> Vec<(u64, usize, usize, usize, usize)> {
        maturity
            .recent_heads
            .iter()
            .map(|head| {
                (
                    head.statement.get(),
                    head.requested_lanes.len(),
                    head.requested_contributors(),
                    head.requested_decliners(),
                    head.requested_unanswered(),
                )
            })
            .collect()
    }

    #[test]
    fn declines_count_per_asking_head_and_never_move_state() {
        let mut maturity = DesignMaturity::default();
        for lane in 1..=3 {
            maturity.observe_request(&notice(10, lane));
        }
        maturity.observe_contribution(&contribution(Some(10), 1, RoomContributionKind::charge(0)));
        let state = maturity.state(&acceptance(&ITEMS));
        maturity.observe_leave(&leave(2));
        assert_eq!(answers(&maturity), [(10, 3, 1, 1, 1)]);

        maturity.observe_request(&notice(20, 4));
        maturity.observe_request(&notice(20, 5));
        maturity.observe_leave(&leave(4));
        maturity.observe_leave(&leave(3));
        maturity.observe_leave(&leave(9));
        assert_eq!(
            answers(&maturity),
            [(10, 3, 1, 2, 0), (20, 2, 0, 1, 1)],
            "each asked lane declines at the head that asked it; an unasked leave is no decline"
        );
        assert_eq!(
            maturity.state(&acceptance(&ITEMS)),
            state,
            "declines never move maturity state"
        );

        maturity.observe_contribution(&contribution(Some(20), 4, RoomContributionKind::Question));
        assert_eq!(
            answers(&maturity)[1],
            (20, 2, 1, 0, 1),
            "a later contribution answers instead of the decline"
        );
    }

    #[test]
    fn an_item_concurs_only_on_a_conceded_or_three_lane_unburied_run() {
        let item = ITEMS[0];
        let mut maturity = DesignMaturity::default();
        assert_eq!(item_state(&maturity, item), DesignItemState::Quiet);
        maturity.observe_contribution(&contribution(Some(10), 1, RoomContributionKind::charge(0)));
        maturity.observe_contribution(&RoomContributed {
            question: RoomQuestion::new(1),
            item: Some(RoomQuestionItem::of(item)),
            ..contribution(Some(10), 1, RoomContributionKind::Question)
        });
        maturity.observe_contribution(&RoomContributed {
            contribution: RoomContributionKind::charge(0),
            ..charge(1, item, 1)
        });
        assert_eq!(
            item_state(&maturity, item),
            DesignItemState::Quiet,
            "zero charges and questions never enter a series"
        );

        maturity.observe_contribution(&charge(1, item, 1));
        assert_eq!(item_state(&maturity, item), DesignItemState::UnopposedFor);
        assert_eq!(
            DesignItemState::UnopposedFor.needs(),
            Some("a negative charge")
        );
        maturity.observe_contribution(&charge(1, item, -1));
        assert_eq!(
            item_state(&maturity, item),
            DesignItemState::Contested,
            "a lane flipping its own sign sees both signs but cannot alone concur"
        );
        maturity.observe_contribution(&charge(2, item, -1));
        assert_eq!(
            item_state(&maturity, item),
            DesignItemState::Concurred,
            "two suffix lanes, one of which conceded"
        );
        maturity.observe_contribution(&charge(3, item, 1));
        assert_eq!(item_state(&maturity, item), DesignItemState::Contested);

        let mut buried = DesignMaturity::default();
        for (lane, value) in [(1, -2), (2, 1), (3, 1), (4, 1)] {
            buried.observe_contribution(&charge(lane, item, value));
        }
        assert_eq!(
            item_state(&buried, item),
            DesignItemState::Contested,
            "three unconceded lanes cannot bury a strength-2 charge without one"
        );
        buried.observe_contribution(&charge(5, item, 2));
        assert_eq!(item_state(&buried, item), DesignItemState::Concurred);

        let mut unconceded = DesignMaturity::default();
        for (lane, value) in [(1, -1), (2, 1), (3, 1)] {
            unconceded.observe_contribution(&charge(lane, item, value));
        }
        assert_eq!(
            item_state(&unconceded, item),
            DesignItemState::Contested,
            "two lanes without a concession do not concur"
        );
        unconceded.observe_contribution(&charge(4, item, 1));
        assert_eq!(item_state(&unconceded, item), DesignItemState::Concurred);
    }

    #[test]
    fn a_design_steeps_iff_a_current_item_concurs_and_none_is_contested() {
        let mut maturity = DesignMaturity::default();
        let current = acceptance(&ITEMS);
        assert_eq!(maturity.state(&current), DesignMaturityState::Fresh);
        maturity.observe_contribution(&contribution(Some(10), 9, RoomContributionKind::charge(1)));
        assert_eq!(
            maturity.state(&current),
            DesignMaturityState::Steeping {
                next: need(1, DesignItemState::Quiet),
                quiet: 2,
            },
            "a retained Design-level charge enters no item"
        );

        for (lane, value) in [(1, 1), (2, -1), (1, -1)] {
            maturity.observe_contribution(&charge(lane, ITEMS[0], value));
        }
        assert_eq!(
            maturity.state(&current),
            DesignMaturityState::Steeped { quiet: 1 },
            "three charged posts from two lanes with a concession steep one item"
        );

        maturity.observe_contribution(&charge(3, ITEMS[1], 1));
        assert_eq!(
            maturity.state(&current),
            DesignMaturityState::Steeped { quiet: 0 },
            "an unopposed item never blocks"
        );
        maturity.observe_contribution(&charge(4, ITEMS[1], -1));
        assert_eq!(
            maturity.state(&current),
            DesignMaturityState::Steeping {
                next: need(2, DesignItemState::Contested),
                quiet: 0,
            }
        );

        let reordered = acceptance(&["  who ANSWERS\tit? ", ITEMS[0]]);
        assert_eq!(
            maturity.state(&reordered),
            DesignMaturityState::Steeping {
                next: need(1, DesignItemState::Contested),
                quiet: 0,
            },
            "a reorder or respacing keeps each series"
        );
        let reworded = acceptance(&[ITEMS[0], "Who answers it first?"]);
        assert_eq!(
            maturity.state(&reworded),
            DesignMaturityState::Steeped { quiet: 1 },
            "a reworded item starts a new series and the old one leaves the gate"
        );
        assert_eq!(
            maturity.state(&acceptance(&ITEMS[..1])),
            DesignMaturityState::Steeped { quiet: 0 },
            "a removed item leaves the gate"
        );
    }

    #[test]
    fn head_yield_history_and_memory_are_bounded() {
        let mut maturity = DesignMaturity::default();
        for statement in 1..=6 {
            maturity.observe_request(&notice(statement, 1));
            maturity.observe_request(&notice(statement, 2));
            maturity.observe_contribution(&contribution(
                Some(statement),
                2,
                RoomContributionKind::charge(0),
            ));
        }
        maturity.observe_contribution(&contribution(None, 9, RoomContributionKind::charge(-1)));

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
                .all(|head| head.requested_contributors() == 1)
        );
        assert_eq!(maturity.older_heads.statement_heads, 2);
        assert_eq!(maturity.older_heads.requested_contributor_lane_heads, 2);
        assert_eq!(maturity.unrecorded_head.contributions, 1);
        assert_eq!(maturity.unrecorded_head.contributing_lanes, vec![lane(9)]);
        for lane in 10..=100 {
            maturity.observe_request(&notice(6, lane));
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
        assert_eq!(current.requested_lanes.len(), DESIGN_MATURITY_LANE_LIMIT);
        assert_eq!(current.contributing_lanes.len(), DESIGN_MATURITY_LANE_LIMIT);
        assert!(current.requested_lanes_truncated);
        assert!(current.contributing_lanes_truncated);
        assert_eq!(
            maturity.contributing_lanes.len(),
            DESIGN_MATURITY_LANE_LIMIT
        );
        assert!(maturity.contributing_lanes_truncated);

        let items = (0..=DESIGN_MATURITY_ITEM_LIMIT)
            .map(|item| format!("Item {item}?"))
            .collect::<Vec<_>>();
        maturity.observe_contribution(&charge(1, &items[0], 1));
        for item in &items[1..] {
            maturity.observe_contribution(&charge(1, item, 1));
        }
        assert_eq!(
            maturity.items.states(&items[..1]),
            [DesignItemState::Quiet],
            "the least recently charged series is dropped beyond the item bound"
        );
        assert!(
            maturity
                .items
                .states(&items[1..])
                .iter()
                .all(|state| *state == DesignItemState::UnopposedFor)
        );
    }

    #[test]
    fn historical_contribution_omission_decodes_explicitly() {
        let current = contribution(Some(10), 1, RoomContributionKind::charge(0));
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
