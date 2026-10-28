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
    DesignAsk, DesignNoticed, RoomCharge, RoomContributed, RoomContributionKind, RoomLeft,
    RoomQuestion, RoomQuestionItem,
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
    /// Only positive charges have weighed the item, short of the agreement
    /// floor.
    UnopposedFor,
    /// Only negative charges have weighed the item, short of the agreement
    /// floor.
    UnopposedAgainst,
    /// Both signs have weighed the item and its latest run does not concur.
    Contested,
    /// The item's latest one-signed run is agreed: three lanes, or two lanes
    /// with a strength-2 charge when only that sign ever weighed the item, or
    /// two lanes with a concession after both signs did. A run that displaced
    /// a strength-2 opposing charge also carries a strength-2 charge.
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
}

/// The input that would move one question item toward concurrence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DesignItemInput {
    /// No charge has weighed the item.
    AnyCharge,
    /// Only positive charges have weighed the item, short of three lanes or
    /// two with a +2 charge: more positive lanes.
    PositiveLanes,
    /// Only negative charges have weighed the item, short of three lanes or
    /// two with a -2 charge: more negative lanes.
    NegativeLanes,
    /// The positive run lacks a concession and three lanes: a positive
    /// charge from a lane whose last charge was negative, or more positive
    /// lanes.
    PositiveAgreement,
    /// The negative run lacks a concession and three lanes.
    NegativeAgreement,
    /// The agreed positive run has not answered the displaced strength-2
    /// negative charge with a +2 of its own.
    PositiveStrength,
    /// The agreed negative run has not answered the displaced strength-2
    /// positive charge with a -2 of its own.
    NegativeStrength,
}

impl DesignItemInput {
    /// Operator-facing words for this input.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::AnyCharge => "any charge",
            Self::PositiveLanes => "more positive lanes",
            Self::NegativeLanes => "more negative lanes",
            Self::PositiveAgreement => {
                "a positive charge from a lane that last charged negative, or more positive lanes"
            }
            Self::NegativeAgreement => {
                "a negative charge from a lane that last charged positive, or more negative lanes"
            }
            Self::PositiveStrength => "a +2 charge",
            Self::NegativeStrength => "a -2 charge",
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

    /// The suffix holds a concession and two lanes, or three lanes.
    fn agreed(&self) -> bool {
        let lanes = self.suffix_lanes.len();
        lanes >= 3 || (lanes >= 2 && self.suffix_conceded)
    }

    /// A run no opposing charge preceded holds three lanes, or two with a
    /// strength-2 charge.
    fn unanimous(&self) -> bool {
        let lanes = self.suffix_lanes.len();
        lanes >= 3 || (lanes >= 2 && self.suffix_strong)
    }

    /// The suffix displaced a strength-2 charge without one of its own.
    fn buried(&self) -> bool {
        self.displaced_strong && !self.suffix_strong
    }

    /// Where the item stands.
    #[must_use]
    pub fn state(&self) -> DesignItemState {
        match (self.positive, self.negative) {
            (false, false) => DesignItemState::Quiet,
            (true, false) | (false, true) if self.unanimous() => DesignItemState::Concurred,
            (true, false) => DesignItemState::UnopposedFor,
            (false, true) => DesignItemState::UnopposedAgainst,
            (true, true) if self.agreed() && !self.buried() => DesignItemState::Concurred,
            (true, true) => DesignItemState::Contested,
        }
    }

    /// The input that would move the item toward concurrence; none once it
    /// concurs. A contested run first needs agreement, then any strength-2
    /// answer it owes.
    #[must_use]
    pub fn needs(&self) -> Option<DesignItemInput> {
        let positive = self.suffix_positive;
        Some(match self.state() {
            DesignItemState::Quiet => DesignItemInput::AnyCharge,
            DesignItemState::UnopposedFor => DesignItemInput::PositiveLanes,
            DesignItemState::UnopposedAgainst => DesignItemInput::NegativeLanes,
            DesignItemState::Contested if !self.agreed() && positive => {
                DesignItemInput::PositiveAgreement
            }
            DesignItemState::Contested if !self.agreed() => DesignItemInput::NegativeAgreement,
            DesignItemState::Contested if positive => DesignItemInput::PositiveStrength,
            DesignItemState::Contested => DesignItemInput::NegativeStrength,
            DesignItemState::Concurred => return None,
        })
    }

    /// Distinct lanes that charged the item, as retained.
    #[must_use]
    pub fn lanes(&self) -> usize {
        self.lanes.len()
    }

