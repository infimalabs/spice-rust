use tape::Locus;

/// Closed vocabulary of listening moments.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub enum EventKind {
    /// Assistant prose visible to the operator.
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
    pub const ALL: [Self; 6] = [
        Self::Prose,
        Self::Reasoning,
        Self::Tool,
        Self::Usage,
        Self::Compaction,
        Self::Failure,
    ];
}

/// One typed moment decoded from a vendor record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    /// Assistant prose.
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
            Self::Prose(event) => event.locus,
            Self::Reasoning(event) => event.locus,
            Self::Tool(event) => event.locus,
            Self::Usage(event) => event.locus,
            Self::Compaction(event) => event.locus,
            Self::Failure(event) => event.locus,
        }
    }
}

/// Visible assistant text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Prose {
    /// Text reported by the vendor.
    pub text: String,
    /// Vendor phase when the current dialect reports one.
    pub phase: Option<String>,
    /// Source bytes.
    pub locus: Locus,
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolMoment {
    /// A call names its tool.
    Call { name: String },
    /// A running call published updated state.
    Update { name: String },
    /// A result reports its outcome when available.
    Result(ToolResult),
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
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct Compaction {
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
