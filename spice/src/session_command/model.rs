use std::collections::{BTreeMap, BTreeSet};

use serde::{Serialize, Serializer};

use crate::message_shape::MessageShape;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Vendor(Option<&'static spice::extension::Vendor>);

impl Vendor {
    pub(crate) const UNKNOWN: Self = Self(None);

    pub(crate) const fn declared(vendor: &'static spice::extension::Vendor) -> Self {
        Self(Some(vendor))
    }

    pub(crate) fn inferred(dialect: listen::Dialect) -> Self {
        let vendor = spice::extension::registry()
            .expect("compiled extension declarations are validated by the test gate")
            .session_vendors()
            .into_iter()
            .find(|vendor| vendor.session_study().dialect() == dialect)
            .expect("every compiled transcript dialect has one study declaration");
        Self::declared(vendor)
    }

    pub(crate) const fn is_unknown(self) -> bool {
        self.0.is_none()
    }

    fn identity(self) -> &'static str {
        self.0
            .map_or("unknown", |vendor| vendor.session_study().identity())
    }
}

impl Serialize for Vendor {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.identity())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceView {
    pub(crate) path: String,
    pub(crate) name: String,
    pub(crate) vendor: Vendor,
    pub(crate) thread: Option<String>,
    pub(crate) compressed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Residue {
    pub(crate) source: String,
    pub(crate) line: u64,
    pub(crate) kind: String,
    pub(crate) detail: String,
    pub(crate) recovery: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MessageRecord {
    pub(crate) source: String,
    pub(crate) ts: String,
    pub(crate) turn_id: Option<String>,
    pub(crate) side: String,
    pub(crate) phase: String,
    pub(crate) text: String,
    pub(crate) primary_flavor: String,
    pub(crate) flavor_tags: Vec<String>,
    pub(crate) matched_cues: Vec<String>,
    pub(crate) shape: MessageShape,
    pub(crate) authored: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MessageView {
    pub(crate) source: String,
    pub(crate) ts: String,
    pub(crate) turn_id: Option<String>,
    pub(crate) side: String,
    pub(crate) phase: String,
    pub(crate) shape: MessageShape,
    pub(crate) flavor: String,
    pub(crate) tags: Vec<String>,
    pub(crate) cues: Vec<String>,
    pub(crate) text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TurnRecord {
    pub(crate) source: String,
    pub(crate) vendor: Vendor,
    pub(crate) turn_id: Option<String>,
    pub(crate) start_ts: String,
    pub(crate) end_ts: Option<String>,
    pub(crate) last_activity_ts: String,
    pub(crate) completed: bool,
    pub(crate) messages: Vec<MessageRecord>,
    pub(crate) tool_calls: BTreeMap<String, u64>,
    pub(crate) touched_files: BTreeSet<String>,
    pub(crate) command_count: u64,
    pub(crate) patch_count: u64,
    pub(crate) error_count: u64,
    pub(crate) web_search_count: u64,
    pub(crate) compaction_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DialogueLine {
    pub(crate) side: String,
    pub(crate) text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TurnView {
    pub(crate) source: String,
    pub(crate) vendor: Vendor,
    pub(crate) turn_id: Option<String>,
    pub(crate) start: String,
    pub(crate) end: Option<String>,
    pub(crate) completed: bool,
    pub(crate) commands: u64,
    pub(crate) patches: u64,
    pub(crate) errors: u64,
    pub(crate) web_searches: u64,
    pub(crate) compactions: u64,
    pub(crate) touched_paths: Vec<String>,
    pub(crate) dialogue: Vec<DialogueLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompactionRecord {
    pub(crate) source: String,
    pub(crate) ts: String,
    pub(crate) assistant_before: String,
    pub(crate) summary_after: String,
    pub(crate) intent: String,
    pub(crate) user_after: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CompactionView {
    pub(crate) source: String,
    pub(crate) ts: String,
    pub(crate) assistant_before: String,
    pub(crate) summary_after: String,
    pub(crate) intent: String,
    pub(crate) user_after: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TokenRecord {
    pub(crate) source: String,
    pub(crate) ts: String,
    pub(crate) input: u64,
    pub(crate) cached: u64,
    pub(crate) output: u64,
    pub(crate) reasoning: u64,
    pub(crate) total: u64,
    pub(crate) model_context_window: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TokenView {
    pub(crate) source: String,
    pub(crate) total: u64,
    pub(crate) input: u64,
    pub(crate) cached: u64,
    pub(crate) uncached: u64,
    pub(crate) output: u64,
    pub(crate) reasoning: u64,
    pub(crate) model_context_window: Option<u64>,
    pub(crate) snapshots: u64,
    pub(crate) first_snapshot: Option<String>,
    pub(crate) last_snapshot: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandRecord {
    pub(crate) source: String,
    pub(crate) ts: String,
    pub(crate) turn_id: Option<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) command: String,
    pub(crate) exit_code: Option<i32>,
    pub(crate) status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CommandView {
    pub(crate) source: String,
    pub(crate) ts: String,
    pub(crate) turn_id: Option<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) command: String,
    pub(crate) exit_code: Option<i32>,
    pub(crate) status: String,
    pub(crate) wrapper: bool,
    pub(crate) pipeline: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CommandAudit {
    pub(crate) population: usize,
    pub(crate) matched: usize,
    pub(crate) filters: Vec<String>,
    pub(crate) failed: usize,
    pub(crate) wrapper: usize,
    pub(crate) non_wrapper: usize,
    pub(crate) pipelines: usize,
    pub(crate) canonical_pipelines: usize,
    pub(crate) noncanonical_pipelines: usize,
    pub(crate) top_noncanonical: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CommitView {
    pub(crate) source: String,
    pub(crate) ts: String,
    pub(crate) turn_id: Option<String>,
    pub(crate) sha: String,
    pub(crate) line: String,
    pub(crate) user: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TimelineView {
    pub(crate) source: String,
    pub(crate) ts: String,
    pub(crate) kind: String,
    pub(crate) turn_id: Option<String>,
    pub(crate) text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SliceView {
    pub(crate) id: String,
    pub(crate) source: String,
    pub(crate) start: String,
    pub(crate) end: String,
    pub(crate) basis: String,
    pub(crate) anchor: String,
    pub(crate) status: String,
    pub(crate) compactions: usize,
    pub(crate) turns: Vec<String>,
    pub(crate) patches: u64,
    pub(crate) crossing_files: Vec<String>,
    pub(crate) messages: Vec<DialogueLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PhaseExample {
    pub(crate) start: String,
    pub(crate) turn_id: Option<String>,
    pub(crate) archetype: String,
    pub(crate) path: Option<String>,
    pub(crate) user: Option<String>,
    pub(crate) final_answer: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PhaseView {
    pub(crate) index: usize,
    pub(crate) family: String,
    pub(crate) primary: String,
    pub(crate) turns: usize,
    pub(crate) commands: u64,
    pub(crate) patches: u64,
    pub(crate) compactions: u64,
    pub(crate) errors: u64,
    pub(crate) duration_millis: Option<i64>,
    pub(crate) start: String,
    pub(crate) end: String,
    pub(crate) top_paths: Vec<String>,
    pub(crate) examples: Vec<PhaseExample>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActivityView {
    pub(crate) commands: u64,
    pub(crate) patches: u64,
    pub(crate) errors: u64,
    pub(crate) web_searches: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecallWindow {
    pub(crate) index: usize,
    pub(crate) start: Option<String>,
    pub(crate) end: Option<String>,
    pub(crate) turns: Vec<TurnView>,
    pub(crate) compactions: Vec<CompactionView>,
    pub(crate) latest_final: Option<String>,
    pub(crate) activity: ActivityView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PruningView {
    pub(crate) max_lines: usize,
    pub(crate) max_bytes: usize,
    pub(crate) turns_before: usize,
    pub(crate) turns_after: usize,
    pub(crate) explanation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AuthorityFoldView {
    pub(crate) mode: String,
    pub(crate) probes: Vec<String>,
    pub(crate) writes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DecisionPriorityView {
    pub(crate) order: Vec<String>,
    pub(crate) end_pruning_preserves: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OutstandingAskView {
    pub(crate) lane: String,
    pub(crate) directive: String,
    pub(crate) key: String,
    pub(crate) stage: String,
    pub(crate) text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) attachments: Vec<wire::Media>,
    pub(crate) authored: String,
    pub(crate) expires: String,
    pub(crate) preview: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HeldClaimView {
    pub(crate) handle: String,
    pub(crate) project: String,
    pub(crate) phase: String,
    pub(crate) status: String,
    pub(crate) title: String,
    pub(crate) lease_expiry: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HeldWorkView {
    pub(crate) thread: Option<String>,
    pub(crate) lane: Option<String>,
    pub(crate) own_claim: Option<HeldClaimView>,
    pub(crate) peer_claims: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AuthorityUnknown {
    pub(crate) authority: String,
    pub(crate) probe: String,
    pub(crate) detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BriefingAuthorities {
    pub(crate) fold: AuthorityFoldView,
    pub(crate) priority: DecisionPriorityView,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) asks: Vec<OutstandingAskView>,
    pub(crate) held_work: HeldWorkView,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) unknowns: Vec<AuthorityUnknown>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ThreadView {
    pub(crate) id: String,
    pub(crate) vendor: Vendor,
    pub(crate) transcript: String,
    pub(crate) turns: usize,
    pub(crate) compactions: usize,
    pub(crate) start: Option<String>,
    pub(crate) end: Option<String>,
    pub(crate) latest_turn: Option<TurnView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "session", rename_all = "kebab-case", deny_unknown_fields)]
#[allow(
    clippy::large_enum_variant,
    reason = "boxing a complete typed CLI document would add a second representation at the serialization boundary"
)]
pub(crate) enum Report {
    Recall {
        sources: Vec<SourceView>,
        horizon_basis: String,
        requested_compactions: usize,
        selected_compactions: usize,
        authorities: Box<BriefingAuthorities>,
        window: RecallWindow,
        pruning: PruningView,
        residues: Vec<Residue>,
    },
    Sweep {
        sources: Vec<SourceView>,
        windows: Vec<RecallWindow>,
        residues: Vec<Residue>,
    },
    Summary {
        sources: Vec<SourceView>,
        turns: usize,
        completed: usize,
        compactions: usize,
        activity: ActivityView,
        recent_prompts: Vec<MessageView>,
        recent_finals: Vec<MessageView>,
        residues: Vec<Residue>,
    },
    Tokens {
        sources: Vec<SourceView>,
        usage: Vec<TokenView>,
        residues: Vec<Residue>,
    },
    Timeline {
        sources: Vec<SourceView>,
        rows: Vec<TimelineView>,
        residues: Vec<Residue>,
    },
    Turns {
        sources: Vec<SourceView>,
        rows: Vec<TurnView>,
        residues: Vec<Residue>,
    },
    Compactions {
        sources: Vec<SourceView>,
        rows: Vec<CompactionView>,
        residues: Vec<Residue>,
    },
    Slices {
        sources: Vec<SourceView>,
        rows: Vec<SliceView>,
        residues: Vec<Residue>,
    },
    Phases {
        sources: Vec<SourceView>,
        rows: Vec<PhaseView>,
        residues: Vec<Residue>,
    },
    Messages {
        sources: Vec<SourceView>,
        rows: Vec<MessageView>,
        residues: Vec<Residue>,
    },
    Commits {
        sources: Vec<SourceView>,
        rows: Vec<CommitView>,
        residues: Vec<Residue>,
    },
    Commands {
        sources: Vec<SourceView>,
        rows: Vec<CommandView>,
        audit: Option<CommandAudit>,
        residues: Vec<Residue>,
    },
    Thread {
        source: SourceView,
        thread: ThreadView,
        residues: Vec<Residue>,
    },
}

#[derive(Debug, Default)]
pub(crate) struct Dataset {
    pub(crate) sources: Vec<SourceView>,
    pub(crate) turns: Vec<TurnRecord>,
    pub(crate) compactions: Vec<CompactionRecord>,
    pub(crate) messages: Vec<MessageRecord>,
    pub(crate) commands: Vec<CommandRecord>,
    pub(crate) tokens: Vec<TokenRecord>,
    pub(crate) residues: Vec<Residue>,
}
