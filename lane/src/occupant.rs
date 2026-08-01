//! One supervised process group per occupied seat.
//!
//! An occupant is a process group, not a process: the seat owns everything the
//! agent spawns beneath it, so ending an occupant ends its children too. The
//! group is launched through the sill door rather than by this module, because
//! the door is where argv, environment, deadlines, and redaction are already
//! decided; a second place deciding them would be a second policy.
//!
//! The law here is arithmetic. A seat holds one occupant or none, and seating
//! is refused *before* the door is opened, so a lane that is already occupied
//! cannot leave a stray process group behind as the cost of finding out.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::io;
use std::time::Duration;

use journal::crossing::CanonicalUuid;
use sill::{ProcessGroup, Running, Spec, WaitError, WaitOutcome, WaitStep, spawn};

/// One process group supervised under a lane.
pub struct Occupant {
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    running: Running,
}

impl Occupant {
    /// The lane this occupant sits in.
    #[must_use]
    pub const fn lane(&self) -> CanonicalUuid {
        self.lane
    }

    /// The occupant's identity, which outlives the process it names.
    #[must_use]
    pub const fn occupant(&self) -> CanonicalUuid {
        self.occupant
    }

    /// The process group the door opened.
    #[must_use]
    pub fn group(&self) -> u32 {
        self.running.id()
    }

    /// The spec the door opened this group under.
    #[must_use]
    pub fn spec(&self) -> &Spec {
        self.running.spec()
    }

    /// Remaining configured runtime available for graceful drain.
    #[must_use]
    pub fn drain_remaining(&self) -> Duration {
        self.running.runtime_remaining()
    }

    /// Configured grace after a termination signal.
    #[must_use]
    pub fn termination_grace(&self) -> Duration {
        self.running.spec().grace
    }

    /// Configured reap bound after forced termination.
    #[must_use]
    pub fn reap_bound(&self) -> Duration {
        self.running.spec().deadlines.reap()
    }

    /// Passively waits for this group to exit within `duration`.
    ///
    /// # Errors
    ///
    /// Returns the SILL wait failure with this occupant's owner evidence.
    pub fn wait_for(self, duration: Duration) -> Result<OccupantWait, WaitError> {
        let Self {
            lane,
            occupant,
            running,
        } = self;
        match running.wait_for(duration)? {
            WaitStep::Exited(outcome) => Ok(OccupantWait::Exited {
                lane,
                occupant,
                outcome,
            }),
            WaitStep::Running(running) => Ok(OccupantWait::Running(Box::new(Self {
                lane,
                occupant,
                running: *running,
            }))),
        }
    }

    /// Requests graceful termination of the whole process group.
    ///
    /// # Errors
    ///
    /// Returns the SILL signalling failure with this occupant's owner evidence.
    pub fn terminate(&self) -> Result<(), WaitError> {
        self.running.signal_terminate()
    }

    /// Forces termination of the whole process group.
    ///
    /// # Errors
    ///
    /// Returns the SILL signalling failure with this occupant's owner evidence.
    pub fn kill(&self) -> Result<(), WaitError> {
        self.running.signal_kill()
    }

    /// Waits for the process group to end.
    ///
    /// # Errors
    ///
    /// Returns [`WaitError`] as the sill door reports it.
    pub fn wait(self) -> Result<WaitOutcome, WaitError> {
        self.running.wait()
    }
}

/// Result of passively observing one occupied seat for a bounded interval.
#[derive(Debug)]
pub enum OccupantWait {
    /// The group exited and was reaped within the interval.
    Exited {
        /// Lane whose group ended.
        lane: CanonicalUuid,
        /// Occupant whose group ended.
        occupant: CanonicalUuid,
        /// Complete bounded process evidence.
        outcome: WaitOutcome,
    },
    /// The interval elapsed and the seat remains occupied.
    Running(Box<Occupant>),
}

impl fmt::Debug for Occupant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Occupant")
            .field("lane", &self.lane)
            .field("occupant", &self.occupant)
            .field("group", &self.running.id())
            .finish()
    }
}

