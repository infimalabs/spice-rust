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
    /// The vendor field does not spell a name the wire accepts.
    #[error("Vendor must be one lower-case word, so {written:?} cannot name one.")]
    Vendor {
        /// Text as the operator wrote it, before any normalisation.
        written: String,
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

/// The seat fields a request carries once the form input is accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seat {
    /// Optional operator-facing identity, distinct from the worktree path.
    pub name: Option<String>,
    /// Absolute path of the worktree the agent runs in.
    pub worktree: String,
    /// Life the seated agent takes, spelled as the lane crate spells it.
    pub life: String,
    /// Vendor name, normalised to the one lower-case word the wire requires.
    pub vendor: wire::Name,
}

/// The model and effort a seat selects for its occupant, each absent when
/// the vendor's configured value governs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    /// Model spelling handed to the vendor as written.
    pub model: Option<String>,
    /// Effort spelling, folded to the lower-case word the closed vocabulary
    /// spells; the resident refuses one outside it.
    pub effort: Option<String>,
}

/// Compose a seat's model and effort selection from its two optional fields.
///
/// Neither field can refuse: a blank one leaves that value to the vendor
/// configuration, and an effort outside the closed vocabulary is refused by
/// the resident, which is the one place that vocabulary is spelled.
#[must_use]
pub fn selection(model: &str, effort: &str) -> Selection {
    let present = |written: &str| match written.trim() {
        "" => None,
        written => Some(written.to_owned()),
    };
    Selection {
        model: present(model),
        effort: present(effort).map(|effort| effort.to_ascii_lowercase()),
    }
}

/// One accepted task observation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    /// Exact task the observation belongs to.
    pub task: wire::TaskHandle,
    /// Trimmed observation text.
    pub text: String,
}

/// Compose a seat request, or say why this input cannot make one.
///
/// The vendor is folded to lower case exactly as the life is, because
/// [`wire::Name`] accepts only lower-case words and `Claude` and `claude` can
/// have meant nothing different. Folding is not forgiveness: a vendor holding
/// a space, a digit, or a hyphen still spells no name, and is refused by name.
///
/// Fields are checked in the order the form lists them, so an operator filling
/// it top to bottom is never sent back up past a field they already passed.
///
/// # Errors
///
/// Returns [`Refusal::Missing`] for any blank field, and [`Refusal::Vendor`]
/// when the vendor is written but does not spell a name even folded.
pub fn seat(name: &str, worktree: &str, vendor: &str, life: &str) -> Result<Seat, Refusal> {
    let name = match name.trim() {
        "" => None,
        configured => Some(configured.to_owned()),
    };
    let worktree = worktree.trim().to_owned();
    if worktree.is_empty() {
        return Err(Refusal::Missing {
            field: "Worktree",
            hint: "give the absolute path of a worktree in this repository",
        });
    }
    let written = vendor.trim();
    if written.is_empty() {
        return Err(Refusal::Missing {
            field: "Vendor",
            hint: "one lower-case word such as claude or codex",
        });
    }
    let vendor = written
        .to_ascii_lowercase()
        .parse::<wire::Name>()
        .map_err(|_| Refusal::Vendor {
            written: written.to_owned(),
        })?;
    let life = life.trim().to_ascii_lowercase();
    if life.is_empty() {
        return Err(Refusal::Missing {
            field: "Life",
            hint: "steer, drive, or drain",
        });
    }
    Ok(Seat {
        name,
        worktree,
        life,
        vendor,
    })
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
