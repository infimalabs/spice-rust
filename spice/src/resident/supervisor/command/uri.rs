//! The `spice://` scheme a lane's omp reads through: each read answers with
//! the [`crate::resource`] the URL names, read over the lane's own connection.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, PoisonError};

use wire::CanonicalUuid;

use super::Driver;

/// Answers one `spice://` URL with what a read of it returns, or the refusal.
pub(super) type UriReader = Box<dyn Fn(&str) -> Result<String, String> + Send + Sync>;

/// The session state a read answers under: the identity and home the
/// occupant's terminal carries, as its own command would see them, else the
/// resident's.
pub(super) fn occupant_environment(
    terminal: Option<&super::Terminal>,
    resident: &crate::session_command::Environment,
) -> crate::session_command::Environment {
    terminal.map_or_else(|| resident.clone(), super::Terminal::session_environment)
}

/// Read `spice://` URLs as `lane`, whose checkout is `worktree` and whose
/// occupant's session state is `environment`, resolving quote targets and
/// CLI reads through the resident.
pub(super) fn resident_reader(
    discovery: PathBuf,
    lane: CanonicalUuid,
    worktree: PathBuf,
    environment: crate::session_command::Environment,
) -> impl Fn(&str) -> Result<String, String> + Send + Sync + 'static {
    let seat = crate::resource::Seat {
        worktree,
        lane,
        environment,
    };
    move |url| {
        seat.read(url, |target| {
            match crate::command_client::read_lane_quote(&discovery, lane, target) {
                Ok(resolution) => spice::quote::render(&resolution),
                Err(wire::Outcome::Refusal(refusal)) => Err(format!(
                    "spice refused the quote read: {}",
                    refusal.reason()
                )),
                Err(wire::Outcome::Fault(fault)) => Err(format!(
                    "spice could not read the quote: {}: {}",
                    fault.reason, fault.detail
                )),
                Err(wire::Outcome::Reply(_)) => {
                    Err("spice answered the quote read with no resolution".to_owned())
                }
            }
        })
    }
}

impl<W: Write + Send + 'static> Driver<W> {
    /// Answer one host URI request: a read, answered on its own thread so the
    /// stream keeps being read.
    ///
    /// Reading commits nothing, so omp may read as often as its tools ask. A
    /// write is refused at once: the scheme only reads.
    pub(super) fn serve_uri(self: &Arc<Self>, id: &str, operation: &str, url: &str) {
        if operation != "read" {
            let _refused = self.write(&vendor::omp::host_uri_refused(id, vendor::omp::URI_SERVED));
            return;
        }
        self.uris
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.to_owned());
        let driver = Arc::clone(self);
        let (answered, url) = (id.to_owned(), url.to_owned());
        let started = std::thread::Builder::new()
            .name("spice-uri".to_owned())
            .spawn(move || driver.answer_uri(&answered, &url));
        if let Err(error) = started {
            self.uris
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(id);
            let _refused = self.write(&vendor::omp::host_uri_refused(
                id,
                &format!("read thread failed: {error}"),
            ));
        }
    }

    /// Answer one read unless omp withdrew it meanwhile. The pending set
    /// stays locked across the write, so a withdrawal either lands first and
    /// silences the answer or lands after omp already holds it.
    fn answer_uri(&self, id: &str, url: &str) {
        let answer = (self.read_uri)(url);
        let mut uris = self.uris.lock().unwrap_or_else(PoisonError::into_inner);
        if !uris.remove(id) {
            return;
        }
        let _answered = self.write(&match answer {
            Ok(content) => vendor::omp::host_uri_result(id, &content),
            Err(reason) => vendor::omp::host_uri_refused(id, &reason),
        });
    }

    /// Silence the answer to a withdrawn read, if one is still pending.
    pub(super) fn withdraw_uri(&self, id: &str) {
        self.uris
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
    }
}

#[cfg(test)]
mod tests;