/// Why a lane did not take an occupant.
#[derive(Debug)]
pub enum SeatError {
    /// A process group already sits in the seat.
    Occupied {
        /// Lane that is already occupied.
        lane: CanonicalUuid,
        /// Occupant already sitting in it.
        sitting: CanonicalUuid,
    },
    /// The spec would place the occupant in the caller's process group.
    InheritedGroup {
        /// Lane that requires its own process group.
        lane: CanonicalUuid,
    },
    /// The sill door refused to open.
    Door(io::Error),
}

impl fmt::Display for SeatError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Occupied { lane, sitting } => write!(
                formatter,
                "lane {lane} is already occupied by {sitting}; a seat holds one process group"
            ),
            Self::InheritedGroup { lane } => {
                write!(formatter, "lane {lane} requires a new process group")
            }
            Self::Door(error) => write!(formatter, "the sill door refused to open: {error}"),
        }
    }
}

impl Error for SeatError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Occupied { .. } => None,
            Self::InheritedGroup { .. } => None,
            Self::Door(error) => Some(error),
        }
    }
}

/// Every occupied seat the supervisor holds.
#[derive(Debug, Default)]
pub struct Seats {
    occupied: BTreeMap<CanonicalUuid, Occupant>,
}

impl Seats {
    /// A supervisor holding no seats.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            occupied: BTreeMap::new(),
        }
    }

    /// The occupant sitting in a lane, if one does.
    #[must_use]
    pub fn occupant(&self, lane: CanonicalUuid) -> Option<&Occupant> {
        self.occupied.get(&lane)
    }

    /// Whether a process group sits in this lane.
    #[must_use]
    pub fn occupied(&self, lane: CanonicalUuid) -> bool {
        self.occupied.contains_key(&lane)
    }

    /// Every occupied lane, ordered by lane.
    #[must_use]
    pub fn lanes(&self) -> Vec<CanonicalUuid> {
        self.occupied.keys().copied().collect()
    }

    /// How many seats are occupied.
    #[must_use]
    pub fn len(&self) -> usize {
        self.occupied.len()
    }

    /// Whether no seat is occupied.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.occupied.is_empty()
    }

    /// Opens the door for a lane and supervises the group it returns.
    ///
    /// Occupancy is decided before the door opens, so a refused seating spawns
    /// nothing at all.
    ///
    /// # Errors
    ///
    /// Returns [`SeatError::Occupied`] when a group already sits in the lane,
    /// or [`SeatError::Door`] when the sill door refuses to open.
    pub fn seat(
        &mut self,
        lane: CanonicalUuid,
        occupant: CanonicalUuid,
        spec: Spec,
    ) -> Result<&Occupant, SeatError> {
        if let Some(sitting) = self.occupied.get(&lane) {
            return Err(SeatError::Occupied {
                lane,
                sitting: sitting.occupant,
            });
        }
        if spec.group != ProcessGroup::New {
            return Err(SeatError::InheritedGroup { lane });
        }
        let running = spawn(spec).map_err(SeatError::Door)?;
        Ok(self.occupied.entry(lane).or_insert(Occupant {
            lane,
            occupant,
            running,
        }))
    }

    /// Releases a seat, handing back the group so it can be waited on.
    pub fn vacate(&mut self, lane: CanonicalUuid) -> Option<Occupant> {
        self.occupied.remove(&lane)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use super::{SeatError, Seats};
    use journal::crossing::CanonicalUuid;
    use sill::{
        Argv, Cancellation, Deadlines, Environment, Output, ProcessGroup, Redaction,
        RestartDisposition, Spec, StdinPolicy, StreamPolicy, WaitPolicy,
    };

    fn uuid(last: u8) -> CanonicalUuid {
        format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02}")
            .parse()
            .expect("fixture uuid")
    }

    /// A spec the door will open: a process that ends on its own at once.
    fn spec() -> Spec {
        Spec {
            argv: Argv::new("/bin/sleep", ["0"]).expect("argv"),
            cwd: PathBuf::from("/"),
            environment: Environment::new([("PATH", "/usr/bin:/bin")]).expect("environment"),
            group: ProcessGroup::New,
            deadlines: Deadlines::new(
                Duration::from_secs(10),
                Duration::from_secs(1),
                Duration::from_secs(1),
            )
            .expect("deadlines"),
            wait: WaitPolicy::new("lane", Cancellation::Never, RestartDisposition::Never)
                .expect("wait policy"),
            output: Output::new(StdinPolicy::Null, StreamPolicy::Null, StreamPolicy::Null),
            redaction: Redaction::None,
            grace: Duration::from_secs(1),
        }
    }

    /// An empty seat takes one group, and the seat names what sits in it.
    #[test]
    fn an_empty_seat_takes_one_supervised_group() {
        let mut seats = Seats::new();

        let occupant = seats
            .seat(uuid(1), uuid(3), spec())
            .expect("the door opens");
        assert_eq!(occupant.lane(), uuid(1));
        assert_eq!(occupant.occupant(), uuid(3));

        assert!(seats.occupied(uuid(1)));
        assert_eq!(seats.len(), 1);
        assert_eq!(seats.lanes(), vec![uuid(1)]);

        seats
            .vacate(uuid(1))
            .expect("the seat was occupied")
            .wait()
            .expect("the group ends");
        assert!(seats.is_empty());
    }

    /// A second seating is refused, and refused without opening the door.
    ///
    /// The unopenable spec is what proves the order. Occupancy is decided
    /// first, so the refusal names the seat; were the door opened first, this
    /// same call would fail as [`SeatError::Door`] instead, and a spec the door
    /// *would* have accepted would have left a stray process group behind.
    #[test]
    fn an_occupied_seat_refuses_a_second_group_without_opening_the_door() {
        let mut seats = Seats::new();
        let first = seats
            .seat(uuid(1), uuid(3), spec())
            .expect("the door opens")
            .group();

        let mut unopenable = spec();
        unopenable.argv = Argv::new("/nonexistent/occupant", [] as [&str; 0]).expect("argv");
        let error = seats
            .seat(uuid(1), uuid(4), unopenable)
            .expect_err("an occupied seat takes no second group");

        assert!(
            matches!(
                error,
                SeatError::Occupied {
                    lane,
                    sitting,
                } if lane == uuid(1) && sitting == uuid(3)
            ),
            "unexpected refusal: {error}",
        );
        assert_eq!(
            seats.occupant(uuid(1)).expect("still occupied").group(),
            first,
            "the refused seating replaced the supervised group",
        );
        assert_eq!(seats.len(), 1);

        seats
            .vacate(uuid(1))
            .expect("the seat was occupied")
            .wait()
            .expect("the group ends");
    }

    /// Every occupant owns a new group; an inherited group never opens.
    #[test]
    fn an_inherited_process_group_is_refused_before_the_door_opens() {
        let mut seats = Seats::new();
        let mut inherited = spec();
        inherited.group = ProcessGroup::Inherit;
        inherited.argv = Argv::new("/nonexistent/occupant", [] as [&str; 0]).expect("argv");

        let error = seats
            .seat(uuid(1), uuid(3), inherited)
            .expect_err("an occupant requires its own process group");

        assert!(matches!(
            error,
            SeatError::InheritedGroup { lane } if lane == uuid(1)
        ));
        assert!(seats.is_empty());
    }

    /// Seats are per lane, so a second lane takes its own group.
    #[test]
    fn each_lane_supervises_its_own_group() {
        let mut seats = Seats::new();
        seats
            .seat(uuid(1), uuid(3), spec())
            .expect("the door opens");
        seats
            .seat(uuid(2), uuid(4), spec())
            .expect("the door opens");

        assert_eq!(seats.lanes(), vec![uuid(1), uuid(2)]);
        assert_ne!(
            seats.occupant(uuid(1)).expect("occupied").group(),
            seats.occupant(uuid(2)).expect("occupied").group(),
            "two lanes shared one process group",
        );

        for lane in [uuid(1), uuid(2)] {
            seats
                .vacate(lane)
                .expect("the seat was occupied")
                .wait()
                .expect("the group ends");
        }
    }

    /// Vacating a seat that holds nothing hands back nothing.
    #[test]
    fn vacating_an_empty_seat_hands_back_nothing() {
        let mut seats = Seats::new();

        assert!(seats.vacate(uuid(1)).is_none());
        assert!(!seats.occupied(uuid(1)));
    }
}
