//! Lane identity, worktree, occupant, and epoch, reduced from facts.
//!
//! The table is the durable half of a seat. A lane is created once with the
//! capability and worktree it stays bound to; occupants then arrive, end, and
//! are succeeded beneath that identity, each one recorded in the lineage so a
//! successor can always name what it descends from.
//!
//! Exactly one occupant sits in a seat at a time. That is the whole point of a
//! seat, so a start against an occupied lane is refused rather than overwriting
//! the occupant already there — two supervisors believing they own the same
//! lane is the failure this crate exists to prevent.
//!
//! Reduction is total and ordered: the same facts read in the same order always
//! produce the same table, and a fact naming a lane that was never seated is
//! refused instead of quietly creating one.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use journal::crossing::body::{LaneDrained, LaneExited, LaneRenewed, LaneSeated, LaneStarted};
use journal::crossing::{CanonicalUuid, LaneResidency, ResidencyHead, ResidentState};

/// The word a seat's disposition is reported under.
const OCCUPIED: &str = "occupied";
/// The word a seat between occupants is reported under.
const VACANT: &str = "vacant";

/// One lane's durable identity and current occupancy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seat {
    lane: CanonicalUuid,
    capability: CanonicalUuid,
    worktree: String,
    occupant: Option<CanonicalUuid>,
    lineage: Vec<CanonicalUuid>,
}

impl Seat {
    /// The lane this seat is.
    #[must_use]
    pub const fn lane(&self) -> CanonicalUuid {
        self.lane
    }

    /// The capability minted with the lane.
    #[must_use]
    pub const fn capability(&self) -> CanonicalUuid {
        self.capability
    }

    /// The worktree the lane is bound to.
    #[must_use]
    pub fn worktree(&self) -> &str {
        &self.worktree
    }

    /// The occupant sitting in the seat, absent between occupants.
    #[must_use]
    pub const fn occupant(&self) -> Option<CanonicalUuid> {
        self.occupant
    }

    /// Every occupant this seat has held, oldest first.
    #[must_use]
    pub fn lineage(&self) -> &[CanonicalUuid] {
        &self.lineage
    }

    /// Whether an occupant sits in the seat.
    #[must_use]
    pub const fn occupied(&self) -> bool {
        self.occupant.is_some()
    }

    /// This seat as the residency row a head carries.
    #[must_use]
    pub fn residency(&self) -> LaneResidency {
        LaneResidency {
            lane: self.lane,
            occupant: self.occupant,
            lineage: self.lineage.clone(),
            capabilities: vec![self.capability],
            disposition: if self.occupied() { OCCUPIED } else { VACANT }.to_owned(),
        }
    }
}

/// One seating fact, borrowed from the journal that carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seating<'a> {
    /// A lane and its capability were created.
    Seated(&'a LaneSeated),
    /// An occupant became live under the lane.
    Started(&'a LaneStarted),
    /// The occupant ended.
    Exited(&'a LaneExited),
    /// A successor occupant was seated with its ancestor named.
    Renewed(&'a LaneRenewed),
    /// A graceful occupant end completed.
    Drained(&'a LaneDrained),
}

/// Why a fact did not reduce into the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The fact names a lane that was never seated.
    Unseated {
        /// Lane the fact names.
        lane: CanonicalUuid,
    },
    /// The lane was seated already, and identity is created once.
    Reseated {
        /// Lane the fact names.
        lane: CanonicalUuid,
    },
    /// An occupant start belongs to another resident generation.
    Stale {
        /// Epoch this table reduces under.
        epoch: CanonicalUuid,
        /// Epoch the start fact named.
        named: CanonicalUuid,
    },
    /// An occupant already sits in the seat.
    Occupied {
        /// Lane the fact names.
        lane: CanonicalUuid,
        /// Occupant already sitting in the seat.
        sitting: CanonicalUuid,
    },
    /// No occupant sits in the seat the fact ends or succeeds.
    Vacant {
        /// Lane the fact names.
        lane: CanonicalUuid,
    },
    /// The fact names an occupant other than the one sitting in the seat.
    Mismatched {
        /// Lane the fact names.
        lane: CanonicalUuid,
        /// Occupant sitting in the seat.
        sitting: CanonicalUuid,
        /// Occupant the fact named.
        named: CanonicalUuid,
    },
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unseated { lane } => write!(formatter, "lane {lane} was never seated"),
            Self::Reseated { lane } => write!(formatter, "lane {lane} is already seated"),
            Self::Stale { epoch, named } => {
                write!(formatter, "lane table epoch is {epoch}, not {named}")
            }
            Self::Occupied { lane, sitting } => {
                write!(formatter, "lane {lane} is already occupied by {sitting}")
            }
            Self::Vacant { lane } => write!(formatter, "lane {lane} has no occupant"),
            Self::Mismatched {
                lane,
                sitting,
                named,
            } => write!(
                formatter,
                "lane {lane} is occupied by {sitting}, not {named}"
            ),
        }
    }
}

impl Error for Refusal {}

/// Every seat in one resident generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    epoch: CanonicalUuid,
    seats: BTreeMap<CanonicalUuid, Seat>,
}

