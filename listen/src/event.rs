use tape::Locus;

use serde::{Deserialize, Serialize};

/// Closed vocabulary of listening moments.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// The vendor declared the exact resumable session identity.
    Session,
    /// Prose attributable to an operator or assistant role.
    Prose,
    /// Model reasoning, whether rendered or opaque.
    Reasoning,
    /// A tool call or its result.
    Tool,
    /// Token usage reported by the vendor.
    Usage,
    /// A context compaction boundary.
    Compaction,
    /// A vendor-reported failure.
    Failure,
}

impl EventKind {
    /// Every kind in stable canary order.
    pub const ALL: [Self; 7] = [
        Self::Session,
        Self::Prose,
        Self::Reasoning,
        Self::Tool,
        Self::Usage,
        Self::Compaction,
        Self::Failure,
    ];

    /// Stable spelling carried by canary evidence.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Prose => "prose",
            Self::Reasoning => "reasoning",
            Self::Tool => "tool",
            Self::Usage => "usage",
            Self::Compaction => "compaction",
            Self::Failure => "failure",
        }
    }
}

/// One typed moment decoded from a vendor record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    /// Exact resumable vendor session declaration.
    Session(SessionDeclaration),
    /// Prose attributable to an operator or assistant role.
    Prose(Prose),
    /// Model reasoning.
    Reasoning(Reasoning),
    /// Tool activity.
    Tool(Tool),
    /// Token usage.
    Usage(Usage),
    /// Context compaction.
    Compaction(Compaction),
    /// Vendor failure.
    Failure(Failure),
}

impl Event {
    /// Closed kind of this event.
    #[must_use]
    pub const fn kind(&self) -> EventKind {
        match self {
            Self::Session(_) => EventKind::Session,
            Self::Prose(_) => EventKind::Prose,
            Self::Reasoning(_) => EventKind::Reasoning,
            Self::Tool(_) => EventKind::Tool,
            Self::Usage(_) => EventKind::Usage,
            Self::Compaction(_) => EventKind::Compaction,
            Self::Failure(_) => EventKind::Failure,
        }
    }

    /// Tape bytes that supplied this event.
    #[must_use]
    pub const fn locus(&self) -> Locus {
        match self {
            Self::Session(event) => event.locus,
            Self::Prose(event) => event.locus,
            Self::Reasoning(event) => event.locus,
            Self::Tool(event) => event.locus,
            Self::Usage(event) => event.locus,
            Self::Compaction(event) => event.locus,
            Self::Failure(event) => event.locus,
        }
    }
}

/// The exact vendor identity declared by a current session record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionDeclaration {
    /// Vendor spelling of the resumable session identity.
    pub identity: String,
    /// Source bytes carrying the declaration.
    pub locus: Locus,
}

/// Visible transcript text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Prose {
    /// Side that authored the visible text.
    pub role: ProseRole,
    /// Text reported by the vendor.
    pub text: String,
    /// Vendor phase when the current dialect reports one.
    pub phase: Option<String>,
    /// Admitted media blocks carried beside this authored text.
    pub media: Vec<wire::Media>,
    /// Source bytes.
    pub locus: Locus,
}

/// Author of one visible transcript message.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ProseRole {
    /// Operator-authored input.
    Operator,
    /// Assistant-authored delivery.
    Assistant,
}

/// Reasoning visibility.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReasoningContent {
    /// Plain reasoning text reported by the vendor.
    Plain(Vec<String>),
    /// A reasoning item exists, but its text is encrypted or withheld.
    Opaque,
}

/// A reasoning moment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Reasoning {
    /// Available reasoning representation.
    pub content: ReasoningContent,
    /// Source bytes.
    pub locus: Locus,
}

/// Whether a tool record starts or completes a call.
///
/// A call carries its arguments as the vendor's own JSON payload rather than a
/// parsed shape: every dialect names its argument fields differently, and a
/// reader that wants the command line is reading vendor vocabulary either way.
/// A result carries neither name nor arguments, because no dialect repeats them
/// on the record that answers a call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolMoment {
    /// A call names its tool and the arguments it was given.
    Call {
        /// Tool name.
        name: String,
        /// Vendor call payload, serialized as JSON.
        arguments: String,
    },
    /// A running call published updated state.
    Update {
        /// Tool name.
        name: String,
        /// Vendor call payload, serialized as JSON.
        arguments: String,
    },
    /// A result reports its normalized outcome and available output evidence.
    Result(ToolOutcome),
}

/// Normalized evidence attached to one tool result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolOutcome {
    /// Vendor knowledge of success or failure.
    pub result: ToolResult,
    /// Rendered result body when the vendor records one.
    pub output: Option<String>,
    /// Process exit code when the result reports one directly or structurally.
    pub exit_code: Option<i32>,
    /// Admitted media blocks carried by this exact result.
    pub media: Vec<wire::Media>,
}

/// Vendor knowledge of a tool result.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ToolResult {
    /// The vendor reported success.
    Succeeded,
    /// The vendor reported failure.
    Failed,
    /// This current result record carries no status field.
    Unreported,
}

/// A tool call or result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tool {
    /// Vendor call identifier joining calls to results.
    pub call_id: String,
    /// Position in the tool lifecycle.
    pub moment: ToolMoment,
    /// Source bytes.
    pub locus: Locus,
}

/// Common token counters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Usage {
    /// Completed vendor turns represented by this final usage event.
    pub turns: u64,
    /// Uncached input tokens.
    pub input_tokens: u64,
    /// Cached input tokens.
    pub cached_input_tokens: u64,
    /// Input tokens written to a cache.
    pub cache_write_input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Reasoning output tokens, when the dialect exposes them.
    pub reasoning_output_tokens: Option<u64>,
    /// Source bytes.
    pub locus: Locus,
}

/// A context compaction boundary.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Compaction {
    /// Vendor-provided summary when this record carries one.
    pub summary: Option<String>,
    /// Vendor identity of the compaction, when the record names one.
    ///
    /// The captured stream announcement and the preserved session entry are
    /// two records of one compaction. This identity is what lets a consumer
    /// recognize the pair instead of comparing summary text.
    pub boundary: Option<wire::MessageBoundary>,
    /// Source bytes.
    pub locus: Locus,
}

/// A vendor-reported failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Failure {
    /// Vendor failure message or reason.
    pub message: String,
    /// Source bytes.
    pub locus: Locus,
}
