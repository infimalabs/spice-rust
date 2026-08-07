//! Playback ownership independent of browser media APIs.

/// The one generation ledger that decides which speech request owns playback.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct PlaybackOwnership {
    generation: u64,
}

impl PlaybackOwnership {
    /// Invalidate the prior owner and return the replacement generation.
    pub fn begin(&mut self) -> u64 {
        self.stop();
        self.generation
    }

    /// Whether asynchronous work still belongs to the current owner.
    #[must_use]
    pub const fn is_current(&self, generation: u64) -> bool {
        self.generation == generation
    }

    /// Invalidate the current owner without selecting a replacement.
    pub fn stop(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }
}
