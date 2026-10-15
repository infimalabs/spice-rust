//! The first turn, held until omp answers the `spice` scheme registration and
//! the resident has seated this occupant, whichever is later. Only a seat that
//! serves the scheme is told about it and handed URLs to read, and only a
//! seated occupant's commands pass job admission, so a turn written before
//! the start commits would open with refusals. A resumed seat's notice is
//! read from current authority only at that release. Background notices, kept
//! jobs' events and endings, and steering wait behind that turn: omp runs
//! whatever reaches an idle session first as a turn of its own, so anything
//! written ahead of the first prompt would leave the prompt's authority stale
//! by its delivery.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

use listen::Decoded;

use super::{Driver, Job, Report};

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

/// Whether omp serves `spice://`, and the first turn awaiting that answer and
/// the occupant's seating.
#[derive(Default)]
pub(super) struct Registration {
    /// Settled by omp's answer; until then nothing has named a read.
    serves: OnceLock<bool>,
    first_turn: Mutex<Option<FirstTurn>>,
    /// Whether a first turn is still unwritten. Read and cleared only under
    /// the driver's background lock, so a notice is either held for the
    /// first prompt's flush or written after it, never neither.
    holding: AtomicBool,
    /// Kept jobs whose telling waits for the first prompt, in the order they
    /// first asked. Nothing about their telling is claimed meanwhile, so a
    /// seat that closes first leaves them owed to its successor.
    jobs: Mutex<Vec<(Job, Owed)>>,
}

/// What a held job still has to say once the first prompt is written.
enum Owed {
    /// Its unheard events.
    Events,
    /// Its unheard events, then the ending it was followed to.
    Ending(Option<Report>),
}

impl Registration {
    /// Whether background notices must wait for the first prompt.
    pub(super) fn holds_first_turn(&self) -> bool {
        self.holding.load(Ordering::Acquire)
    }
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
        {
            let _background = self
                .background
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            self.registration
                .holding
                .store(first_turn.is_some(), Ordering::Release);
            *self
                .registration
                .first_turn
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = first_turn;
        }
        self.write(opening)
    }

    /// Settle whether this seat serves `spice://` from omp's answer to the
    /// registration, then release the first turn if the occupant is already
    /// seated. The first answer alone settles it.
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
        self.release_first_turn();
    }

    /// Start the held first turn once omp has answered the registration and
    /// the journal seats this occupant; returns whether this call started it.
    /// Only one call takes the turn, so a resumed seat's authority is read
    /// once, at this delivery. Background notices held meanwhile follow the
    /// prompt, then held jobs' events and endings, then the inbox as it stands
    /// now, so omp starts the prompt's turn before any of theirs and the
    /// direction it counts arrives after it.
    pub(super) fn release_first_turn(&self) -> bool {
        if self.registration.serves.get().is_none() {
            return false;
        }
        let turn = {
            let mut held = self
                .registration
                .first_turn
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if held.is_none() || !(self.read_seated)() {
                return false;
            }
            held.take()
        };
        let Some(turn) = turn else {
            return false;
        };
        let prompt = match turn {
            FirstTurn::Fresh(prompt) => prompt,
            FirstTurn::Resumed(mut prompt) => {
                prompt.push_str("\n\n");
                prompt.push_str(&(self.read_resume)());
                prompt
            }
        };
        let prompt = vendor::omp::first_prompt_request(&self.for_seat(prompt));
        let queued = self
            .background
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Err(error) = self.write(&prompt) {
            eprintln!("spice supervisor: first turn unwritten: {error}");
        }
        self.registration.holding.store(false, Ordering::Release);
        for (purpose, message) in queued.iter() {
            let sequence = self.sequence.fetch_add(1, Ordering::Relaxed) + 1;
            if let Err(error) = self.write(&vendor::omp::aside_request(sequence, purpose, message))
            {
                eprintln!("spice supervisor: held notice {purpose} unwritten: {error}");
            }
        }
        drop(queued);
        let held = std::mem::take(
            &mut *self
                .registration
                .jobs
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        for (job, owed) in held {
            self.deliver_events(&job);
            if let Owed::Ending(report) = owed {
                self.deliver(&job, report.as_ref());
            }
        }
        self.interject(super::steering::InboxDelivery::Update);
        true
    }

    /// Hold one kept job's telling while the first turn is unwritten, and say
    /// whether it was held. An ending replaces an earlier events-only hold,
    /// since a delivered ending follows the job's events.
    pub(super) fn hold_job(&self, job: &Job, ending: Option<Option<&Report>>) -> bool {
        let _background = self
            .background
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !self.registration.holds_first_turn() {
            return false;
        }
        let owed = ending.map_or(Owed::Events, |report| Owed::Ending(report.cloned()));
        let mut held = self
            .registration
            .jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match held.iter_mut().find(|(held, _)| held.id() == job.id()) {
            Some((_, slot)) => {
                if matches!(owed, Owed::Ending(_)) {
                    *slot = owed;
                }
            }
            None => held.push((job.clone(), owed)),
        }
        true
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
