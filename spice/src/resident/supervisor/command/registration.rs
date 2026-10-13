//! The first turn, held until omp answers the `spice` scheme registration:
//! only a seat that serves the scheme is told about it and handed URLs to
//! read. A resumed seat's notice is read from current authority only then.

use std::io::{self, Write};
use std::sync::{Mutex, OnceLock, PoisonError};

use listen::Decoded;

use super::Driver;

#[cfg(test)]
mod tests;

/// The first turn a launch composed, and whether its seat resumes a session.
pub(in crate::resident) enum FirstTurn {
    /// A fresh session's turn, already carrying its startup briefing.
    Fresh(String),
    /// A resumed session's neutral turn. The resume notice naming the held
    /// claim and pending steering joins it only as it is delivered, so no
    /// launch, however early or abandoned, freezes that authority.
    Resumed(String),
}

/// Whether omp serves `spice://`, and the first turn awaiting that answer.
#[derive(Default)]
pub(super) struct Registration {
    /// Settled by omp's answer; until then nothing has named a read.
    serves: OnceLock<bool>,
    first_turn: Mutex<Option<FirstTurn>>,
}

impl<W: Write + Send + 'static> Driver<W> {
    /// Write the opening request the vendor contract composed and hold the
    /// first turn for the registration answer. omp answers every command, an
    /// unknown one with a refusal, in the order the opening sent them, so that
    /// answer always precedes the session's.
    ///
    /// # Errors
    ///
    /// Returns the pipe's error, or `BrokenPipe` once the stream is closed.
    pub(in crate::resident) fn open(
        &self,
        opening: &[u8],
        first_turn: Option<FirstTurn>,
    ) -> io::Result<()> {
        *self
            .registration
            .first_turn
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = first_turn;
        self.write(opening)
    }

    /// Settle whether this seat serves `spice://` from omp's answer to the
    /// registration, then start the first turn as that answer decides. The
    /// first answer alone takes the turn, so a resumed seat's authority is
    /// read once, at this delivery.
    pub(super) fn observe_uri_registration(&self, decoded: &Decoded) {
        let Some(response) = decoded
            .context()
            .and_then(|context| context.response.as_ref())
        else {
            return;
        };
        if response.id.as_deref() != Some(listen::OMP_URI_SCHEMES_REQUEST_ID)
            || self.registration.serves.set(response.success).is_err()
        {
            return;
        }
        let Some(turn) = self
            .registration
            .first_turn
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        else {
            return;
        };
        let prompt = match turn {
            FirstTurn::Fresh(prompt) => prompt,
            FirstTurn::Resumed(mut prompt) => {
                prompt.push_str("\n\n");
                prompt.push_str(&(self.read_resume)());
                prompt
            }
        };
        if let Err(error) = self.write(&vendor::omp::first_prompt_request(&self.for_seat(prompt))) {
            eprintln!("spice supervisor: first turn unwritten: {error}");
        }
    }

    /// `text` as this seat should read it: with `spice://` URLs and the scheme
    /// guide when omp serves the scheme, else exactly as composed.
    pub(super) fn for_seat(&self, text: String) -> String {
        if self.registration.serves.get() == Some(&true) {
            crate::resource::for_served_seat(&text)
        } else {
            text
        }
    }
}