impl Table {
    /// An empty table for this resident generation.
    #[must_use]
    pub const fn new(epoch: CanonicalUuid) -> Self {
        Self {
            epoch,
            seats: BTreeMap::new(),
        }
    }

    /// The resident generation this table reduces under.
    #[must_use]
    pub const fn epoch(&self) -> CanonicalUuid {
        self.epoch
    }

    /// One lane's seat, if the lane was seated.
    #[must_use]
    pub fn seat(&self, lane: CanonicalUuid) -> Option<&Seat> {
        self.seats.get(&lane)
    }

    /// Every seat, ordered by lane.
    #[must_use]
    pub fn seats(&self) -> Vec<&Seat> {
        self.seats.values().collect()
    }

    /// Reduces one seating fact into the table.
    ///
    /// # Errors
    ///
    /// Returns [`Refusal`] naming the lane when the fact seats a lane twice,
    /// names a lane that was never seated, starts an occupant against an
    /// occupied seat, or ends an occupant the seat is not holding.
    pub fn absorb(&mut self, seating: Seating<'_>) -> Result<(), Refusal> {
        match seating {
            Seating::Seated(fact) => self.seat_lane(fact),
            Seating::Started(fact) => self.start(fact),
            Seating::Renewed(fact) => self.renew(fact),
            Seating::Exited(fact) => self.end(fact.lane, fact.occupant),
            Seating::Drained(fact) => self.end(fact.lane, fact.occupant),
        }
    }

    /// This table as the residency head the journal carries.
    #[must_use]
    pub fn head(&self, state: ResidentState) -> ResidencyHead {
        ResidencyHead {
            epoch: self.epoch,
            state,
            lanes: self.seats.values().map(Seat::residency).collect(),
        }
    }

    /// Creates a lane's durable identity, once.
    fn seat_lane(&mut self, fact: &LaneSeated) -> Result<(), Refusal> {
        if self.seats.contains_key(&fact.lane) {
            return Err(Refusal::Reseated { lane: fact.lane });
        }
        self.seats.insert(
            fact.lane,
            Seat {
                lane: fact.lane,
                capability: fact.capability,
                worktree: fact.worktree.clone(),
                occupant: None,
                lineage: Vec::new(),
            },
        );
        Ok(())
    }

    /// Seats an occupant, which the seat must be empty to admit.
    fn start(&mut self, fact: &LaneStarted) -> Result<(), Refusal> {
        if fact.epoch != self.epoch {
            return Err(Refusal::Stale {
                epoch: self.epoch,
                named: fact.epoch,
            });
        }
        let lane = fact.lane;
        let seat = self
            .seats
            .get_mut(&lane)
            .ok_or(Refusal::Unseated { lane })?;
        if let Some(sitting) = seat.occupant {
            return Err(Refusal::Occupied { lane, sitting });
        }
        seat.occupant = Some(fact.occupant);
        seat.lineage.push(fact.occupant);
        Ok(())
    }

    /// Succeeds one occupant with another, naming the ancestor it descends
    /// from, which must be the occupant the seat is actually holding.
    fn renew(&mut self, fact: &LaneRenewed) -> Result<(), Refusal> {
        let lane = fact.lane;
        let seat = self
            .seats
            .get_mut(&lane)
            .ok_or(Refusal::Unseated { lane })?;
        let sitting = seat.occupant.ok_or(Refusal::Vacant { lane })?;
        if sitting != fact.ancestor {
            return Err(Refusal::Mismatched {
                lane,
                sitting,
                named: fact.ancestor,
            });
        }
        if sitting == fact.occupant {
            return Err(Refusal::Occupied { lane, sitting });
        }
        seat.occupant = Some(fact.occupant);
        seat.lineage.push(fact.occupant);
        Ok(())
    }

