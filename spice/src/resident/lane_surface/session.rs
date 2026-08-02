//! Session-declaration proof, durable resume, and its refusals.

use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum SessionProofError {
    Missing,
    Capture(String),
    CrossLane {
        expected: CanonicalUuid,
        observed: CanonicalUuid,
    },
    CrossOccupant {
        expected: CanonicalUuid,
        observed: CanonicalUuid,
    },
    WrongStream,
    Mismatched {
        expected: String,
        observed: String,
    },
    Duplicate {
        identity: String,
    },
}

impl SessionProofError {
    pub(super) fn refusal(&self) -> Refusal {
        let (reason, expected, observed, remedy) = match self {
            Self::Missing => (
                "lane::session-missing",
                Some("one current selected-stream session declaration".to_owned()),
                None,
                "retry with a vendor that emits its current initialization record",
            ),
            Self::Capture(detail) => (
                "lane::session-capture-failed",
                Some("one intact selected-stream capture".to_owned()),
                Some(detail.clone()),
                "repair the capture path and retry the lane launch",
            ),
            Self::CrossLane { expected, observed } => (
                "lane::session-cross-lane",
                Some(expected.to_string()),
                Some(observed.to_string()),
                "use only the declaration emitted by this lane's supervised occupant",
            ),
            Self::CrossOccupant { expected, observed } => (
                "lane::session-cross-occupant",
                Some(expected.to_string()),
                Some(observed.to_string()),
                "use only the declaration emitted by the current supervised occupant",
            ),
            Self::WrongStream => (
                "lane::session-wrong-stream",
                Some("the vendor-selected record stream".to_owned()),
                Some("another child stream".to_owned()),
                "declare the session on the selected JSONL stream",
            ),
            Self::Mismatched { expected, observed } => (
                "lane::session-mismatched",
                Some(expected.clone()),
                Some(observed.clone()),
                "resume the exact durable session identity",
            ),
            Self::Duplicate { identity } => (
                "lane::session-duplicate",
                Some("one declaration per occupant".to_owned()),
                Some(identity.clone()),
                "emit the initialization declaration exactly once",
            ),
        };
        Refusal {
            reason: reason.to_owned(),
            witness: Some("session-declaration".to_owned()),
            expected,
            observed,
            remedy: remedy.to_owned(),
        }
    }

    pub(super) fn outcome(&self) -> Outcome<CommandAnswer> {
        Outcome::Refusal(self.refusal())
    }
}

pub(super) struct DeclarationState {
    pub(super) reply: mpsc::SyncSender<Result<wire::LaneSession, SessionProofError>>,
    pub(super) seen: Option<String>,
}

pub(super) struct DeclarationReporter {
    pub(super) inner: Arc<dyn CaptureReporter>,
    pub(super) lane: CanonicalUuid,
    pub(super) occupant: CanonicalUuid,
    pub(super) worktree: String,
    pub(super) vendor: wire::Name,
    pub(super) epoch: CanonicalUuid,
    pub(super) stream: vendor::RecordStream,
    pub(super) expected: Option<String>,
    pub(super) state: Mutex<DeclarationState>,
}

impl DeclarationReporter {
    fn reject(&self, state: &mut DeclarationState, error: SessionProofError) -> CaptureReportError {
        let _sent = state.reply.try_send(Err(error.clone()));
        CaptureReportError::new(format!("session declaration refused: {error:?}"))
    }
}

