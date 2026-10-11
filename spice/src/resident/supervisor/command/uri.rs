//! The `spice://` scheme a lane's omp reads through: each read of a quote
//! target answers from the resident's authenticated quote resolution.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, PoisonError};

use wire::CanonicalUuid;

use super::Driver;

/// Resolves one quote target into what a read of it returns, or the refusal.
pub(super) type QuoteReader = Box<dyn Fn(&str) -> Result<String, String> + Send + Sync>;

/// Read quote targets from the resident over the lane's own connection.
pub(super) fn resident_reader(
    discovery: PathBuf,
    lane: CanonicalUuid,
) -> impl Fn(&str) -> Result<String, String> + Send + Sync + 'static {
    move |target| match crate::command_client::read_lane_quote(&discovery, lane, target) {
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
}

impl<W: Write + Send + 'static> Driver<W> {
    /// Answer one host URI request: a read of a quote target, resolved by
    /// the resident on its own thread so the stream keeps being read.
    ///
    /// Resolution commits nothing, so omp may read as often as its tools ask.
    /// A write, or any URI that is not a quote target, is refused at once
    /// with what the scheme serves.
    pub(super) fn serve_uri(self: &Arc<Self>, id: &str, operation: &str, url: &str) {
        if operation != "read" || !url.starts_with("spice://quote/") {
            let _refused = self.write(&vendor::omp::host_uri_refused(id, vendor::omp::URI_SERVED));
            return;
        }
        self.uris
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.to_owned());
        let driver = Arc::clone(self);
        let (answered, target) = (id.to_owned(), url.to_owned());
        let started = std::thread::Builder::new()
            .name("spice-quote".to_owned())
            .spawn(move || driver.answer_uri(&answered, &target));
        if let Err(error) = started {
            self.uris
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(id);
            let _refused = self.write(&vendor::omp::host_uri_refused(
                id,
                &format!("quote thread failed: {error}"),
            ));
        }
    }

    /// Answer one quote read unless omp withdrew it meanwhile. The pending
    /// set stays locked across the write, so a withdrawal either lands first
    /// and silences the answer or lands after omp already holds it.
    fn answer_uri(&self, id: &str, target: &str) {
        let answer = (self.read_quote)(target);
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