    /// Vacates the seat, which must be holding the occupant the fact names.
    fn end(&mut self, lane: CanonicalUuid, occupant: CanonicalUuid) -> Result<(), Refusal> {
        let seat = self
            .seats
            .get_mut(&lane)
            .ok_or(Refusal::Unseated { lane })?;
        let sitting = seat.occupant.ok_or(Refusal::Vacant { lane })?;
        if sitting != occupant {
            return Err(Refusal::Mismatched {
                lane,
                sitting,
                named: occupant,
            });
        }
        seat.occupant = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Refusal, Seating, Table};
    use journal::crossing::body::{LaneDrained, LaneExited, LaneRenewed, LaneSeated, LaneStarted};
    use journal::crossing::{CanonicalUuid, ResidentState};

    fn uuid(last: u8) -> CanonicalUuid {
        format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02}")
            .parse()
            .expect("fixture uuid")
    }

    fn seated() -> LaneSeated {
        LaneSeated {
            lane: uuid(1),
            capability: uuid(2),
            worktree: "/worktrees/lane-a".to_owned(),
        }
    }

    fn table() -> Table {
        let mut table = Table::new(uuid(0));
        table
            .absorb(Seating::Seated(&seated()))
            .expect("a lane seats once");
        table
    }

    fn started(occupant: CanonicalUuid) -> LaneStarted {
        LaneStarted {
            lane: uuid(1),
            occupant,
            epoch: uuid(0),
        }
    }

    /// Identity, worktree, and capability survive the occupant that sat in them.
    #[test]
    fn a_seat_keeps_its_identity_across_occupants() {
        let mut table = table();
        table
            .absorb(Seating::Started(&started(uuid(3))))
            .expect("an empty seat admits an occupant");
        table
            .absorb(Seating::Exited(&LaneExited {
                lane: uuid(1),
                occupant: uuid(3),
                code: 0,
            }))
            .expect("the sitting occupant ends");

        let seat = table.seat(uuid(1)).expect("the lane was seated");
        assert_eq!(seat.worktree(), "/worktrees/lane-a");
        assert_eq!(seat.capability(), uuid(2));
        assert_eq!(seat.occupant(), None);
        assert!(!seat.occupied());
        assert_eq!(seat.lineage(), [uuid(3)]);
    }

    /// One occupant per seat: a second start is refused and changes nothing.
    #[test]
    fn a_seat_holds_exactly_one_occupant() {
        let mut table = table();
        table
            .absorb(Seating::Started(&started(uuid(3))))
            .expect("an empty seat admits an occupant");

        let refusal = table
            .absorb(Seating::Started(&started(uuid(4))))
            .expect_err("an occupied seat admits no second occupant");

        assert_eq!(
            refusal,
            Refusal::Occupied {
                lane: uuid(1),
                sitting: uuid(3),
            }
        );
        assert_eq!(
            refusal.to_string(),
            format!("lane {} is already occupied by {}", uuid(1), uuid(3))
        );
        assert_eq!(
            table.seat(uuid(1)).expect("seated").occupant(),
            Some(uuid(3))
        );
    }

    /// An old resident cannot seat an occupant in the current generation.
    #[test]
    fn a_stale_epoch_cannot_start_an_occupant() {
        let mut table = table();
        let stale = LaneStarted {
            lane: uuid(1),
            occupant: uuid(3),
            epoch: uuid(9),
        };

        assert_eq!(
            table.absorb(Seating::Started(&stale)),
            Err(Refusal::Stale {
                epoch: uuid(0),
                named: uuid(9),
            })
        );
        let seat = table.seat(uuid(1)).expect("the lane remains seated");
        assert_eq!(seat.occupant(), None);
        assert!(seat.lineage().is_empty());
    }

    /// A successor names the occupant it descends from, and the lineage grows.
    #[test]
    fn renewal_succeeds_the_sitting_occupant_and_records_the_lineage() {
        let mut table = table();
        table
            .absorb(Seating::Started(&started(uuid(3))))
            .expect("an empty seat admits an occupant");
        table
            .absorb(Seating::Renewed(&LaneRenewed {
                lane: uuid(1),
                occupant: uuid(4),
                ancestor: uuid(3),
            }))
            .expect("the successor names the sitting occupant");

        let seat = table.seat(uuid(1)).expect("seated");
        assert_eq!(seat.occupant(), Some(uuid(4)));
        assert_eq!(seat.lineage(), [uuid(3), uuid(4)]);
    }

    /// A successor naming an ancestor the seat is not holding is refused.
    #[test]
    fn renewal_refuses_an_ancestor_the_seat_is_not_holding() {
        let mut table = table();
        table
            .absorb(Seating::Started(&started(uuid(3))))
            .expect("an empty seat admits an occupant");

        let refusal = table
            .absorb(Seating::Renewed(&LaneRenewed {
                lane: uuid(1),
                occupant: uuid(5),
                ancestor: uuid(4),
            }))
            .expect_err("the named ancestor is not sitting");

        assert_eq!(
            refusal,
            Refusal::Mismatched {
                lane: uuid(1),
                sitting: uuid(3),
                named: uuid(4),
            }
        );
        assert_eq!(
            table.seat(uuid(1)).expect("seated").lineage(),
            [uuid(3)],
            "a refused renewal still grew the lineage",
        );
    }

    /// Renewal introduces a successor rather than repeating the same occupant.
    #[test]
    fn renewal_refuses_the_sitting_occupant_as_its_own_successor() {
        let mut table = table();
        table
            .absorb(Seating::Started(&started(uuid(3))))
            .expect("an empty seat admits an occupant");

        assert_eq!(
            table.absorb(Seating::Renewed(&LaneRenewed {
                lane: uuid(1),
                occupant: uuid(3),
                ancestor: uuid(3),
            })),
            Err(Refusal::Occupied {
                lane: uuid(1),
                sitting: uuid(3),
            })
        );
        assert_eq!(table.seat(uuid(1)).expect("seated").lineage(), [uuid(3)]);
    }

    /// Ending an occupant the seat is not holding is refused by name.
    #[test]
    fn ending_refuses_an_occupant_the_seat_is_not_holding() {
        let mut table = table();
        table
            .absorb(Seating::Started(&started(uuid(3))))
            .expect("an empty seat admits an occupant");

        assert_eq!(
            table.absorb(Seating::Drained(&LaneDrained {
                lane: uuid(1),
                occupant: uuid(9),
            })),
            Err(Refusal::Mismatched {
                lane: uuid(1),
                sitting: uuid(3),
                named: uuid(9),
            })
        );
        assert_eq!(
            table.seat(uuid(1)).expect("seated").occupant(),
            Some(uuid(3))
        );
    }

    /// Ending an already-empty seat is refused rather than doing nothing.
    #[test]
    fn ending_an_empty_seat_is_refused() {
        let mut table = table();

        assert_eq!(
            table.absorb(Seating::Exited(&LaneExited {
                lane: uuid(1),
                occupant: uuid(3),
                code: 0,
            })),
            Err(Refusal::Vacant { lane: uuid(1) })
        );
    }

    /// A fact naming a lane that was never seated does not create one.
    #[test]
    fn a_fact_for_an_unseated_lane_is_refused() {
        let mut table = Table::new(uuid(0));

        assert_eq!(
            table.absorb(Seating::Started(&started(uuid(3)))),
            Err(Refusal::Unseated { lane: uuid(1) })
        );
        assert!(table.seats().is_empty());
    }

    /// Identity is created once, so seating a lane twice is refused.
    #[test]
    fn seating_a_lane_twice_is_refused() {
        let mut table = table();

        assert_eq!(
            table.absorb(Seating::Seated(&seated())),
            Err(Refusal::Reseated { lane: uuid(1) })
        );
    }

    /// The table renders the residency head the journal carries.
    #[test]
    fn the_table_renders_the_residency_head() {
        let mut table = table();
        table
            .absorb(Seating::Started(&started(uuid(3))))
            .expect("an empty seat admits an occupant");

        let head = table.head(ResidentState::Seated);

        assert_eq!(head.epoch, uuid(0));
        assert_eq!(head.state, ResidentState::Seated);
        assert_eq!(head.lanes.len(), 1);
        let row = &head.lanes[0];
        assert_eq!(row.lane, uuid(1));
        assert_eq!(row.occupant, Some(uuid(3)));
        assert_eq!(row.lineage, vec![uuid(3)]);
        assert_eq!(row.capabilities, vec![uuid(2)]);
        assert_eq!(row.disposition, "occupied");
    }

    /// A seat between occupants reports itself vacant, not occupied by nobody.
    #[test]
    fn a_vacant_seat_is_reported_as_vacant() {
        let table = table();

        let head = table.head(ResidentState::Quiescing);

        assert_eq!(head.lanes[0].disposition, "vacant");
        assert_eq!(head.lanes[0].occupant, None);
        assert_eq!(head.state, ResidentState::Quiescing);
    }
}