impl CaptureReporter for DeclarationReporter {
    fn record(&self, record: CaptureRecord) -> Result<(), CaptureReportError> {
        let Some(declaration) = record.declaration.as_ref() else {
            return self.inner.record(record);
        };
        let mut state = self
            .state
            .lock()
            .map_err(|_| CaptureReportError::new("session proof lock is poisoned"))?;
        if let Some(identity) = state.seen.clone() {
            return Err(self.reject(&mut state, SessionProofError::Duplicate { identity }));
        }
        if record.lane != self.lane {
            return Err(self.reject(
                &mut state,
                SessionProofError::CrossLane {
                    expected: self.lane,
                    observed: record.lane,
                },
            ));
        }
        if record.occupant != self.occupant {
            return Err(self.reject(
                &mut state,
                SessionProofError::CrossOccupant {
                    expected: self.occupant,
                    observed: record.occupant,
                },
            ));
        }
        if record.stream != self.stream {
            return Err(self.reject(&mut state, SessionProofError::WrongStream));
        }
        let identity = declaration.session.as_str().to_owned();
        if let Some(expected) = &self.expected
            && expected != &identity
        {
            return Err(self.reject(
                &mut state,
                SessionProofError::Mismatched {
                    expected: expected.clone(),
                    observed: identity,
                },
            ));
        }
        let session = wire::LaneSession {
            lane: self.lane,
            occupant: self.occupant,
            worktree: self.worktree.clone(),
            vendor: self.vendor.clone(),
            epoch: self.epoch,
            stream: record_stream(self.stream).to_owned(),
            identity: identity.clone(),
            locus: wire::TapeLocus {
                member: declaration.locus.member,
                offset: declaration.locus.offset,
                length: declaration.locus.length,
            },
        };
        self.inner.record(record)?;
        state.seen = Some(identity);
        state.reply.try_send(Ok(session)).map_err(|error| {
            CaptureReportError::new(format!(
                "session proof receiver refused declaration: {error}"
            ))
        })
    }

    fn failed(&self, failure: CaptureFailure) {
        if let Ok(state) = self.state.lock() {
            let _sent = state
                .reply
                .try_send(Err(SessionProofError::Capture(failure.failure.to_string())));
        }
        self.inner.failed(failure);
    }
}

pub(super) fn session_fact(session: &wire::LaneSession) -> wire::body::LaneSessionDeclared {
    wire::body::LaneSessionDeclared {
        lane: session.lane,
        occupant: session.occupant,
        worktree: session.worktree.clone(),
        vendor: session.vendor.clone(),
        epoch: session.epoch,
        stream: session.stream.clone(),
        identity: session.identity.clone(),
        member: session.locus.member,
        offset: session.locus.offset,
        length: session.locus.length,
    }
}

pub(super) fn record_stream(stream: vendor::RecordStream) -> &'static str {
    match stream {
        vendor::RecordStream::Stdout => "stdout",
        vendor::RecordStream::Stderr => "stderr",
    }
}

pub(super) fn await_session(
    session: mpsc::Receiver<Result<wire::LaneSession, SessionProofError>>,
) -> Result<wire::LaneSession, SessionProofError> {
    match session.recv_timeout(SESSION_PROOF) {
        Ok(Ok(declared)) => match session.recv_timeout(SESSION_SETTLE) {
            Ok(Err(error)) => Err(error),
            Ok(Ok(duplicate)) => Err(SessionProofError::Duplicate {
                identity: duplicate.identity,
            }),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(declared),
            Err(mpsc::RecvTimeoutError::Disconnected) => Ok(declared),
        },
        Ok(Err(error)) => Err(error),
        Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => {
            Err(SessionProofError::Missing)
        }
    }
}

pub(super) fn resume_session(
    paths: &RuntimePaths,
    row: &wire::LaneResidency,
) -> CommandResult<vendor::Resume> {
    let session = validate_session_head(row)?;
    let dialect = session_dialect(&row.vendor)?;
    let identity = prove_session_tape(paths, row, session, dialect)?;
    Ok(vendor::Resume::Session(identity))
}