    /// Whether `lane`'s latest charge on the item is retained.
    #[must_use]
    pub fn holds(&self, lane: CanonicalUuid) -> bool {
        self.lanes
            .binary_search_by_key(&lane, |known| known.lane)
            .is_ok()
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

    /// Whether at least 80% of current items have been charged, one concurs,
    /// and none is contested.
    #[must_use]
    pub fn steeped(&self, acceptance: &[String]) -> bool {
        steeped(&self.states(acceptance))
    }

    /// Whether the statement names at least one item and every current item
    /// concurs, leaving nothing for an outside lane to weigh.
    #[must_use]
    pub fn all_concurred(&self, acceptance: &[String]) -> bool {
        let states = self.states(acceptance);
        !states.is_empty()
            && states
                .iter()
                .all(|state| *state == DesignItemState::Concurred)
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

    /// The current items `lane` may be asked about, in statement order. A lane
    /// outside an item's series is asked about its quiet and unopposed items.
    /// While some item is contested, a lane already in a contested item's
    /// series is instead asked to re-charge it at strength 2; a lane in no
    /// contested series keeps the outside asks. A concurred item is never
    /// asked.
    #[must_use]
    pub fn askable(&self, acceptance: &[String], lane: CanonicalUuid) -> Vec<DesignAsk> {
        let mut outside = Vec::new();
        let mut recharge = Vec::new();
        for (offset, item) in RoomQuestionItem::each(acceptance).enumerate() {
            let Some(question) = u16::try_from(offset + 1).ok().and_then(RoomQuestion::new) else {
                continue;
            };
            let series = self.series(&item);
            let state = series.map_or(DesignItemState::Quiet, DesignItemSeries::state);
            let holds = series.is_some_and(|series| series.holds(lane));
            let ask = DesignAsk {
                question,
                item,
                state,
            };
            match state {
                DesignItemState::Quiet
                | DesignItemState::UnopposedFor
                | DesignItemState::UnopposedAgainst
                    if !holds =>
                {
                    outside.push(ask);
                }
                DesignItemState::Contested if holds => {
                    recharge.push(ask);
                }
                _ => {}
            }
        }
        if recharge.is_empty() {
            outside
        } else {
            recharge
        }
    }
}

fn steeped(states: &[DesignItemState]) -> bool {
    let mut quiet = 0;
    let mut concurred = false;
    for state in states {
        match state {
            DesignItemState::Quiet => quiet += 1,
            DesignItemState::Concurred => concurred = true,
            DesignItemState::Contested => return false,
            DesignItemState::UnopposedFor | DesignItemState::UnopposedAgainst => {}
        }
    }
    // At most one fifth may be quiet; integer division rounds coverage up.
    concurred && quiet <= states.len() / 5
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
    /// The input that would move it toward concurrence.
    pub needs: DesignItemInput,
}

/// Advisory state judged per current question item.
///
/// Steeped iff at least 80% of current items have been charged, some item
/// concurs, and none is contested. Unopposed items count as heard; up to 20%
/// may remain quiet. Steeped maturity narrows the Design's invitation reach.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum DesignMaturityState {
    /// No contribution exists.
    Fresh,
    /// Too few current items are charged, some item is contested, or none concurs.
    Steeping {
        /// The lowest-index contested item, else the lowest-index quiet item,
        /// else the lowest-index unopposed item; none when the statement names
        /// no item.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next: Option<DesignItemNeed>,
        /// Current items no charge has weighed.
        quiet: usize,
    },
    /// At least 80% of current items are charged, some concurs, and none is contested.
    Steeped,
}

impl DesignMaturityState {
    /// One-line operator summary of where the Design stands.
    #[must_use]
    pub fn summary(&self) -> String {
        match self {
            Self::Fresh => "fresh: no contribution yet".to_owned(),
            Self::Steeped => "steeped".to_owned(),
            Self::Steeping {
                next: Some(need),
                quiet,
            } => format!(
                "steeping; item {} is {} and needs {}; {} quiet",
                need.question,
                need.item.label(),
                need.needs.label(),
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
            return DesignMaturityState::Steeped;
        }
        let lowest = |wanted: fn(&DesignItemState) -> bool| states.iter().position(wanted);
        let next = lowest(|state| *state == DesignItemState::Contested)
            .or_else(|| lowest(|state| *state == DesignItemState::Quiet))
            .or_else(|| lowest(|state| *state != DesignItemState::Concurred))
            .and_then(|offset| {
                let question = u16::try_from(offset + 1).ok().and_then(RoomQuestion::new)?;
                let needs = self
                    .items
                    .series(&RoomQuestionItem::of(&acceptance[offset]))
                    .map_or(Some(DesignItemInput::AnyCharge), DesignItemSeries::needs)?;
                Some(DesignItemNeed {
                    question,
                    item: states[offset],
                    needs,
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
            reach: None,
            asks: Vec::new(),
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

    fn item_needs(maturity: &DesignMaturity, item: &str) -> Option<DesignItemInput> {
        maturity
            .items
            .series(&RoomQuestionItem::of(item))
            .and_then(DesignItemSeries::needs)
    }

    fn acceptance(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    fn need(
        question: u16,
        item: DesignItemState,
        needs: DesignItemInput,
    ) -> Option<DesignItemNeed> {
        Some(DesignItemNeed {
            question: RoomQuestion::new(question).expect("item"),
            item,
            needs,
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
            item_needs(&maturity, item),
            Some(DesignItemInput::PositiveLanes)
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

        let mut opposed = DesignMaturity::default();
        for (lane, value) in [(1, 1), (2, -1)] {
            opposed.observe_contribution(&charge(lane, item, value));
        }
        assert_eq!(
            item_needs(&opposed, item),
            Some(DesignItemInput::NegativeAgreement),
            "an unagreed run needs a concession or more lanes, not strength"
        );
        let mut conceded = opposed.clone();
        conceded.observe_contribution(&charge(1, item, -1));
        assert_eq!(item_needs(&conceded, item), None, "the concession concurs");
        opposed.observe_contribution(&charge(1, item, 2));
        assert_eq!(
            item_state(&opposed, item),
            DesignItemState::Contested,
            "a strength-2 re-charge does not supply agreement"
        );

        let mut buried = DesignMaturity::default();
        for (lane, value) in [(1, -2), (2, 1), (3, 1), (4, 1)] {
            buried.observe_contribution(&charge(lane, item, value));
        }
        assert_eq!(
            item_state(&buried, item),
            DesignItemState::Contested,
            "three unconceded lanes cannot bury a strength-2 charge without one"
        );
        assert_eq!(
            item_needs(&buried, item),
            Some(DesignItemInput::PositiveStrength)
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

    /// A run no opposing charge preceded concurs at three lanes, or two with
    /// a strength-2 charge; short of that it asks for more lanes of its sign.
    #[test]
    fn a_one_signed_item_concurs_at_the_agreement_floor() {
        let item = ITEMS[0];
        let observed = |charges: &[(u8, i8)]| {
            let mut maturity = DesignMaturity::default();
            for &(lane, value) in charges {
                maturity.observe_contribution(&charge(lane, item, value));
            }
            maturity
        };

        let lone_strong = observed(&[(1, 2)]);
        assert_eq!(
            item_state(&lone_strong, item),
            DesignItemState::UnopposedFor
        );
        assert_eq!(
            item_needs(&lone_strong, item),
            Some(DesignItemInput::PositiveLanes)
        );
        let two_weak = observed(&[(1, 1), (2, 1), (1, 1)]);
        assert_eq!(
            item_state(&two_weak, item),
            DesignItemState::UnopposedFor,
            "a repeated lane adds no agreement"
        );
        assert_eq!(
            item_needs(&two_weak, item),
            Some(DesignItemInput::PositiveLanes)
        );
        let two_strong = observed(&[(1, 1), (2, 2)]);
        assert_eq!(item_state(&two_strong, item), DesignItemState::Concurred);
        assert_eq!(item_needs(&two_strong, item), None);
        let three = observed(&[(1, 1), (2, 1), (3, 1)]);
        assert_eq!(item_state(&three, item), DesignItemState::Concurred);
        assert_eq!(item_needs(&three, item), None);

        let against = observed(&[(1, -1), (2, -1)]);
        assert_eq!(
            item_state(&against, item),
            DesignItemState::UnopposedAgainst
        );
        assert_eq!(
            item_needs(&against, item),
            Some(DesignItemInput::NegativeLanes)
        );
        assert_eq!(
            item_state(&observed(&[(1, -1), (2, -1), (3, -1)]), item),
            DesignItemState::Concurred
        );

        let current = acceptance(&ITEMS);
        let mut contested = three;
        for (lane, value) in [(4, 1), (5, -1)] {
            contested.observe_contribution(&charge(lane, ITEMS[1], value));
        }
        assert_eq!(
            contested.state(&current),
            DesignMaturityState::Steeping {
                next: need(
                    2,
                    DesignItemState::Contested,
                    DesignItemInput::NegativeAgreement
                ),
                quiet: 0,
            }
        );
    }

    #[test]
    fn eighty_percent_coverage_rounds_up_to_whole_questions() {
        for (total, answered, expected) in [
            (0, 0, false),
            (1, 0, false),
            (1, 1, true),
            (4, 3, false),
            (4, 4, true),
            (5, 3, false),
            (5, 4, true),
            (6, 4, false),
            (6, 5, true),
            (10, 8, true),
            (32, 25, false),
            (32, 26, true),
        ] {
            let current: Vec<_> = (1..=total)
                .map(|question| format!("Who answers question {question}?"))
                .collect();
            let mut maturity = DesignMaturity::default();
            for item in current.iter().take(answered) {
                maturity.observe_contribution(&charge(1, item, -1));
            }
            if answered > 0 {
                maturity.observe_contribution(&charge(2, &current[0], -2));
            }
            assert_eq!(
                maturity.items.steeped(&current),
                expected,
                "{answered}/{total} questions answered"
            );
        }
    }

    #[test]
    fn eighty_percent_coverage_preserves_concurrence_and_contention_guards() {
        let mut current = acceptance(&[
            "Who defines the input?",
            "Who owns the state?",
            "Who reports errors?",
            "Who checks the result?",
            "Who handles cleanup?",
        ]);
        let mut maturity = DesignMaturity::default();
        for item in &current[..4] {
            maturity.observe_contribution(&charge(1, item, 1));
        }
        assert_eq!(
            maturity.state(&current),
            DesignMaturityState::Steeping {
                next: need(5, DesignItemState::Quiet, DesignItemInput::AnyCharge),
                quiet: 1,
            }
        );
        maturity.observe_contribution(&charge(2, &current[0], 2));
        assert_eq!(maturity.state(&current), DesignMaturityState::Steeped);

        current.push("Who handles cancellation?".to_owned());
        assert_eq!(
            maturity.state(&current),
            DesignMaturityState::Steeping {
                next: need(5, DesignItemState::Quiet, DesignItemInput::AnyCharge),
                quiet: 2,
            }
        );
        current.pop();
        assert_eq!(
            maturity.state(&[]),
            DesignMaturityState::Steeping {
                next: None,
                quiet: 0,
            }
        );

        maturity.observe_contribution(&charge(2, &current[1], -1));
        assert_eq!(
            maturity.state(&current),
            DesignMaturityState::Steeping {
                next: need(
                    2,
                    DesignItemState::Contested,
                    DesignItemInput::NegativeAgreement,
                ),
                quiet: 1,
            }
        );
    }

    #[test]
    fn a_design_tracks_current_items_and_prioritizes_contested_input() {
        let mut maturity = DesignMaturity::default();
        let current = acceptance(&ITEMS);
        assert_eq!(maturity.state(&current), DesignMaturityState::Fresh);
        maturity.observe_contribution(&contribution(Some(10), 9, RoomContributionKind::charge(1)));
        assert_eq!(
            maturity.state(&current),
            DesignMaturityState::Steeping {
                next: need(1, DesignItemState::Quiet, DesignItemInput::AnyCharge),
                quiet: 2,
            },
            "a retained Design-level charge enters no item"
        );

        for (lane, value) in [(1, 1), (2, -1), (1, -1)] {
            maturity.observe_contribution(&charge(lane, ITEMS[0], value));
        }
        assert_eq!(
            maturity.state(&current),
            DesignMaturityState::Steeping {
                next: need(2, DesignItemState::Quiet, DesignItemInput::AnyCharge),
                quiet: 1,
            },
            "a quiet item keeps a Design steeping although another item concurs"
        );

        maturity.observe_contribution(&charge(3, ITEMS[1], 1));
        assert_eq!(
            maturity.state(&current),
            DesignMaturityState::Steeped,
            "once every item is heard, an unopposed item never blocks"
        );
        assert_eq!(
            maturity.state(&acceptance(&[ITEMS[1], ITEMS[0], "Who else weighs it?"])),
            DesignMaturityState::Steeping {
                next: need(3, DesignItemState::Quiet, DesignItemInput::AnyCharge),
                quiet: 1,
            },
            "a quiet item is named before a lower unopposed one"
        );
        maturity.observe_contribution(&charge(4, ITEMS[1], -1));
        assert_eq!(
            maturity.state(&current),
            DesignMaturityState::Steeping {
                next: need(
                    2,
                    DesignItemState::Contested,
                    DesignItemInput::NegativeAgreement
                ),
                quiet: 0,
            }
        );
        assert_eq!(
            maturity.state(&acceptance(&["Who else weighs it?", ITEMS[0], ITEMS[1]])),
            DesignMaturityState::Steeping {
                next: need(
                    3,
                    DesignItemState::Contested,
                    DesignItemInput::NegativeAgreement
                ),
                quiet: 1,
            },
            "a contested item is named before a lower quiet one"
        );

        let reordered = acceptance(&["  who ANSWERS\tit? ", ITEMS[0]]);
        assert_eq!(
            maturity.state(&reordered),
            DesignMaturityState::Steeping {
                next: need(
                    1,
                    DesignItemState::Contested,
                    DesignItemInput::NegativeAgreement
                ),
                quiet: 0,
            },
            "a reorder or respacing keeps each series"
        );
        let reworded = acceptance(&[ITEMS[0], "Who answers it first?"]);
        assert_eq!(
            maturity.state(&reworded),
            DesignMaturityState::Steeping {
                next: need(2, DesignItemState::Quiet, DesignItemInput::AnyCharge),
                quiet: 1,
            },
            "a reworded item starts a new series that must be heard again"
        );
        assert_eq!(
            maturity.state(&acceptance(&ITEMS[..1])),
            DesignMaturityState::Steeped,
            "a removed item leaves the gate"
        );
    }

    #[test]
    fn every_item_concurring_is_judged_apart_from_steeped() {
        let mut maturity = DesignMaturity::default();
        let current = acceptance(&ITEMS);
        assert!(!maturity.items.all_concurred(&current));
        assert!(
            !maturity.items.all_concurred(&[]),
            "a statement without items leaves nothing to concur"
        );
        for lane in 1..=3 {
            maturity.observe_contribution(&charge(lane, ITEMS[0], 1));
        }
        assert!(
            !maturity.items.steeped(&current),
            "a quiet item keeps the statement steeping"
        );
        maturity.observe_contribution(&charge(4, ITEMS[1], 1));
        assert!(maturity.items.steeped(&current));
        assert!(
            !maturity.items.all_concurred(&current),
            "an unopposed item is still open to an outside lane"
        );
        for lane in 1..=3 {
            maturity.observe_contribution(&charge(lane, ITEMS[1], -1));
        }
        assert!(maturity.items.all_concurred(&current));
        assert!(
            !maturity
                .items
                .all_concurred(&acceptance(&[ITEMS[0], "Who answers it first?"])),
            "a reworded item reopens the statement"
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

    /// A lane outside an item's series is asked about its quiet and unopposed
    /// items; a lane in a contested series is asked to re-charge only that
    /// item, and is asked as an outside lane once nothing is contested. A
    /// concurred item is never asked.
    #[test]
    fn askable_items_follow_the_items_and_the_lanes_series() {
        use DesignItemState::{Contested, Quiet, UnopposedFor};

        let items = [
            "Is the first item quiet?",
            "Is the second item unopposed?",
            "Is the third item contested?",
            "Is the fourth item concurred?",
        ];
        let current = acceptance(&items);
        let mut maturity = DesignMaturity::default();
        for (lane, item, value) in [
            (1, items[1], 1),
            (3, items[2], 1),
            (2, items[2], -1),
            (3, items[3], 1),
            (3, items[3], -1),
            (4, items[3], -1),
        ] {
            maturity.observe_contribution(&charge(lane, item, value));
        }
        let asked = |maturity: &DesignMaturity, lane| {
            maturity
                .items
                .askable(&current, self::lane(lane))
                .into_iter()
                .map(|ask| (ask.question.offset() + 1, ask.state))
                .collect::<Vec<_>>()
        };

        assert_eq!(
            asked(&maturity, 9),
            [(1, Quiet), (2, UnopposedFor)],
            "an outside lane is asked every quiet and unopposed item"
        );
        assert_eq!(
            asked(&maturity, 1),
            [(1, Quiet)],
            "an item whose series holds the lane is not asked of it"
        );
        assert_eq!(asked(&maturity, 2), [(3, Contested)]);

        maturity.observe_contribution(&charge(3, items[2], -1));
        assert_eq!(
            asked(&maturity, 2),
            [(1, Quiet), (2, UnopposedFor)],
            "with nothing contested a series lane is asked as an outside lane"
        );
    }
}
