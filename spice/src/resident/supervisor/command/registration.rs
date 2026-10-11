//! The first turn, held until omp answers the `spice` scheme registration:
//! only a seat that serves the scheme is told about it and handed URLs to
//! read.

use std::io::{self, Write};
use std::sync::{Mutex, OnceLock, PoisonError};

use listen::Decoded;

use super::Driver;

#[cfg(test)]
mod tests;

/// Whether omp serves `spice://`, and the first turn awaiting that answer.
#[derive(Default)]
pub(super) struct Registration {
    /// Settled by omp's answer; until then nothing has named a read.
    serves: OnceLock<bool>,
    first_prompt: Mutex<Option<String>>,
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
        first_prompt: Option<String>,
    ) -> io::Result<()> {
        *self
            .registration
            .first_prompt
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = first_prompt;
        self.write(opening)
    }

    /// Settle whether this seat serves `spice://` from omp's answer to the
    /// registration, then start the first turn as that answer decides.
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
        let Some(prompt) = self
            .registration
            .first_prompt
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        else {
            return;
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