/// Admit only a session head bound to this exact lane, worktree, vendor,
/// latest occupant, and selected stream.
fn validate_session_head(row: &wire::LaneResidency) -> CommandResult<&wire::LaneSession> {
    let session = row.session.as_ref().ok_or_else(|| {
        session_head_refusal(
            "lane::session-authority-missing",
            Some("lane::declared authority from the current release".to_owned()),
            Some("legacy occupied lane without session authority".to_owned()),
            "drain and re-seat this lane under the current release",
        )
    })?;
    if session.lane != row.lane {
        return Err(session_head_refusal(
            "lane::session-cross-lane",
            Some(row.lane.to_string()),
            Some(session.lane.to_string()),
            "restore the session head bound to this exact lane",
        ));
    }
    if session.worktree != row.worktree {
        return Err(session_head_refusal(
            "lane::session-wrong-worktree",
            Some(row.worktree.clone()),
            Some(session.worktree.clone()),
            "restore the session head bound to this exact worktree",
        ));
    }
    if session.vendor != row.vendor {
        return Err(session_head_refusal(
            "lane::session-cross-vendor",
            Some(row.vendor.to_string()),
            Some(session.vendor.to_string()),
            "restore the session head admitted by this lane's vendor",
        ));
    }
    let expected_occupant = row.lineage.last().copied().ok_or_else(|| {
        session_head_refusal(
            "lane::session-lineage-missing",
            Some("the occupant that declared the session".to_owned()),
            None,
            "repair the durable lane lineage before resuming",
        )
    })?;
    if session.occupant != expected_occupant {
        return Err(session_head_refusal(
            "lane::session-cross-occupant",
            Some(expected_occupant.to_string()),
            Some(session.occupant.to_string()),
            "restore the declaration made by the lane's latest occupant",
        ));
    }
    if session.stream != "stdout" {
        return Err(session_head_refusal(
            "lane::session-wrong-stream",
            Some("stdout".to_owned()),
            Some(session.stream.clone()),
            "restore the locus from the vendor-selected JSONL stream",
        ));
    }
    Ok(session)
}

/// Select the declaration dialect owned by this lane's vendor.
fn session_dialect(vendor: &wire::Name) -> CommandResult<listen::Dialect> {
    let dialect = match vendor.as_str() {
        "claude" => listen::Dialect::Claude,
        "codex" => listen::Dialect::Codex,
        other => {
            return Err(session_head_refusal(
                "lane::session-vendor-unsupported",
                Some("claude or codex".to_owned()),
                Some(other.to_owned()),
                "select a vendor with a current session declaration dialect",
            ));
        }
    };
    Ok(dialect)
}

/// Prove the durable tape still holds the exact declaration the head names.
fn prove_session_tape(
    paths: &RuntimePaths,
    row: &wire::LaneResidency,
    session: &wire::LaneSession,
    dialect: listen::Dialect,
) -> CommandResult<vendor::SessionId> {
    let tape = tape::Tape::open_existing(
        paths
            .tape
            .join(row.lane.to_string())
            .join(row.vendor.as_str()),
    )
    .map_err(|error| {
        session_head_refusal(
            "lane::session-tape-moved",
            Some("the exact durable lane/vendor tape".to_owned()),
            Some(error.to_string()),
            "restore the preserved tape before resuming this lane",
        )
    })?;
    let locus = listen::Locus::new(
        session.locus.member,
        session.locus.offset,
        session.locus.length,
    );
    let bytes = tape.read(locus).map_err(|error| {
        session_head_refusal(
            "lane::session-tape-moved",
            Some(locus.to_string()),
            Some(error.to_string()),
            "restore the exact declaration bytes before resuming this lane",
        )
    })?;
    let decoded = listen::decode(dialect, &bytes, locus).map_err(|error| {
        session_head_refusal(
            "lane::session-tape-moved",
            Some(session.identity.clone()),
            Some(error.to_string()),
            "restore the exact declaration record before resuming this lane",
        )
    })?;
    let declared = decoded.events().iter().find_map(|event| match event {
        listen::Event::Session(declaration) => Some(declaration.identity.as_str()),
        _ => None,
    });
    if declared != Some(session.identity.as_str()) {
        return Err(session_head_refusal(
            "lane::session-tape-moved",
            Some(session.identity.clone()),
            declared.map(str::to_owned),
            "restore the declaration record named by the durable locus",
        ));
    }
    let identity = vendor::SessionId::new(session.identity.clone()).map_err(|error| {
        session_head_refusal(
            "lane::session-identity-invalid",
            Some("one nonempty vendor session identity".to_owned()),
            Some(error.to_string()),
            "repair the durable session authority before resuming",
        )
    })?;
    Ok(identity)
}

pub(super) fn session_head_refusal(
    reason: &str,
    expected: Option<String>,
    observed: Option<String>,
    remedy: &str,
) -> Outcome<()> {
    Outcome::Refusal(Refusal {
        reason: reason.to_owned(),
        witness: Some("lane-session-head".to_owned()),
        expected,
        observed,
        remedy: remedy.to_owned(),
    })
}
