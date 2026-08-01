//! Recognizing the retirement of a directive.
//!
//! ack is a module of steer rather than a crate of its own: every fact it
//! recognizes wears steer's prefix, and steer is its one consumer. Splitting it
//! out would put a crate boundary through the middle of one vocabulary.
//!
//! It has two halves. [`keys`] fixes the grammar — what a key is, and which
//! dispositions exist — and [`scan`] reads outcomes out of assistant prose
//! against that grammar without touching disk.

pub mod keys;
pub mod scan;
