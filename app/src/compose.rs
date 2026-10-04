//! Operator form input composed into the fields a client action carries.
//!
//! Every other floor faces inward, holding what the resident said. This one
//! faces the other way: it is the only place where text an operator typed
//! becomes a request, and so the only place that can refuse one. A refusal
//! here is operator-facing prose rather than a log line, because the form it
//! came from has a button, and a button that does nothing and says nothing is
//! indistinguishable from a dead control or a lost request.
//!
//! The browser module that owns the forms compiles only for wasm32, so no
//! native test can reach a decision made inside a `Callback`. Keeping the
//! decisions here instead means each refusal is reachable from an ordinary
//! test on any target, and the callback is left with nothing to do but render
//! what it is handed.

use core::fmt;

/// Why one composed submission cannot become a request.
///
/// Each variant renders as a sentence naming both what was wrong and what to
/// do about it, because the operator reading it is looking at the field that
/// produced it and needs to know which one to change.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// A required field was left empty or held only whitespace.
    #[error("{field} is required: {hint}.")]
    Missing {
        /// Label of the field as the form spells it.
        field: &'static str,
        /// What an accepted value for that field looks like.
        hint: &'static str,
    },
    /// The task field does not spell the wire's task-handle grammar.
    #[error(
        "Task must be `<UPPER-STEM>-<stamp>` with an eight- or nine-character canonical task stamp, so {written:?} cannot name one."
    )]
    Task {
        /// Text as the operator wrote it, after surrounding whitespace is removed.
        written: String,
    },
    /// No seated agent is selected to receive the message.
    #[error("Select a seated agent before sending direction.")]
    Unseated,
}

/// One accepted task observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    /// Exact task the observation belongs to.
    pub task: wire::TaskHandle,
    /// Trimmed observation text.
    pub text: String,
}

/// Compose steering text for a selected agent, or say why it cannot be sent.
///
/// `seated` is the caller's lane lookup already resolved, and it is returned
/// inside the accepted result rather than checked beside it: the caller cannot
/// hold an accepted message without also holding the seat it goes to, so no
/// arm downstream has to assert a pairing this function already guarantees.
/// The form disables its button while no lane is selected, so arriving here
/// with [`None`] means the agent left its seat between the render and the
/// click rather than that the operator ignored a disabled control. It still
/// earns a sentence, since to the operator the send simply did not happen.
///
/// # Errors
///
/// Returns [`Refusal::Unseated`] when no seated agent is selected, and
/// [`Refusal::Missing`] when the message is blank.
pub fn steer<T>(text: &str, seated: Option<T>) -> Result<(T, String), Refusal> {
    let Some(seated) = seated else {
        return Err(Refusal::Unseated);
    };
    let text = text.trim().to_owned();
    if text.is_empty() {
        return Err(Refusal::Missing {
            field: "Message",
            hint: "type the direction to send",
        });
    }
    Ok((seated, text))
}

/// Compose a task observation, or say why it cannot be sent.
///
/// Fields are checked in the order the note form presents them: task first,
/// then observation. A nonempty malformed handle is refused here rather than
/// disappearing at the browser callback boundary.
///
/// # Errors
///
/// Returns [`Refusal::Missing`] for a blank field and [`Refusal::Task`] when
/// the task text does not spell a [`wire::TaskHandle`].
pub fn note(task: &str, text: &str) -> Result<Note, Refusal> {
    let written = task.trim();
    if written.is_empty() {
        return Err(Refusal::Missing {
            field: "Task",
            hint: "choose a task handle such as SPICE-1kKRRsDP",
        });
    }
    let task = written
        .parse::<wire::TaskHandle>()
        .map_err(|_| Refusal::Task {
            written: written.to_owned(),
        })?;
    let text = text.trim().to_owned();
    if text.is_empty() {
        return Err(Refusal::Missing {
            field: "Observation",
            hint: "type the observation to append",
        });
    }
    Ok(Note { task, text })
}

/// Compose the validation that closes a task, or say why it cannot be sent.
///
/// # Errors
///
/// Returns [`Refusal::Missing`] when the validation is blank.
pub fn close(text: &str) -> Result<String, Refusal> {
    let text = text.trim();
    if text.is_empty() {
        return Err(Refusal::Missing {
            field: "Validation",
            hint: "type the evidence or reason the task is complete",
        });
    }
    Ok(text.to_owned())
}

/// A refusal held for display, or nothing while the last submission stood.
///
/// The form keeps one of these rather than a bare [`Option<String>`] so that
/// clearing it is a state the type names: a submission that succeeds sets
/// [`Notice::none`], and the stale reason for a problem the operator has since
/// fixed cannot survive under the field it no longer describes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Notice(Option<Refusal>);

impl Notice {
    /// No refusal outstanding.
    #[must_use]
    pub fn none() -> Self {
        Self(None)
    }

    /// The refusal to show, if the last submission was refused.
    #[must_use]
    pub fn refusal(&self) -> Option<&Refusal> {
        self.0.as_ref()
    }

    /// Whether there is a refusal to show.
    #[must_use]
    pub fn is_some(&self) -> bool {
        self.0.is_some()
    }
}

impl From<Refusal> for Notice {
    fn from(refusal: Refusal) -> Self {
        Self(Some(refusal))
    }
}

impl fmt::Display for Notice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Some(refusal) => write!(formatter, "{refusal}"),
            None => Ok(()),
        }
    }
}
