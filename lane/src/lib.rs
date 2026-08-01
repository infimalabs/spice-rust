//! The supervised seat: one agent occupant per lane.
//!
//! A lane is a seat, not a process. The seat outlives the occupant sitting in
//! it, which is why identity, worktree, lineage, and epoch are durable state
//! while the occupant is a process group that comes and goes beneath them.
//! Every module here reduces facts into that durable state or guards a
//! transition of it; none of them decide when to run, because a supervisor that
//! reads a clock cannot be replayed.
//!
//! The crate is the widest row in the lattice, sitting above journal, bus,
//! vendor, tape, steer, consent, listen, fold, taste, atomic, and sill. It
//! reaches the plane-crossing vocabulary through [`journal::crossing`] rather
//! than depending on wire directly, so the one vocabulary stays the one every
//! row reads.
//!
//! Time arrives as an argument. [`hold`] compares deadlines it was handed and
//! [`renew`] is told whether an occupant is alive; neither consults a clock, so
//! the same facts read in the same order always reduce to the same seat.

pub mod config;
pub mod drain;
pub mod hold;
pub mod life;
pub mod occupant;
pub mod renew;
pub mod table;

pub use config::Config;
