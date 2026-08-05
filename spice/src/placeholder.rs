//! The sentences that mean "registered, but the work is not here yet".
//!
//! Reasons are inline literals nearly everywhere in this crate, and that is
//! right: a reason is read where it is emitted, by someone already looking at
//! the branch that emits it. These four are the exception because they are read
//! from a distance, and read as absences. `spice/tests/landing.rs` walks every
//! registered leaf and asserts no operator ever sees one; several process
//! fixtures assert the same of one command's output. None of them stands
//! anywhere near the branches that produce these sentences.
//!
//! An absence asserted against a restated copy is the fixture that cannot fail.
//! Rename an emitted reason and every such fixture goes on searching for a
//! sentence the binary no longer emits, finds it nowhere, and passes -- for the
//! wrong reason, with the guard gone and nothing reporting its absence. So the
//! sentence is stated once here, the emitters read it, and every fixture that
//! searches for one reads the same inventory.

/// One reason a registered leaf reports instead of the work it names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlaceholderReason {
    /// A current public task owns this leaf's visible terminal refusal.
    NotLanded,
    /// A leaf registered as landed reached the terminal release branch.
    DispatchGap,
    /// A registered leaf states no landing disposition at all.
    LandingUnaccounted,
    /// The resident holds no handler for a registered command kind.
    HandlerNotLanded,
}

impl PlaceholderReason {
    /// Every placeholder reason, in declaration order.
    ///
    /// The length is stated rather than inferred, so a reason dropped from this
    /// inventory fails to compile instead of quietly shrinking the set the
    /// fixture searches operator output for.
    pub const ALL: [Self; 4] = [
        Self::NotLanded,
        Self::DispatchGap,
        Self::LandingUnaccounted,
        Self::HandlerNotLanded,
    ];

    /// The exact sentence this reason is emitted as.
    ///
    /// The match is exhaustive, so a reason added without a stated sentence
    /// fails to compile: the text is declared beside the variant rather than
    /// derived from its name, because the two are allowed to differ and the
    /// operator only ever sees the text.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotLanded => "release::not-landed",
            Self::DispatchGap => "release::dispatch-gap",
            Self::LandingUnaccounted => "release::landing-unaccounted",
            Self::HandlerNotLanded => "command::handler-not-landed",
        }
    }

    /// The owned spelling a `Refusal` or `Fault` carries.
    #[must_use]
    pub fn to_reason(self) -> String {
        self.as_str().to_owned()
    }
}
