//! The `spice://` scheme a lane's omp reads and writes through: each request
//! answers with the [`crate::resource`] the URL names, over the lane's own
//! connection.

use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use wire::CanonicalUuid;

use super::Driver;
use crate::resource::Access;

/// Answers one `spice://` read or write with its result, or the refusal.
pub(super) type UriServer = Box<dyn Fn(&str, Access<'_>) -> Result<String, String> + Send + Sync>;

/// The `spice://` server and the requests it is answering now. A withdrawn
/// request leaves the pending set, and only a request still in it is answered.
pub(super) struct Uris {
    server: UriServer,
    pending: Mutex<HashSet<String>>,
}

impl Uris {
    pub(super) fn new(server: UriServer) -> Self {
        Self {
            server,
            pending: Mutex::new(HashSet::new()),
        }
    }
}

/// Serve `spice://` URLs as `lane`, whose checkout is `worktree` and whose
/// occupant's session state is `environment`, resolving quote targets, CLI
/// reads, and task text writes through the resident. A human-authored quote
/// names its author from the principal index of the journal at `journal`.
pub(super) fn resident_server(
    discovery: PathBuf,
    journal: PathBuf,
    lane: CanonicalUuid,
    worktree: PathBuf,
    environment: crate::session_command::Environment,
) -> UriServer {
    let seat = crate::resource::Seat {
        worktree,
        lane,
        environment,
        graphs: Default::default(),
    };
    Box::new(move |url, access| {
        seat.serve(
            url,
            access,
            |target| match crate::command_client::read_lane_quote(&discovery, lane, target) {
                Ok(resolution) => {
                    let principals = spice::quote::author_names(&journal, &resolution)?;
                    spice::quote::render(&resolution, principals.as_ref())
                }
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
            },
        )
    })
}

impl<W: Write + Send + 'static> Driver<W> {
    /// Answer one host URI request, a read or a write carrying its content,
    /// on its own thread so the stream keeps being read. Any other operation
    /// is refused at once.
    ///
    /// Reading commits nothing, so omp may read as often as its tools ask.
    pub(super) fn serve_uri(
        self: &Arc<Self>,
        id: &str,
        operation: &str,
        url: &str,
        content: Option<&str>,
    ) {
        let content = match (operation, content) {
            ("read", _) => None,
            ("write", Some(content)) => Some(content.to_owned()),
            _ => {
                let _refused =
                    self.write(&vendor::omp::host_uri_refused(id, vendor::omp::URI_SERVED));
                return;
            }
        };
        self.uris
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.to_owned());
        let driver = Arc::clone(self);
        let (answered, url) = (id.to_owned(), url.to_owned());
        let started = std::thread::Builder::new()
            .name("spice-uri".to_owned())
            .spawn(move || {
                let access = content.as_deref().map_or(Access::Read, Access::Write);
                driver.answer_uri(&answered, &url, access);
            });
        if let Err(error) = started {
            self.uris
                .pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(id);
            let _refused = self.write(&vendor::omp::host_uri_refused(
                id,
                &format!("URI thread failed: {error}"),
            ));
        }
    }

    /// Answer one request unless omp withdrew it meanwhile. The pending set
    /// stays locked across the answer, so a withdrawal either lands first and
    /// silences it or lands after omp already holds it. A withdrawn write
    /// that already committed stays committed; only its answer is silenced.
    fn answer_uri(&self, id: &str, url: &str, access: Access<'_>) {
        let answer = (self.uris.server)(url, access);
        let mut pending = self
            .uris
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !pending.remove(id) {
            return;
        }
        let _answered = self.write(&match answer {
            Ok(content) => vendor::omp::host_uri_result(id, &content),
            Err(reason) => vendor::omp::host_uri_refused(id, &reason),
        });
    }

    /// Silence the answer to a withdrawn request, if one is still pending.
    pub(super) fn withdraw_uri(&self, id: &str) {
        self.uris
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
    }
}

#[cfg(test)]
impl<W: Write + Send + 'static> Driver<W> {
    /// A driver whose `spice://` server answers reads through `read_uri` and
    /// refuses every write.
    pub(super) fn with_readers(
        stdin: W,
        read_inbox: impl Fn() -> Option<Vec<u8>> + Send + Sync + 'static,
        read_briefing: impl Fn() -> Option<String> + Send + Sync + 'static,
        read_uri: impl Fn(&str) -> Result<String, String> + Send + Sync + 'static,
        seal: steer::Key,
        terminal: Option<super::Terminal>,
    ) -> Arc<Self> {
        let server: UriServer = Box::new(move |url, access| match access {
            Access::Read => read_uri(url),
            Access::Write(_) => Err(vendor::omp::URI_SERVED.to_owned()),
        });
        Self::with_server(
            stdin,
            super::Readers {
                inbox: Box::new(read_inbox),
                briefing: Box::new(read_briefing),
                ..super::Readers::default()
            },
            server,
            seal,
            terminal,
        )
    }

    /// A driver that reads pending steering and a resumed seat's notice when
    /// releasing its first prompt, and serves no `spice://` read.
    pub(super) fn with_startup_readers(
        stdin: W,
        read_inbox: impl Fn() -> Option<Vec<u8>> + Send + Sync + 'static,
        read_resume: impl Fn() -> String + Send + Sync + 'static,
    ) -> Arc<Self> {
        let server: UriServer = Box::new(|_, _| Err(vendor::omp::URI_SERVED.to_owned()));
        Self::with_server(
            stdin,
            super::Readers {
                inbox: Box::new(read_inbox),
                resume: Box::new(read_resume),
                ..super::Readers::default()
            },
            server,
            steer::Key::for_identity(crate::resident::new_uuid()),
            None,
        )
    }

    /// A driver that reads a resumed seat's notice through `read_resume` and
    /// serves no `spice://` read.
    pub(super) fn resuming(
        stdin: W,
        read_resume: impl Fn() -> String + Send + Sync + 'static,
    ) -> Arc<Self> {
        Self::seating(stdin, read_resume, || true)
    }

    /// A resuming driver whose occupant is seated whenever `read_seated` says.
    pub(super) fn seating(
        stdin: W,
        read_resume: impl Fn() -> String + Send + Sync + 'static,
        read_seated: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Arc<Self> {
        let server: UriServer = Box::new(|_, _| Err(vendor::omp::URI_SERVED.to_owned()));
        Self::with_server(
            stdin,
            super::Readers {
                resume: Box::new(read_resume),
                seated: Box::new(read_seated),
                ..super::Readers::default()
            },
            server,
            steer::Key::for_identity(crate::resident::new_uuid()),
            None,
        )
    }
}

#[cfg(test)]
mod tests;
