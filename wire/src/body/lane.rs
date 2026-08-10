//! Lane lifecycle fact bodies.

use super::*;

/// A lane and its capability were created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneSeated {
    /// Created independently steerable member seat.
    pub lane: CanonicalUuid,
    /// Canvas lineage whose lane contains the member.
    pub lineage_id: CanonicalUuid,
    /// Capability minted with the lane.
    pub capability: CanonicalUuid,
    /// Operator-configured agent display name, absent on pre-field facts.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub name: Option<String>,
    /// Worktree the lane is bound to.
    pub worktree: String,
    /// Closed lane-life spelling selected for the seat.
    pub life: String,
    /// Vendor selected for occupants of this seat.
    pub vendor: Name,
}

/// One Canvas lineage adopted a task-subscription policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineageConfigured {
    /// Stable Canvas lineage identifying the configured group.
    pub lineage_id: CanonicalUuid,
    /// Member moved into the configured lineage, absent for policy-only changes.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub member: Option<CanonicalUuid>,
    /// Lineage the moved member occupied before this transaction.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub from_lineage: Option<CanonicalUuid>,
    /// Replacement identity assigned to a source team when its host left.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub source_rekeyed_to: Option<CanonicalUuid>,
    /// Prior task-subscription policy before this transaction.
    pub from: crate::intent::LineageLife,
    /// Authoritative task-subscription policy after this transaction.
    pub life: crate::intent::LineageLife,
    /// Prior operator-pinned project set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub from_manual_projects: Vec<String>,
    /// Authoritative operator-pinned project set after this transaction.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub manual_projects: Vec<String>,
    /// Boundary for the moved member, when one moved and is actively claimed.
    ///
    /// Empty for a policy-only change, which moves no member and so crosses no
    /// lane's attribution.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<LaneSourceBoundary>,
}

/// One Canvas lineage and the lanes it holds after an arrangement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineageStanding {
    /// Stable Canvas lineage identifying the group.
    pub lineage_id: CanonicalUuid,
    /// Every lane this lineage holds, in the order the operator arranged them.
    pub lanes: Vec<CanonicalUuid>,
}

/// One lane's accounted transcript boundary at a membership transition.
///
/// A membership transition names the lanes it moves rather than carrying a
/// single lane of its own, so the boundary travels beside the lane it belongs
/// to. This is what lets a reducer split attribution across the move: the spend
/// before this locus belongs to the lineage the lane left, and the spend after
/// it to the lineage the lane joined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneSourceBoundary {
    /// Lane whose membership the transition changed.
    pub lane: CanonicalUuid,
    /// Accounted transcript that lane's membership changed at.
    pub source: SourceBoundary,
}

/// The operator rearranged the Canvas lineages.
///
/// Both sides are recorded because the gesture is not recoverable from the
/// result alone: a reorder and a join leave different `from` standings behind,
/// and a reader that only saw the outcome could not tell which one the
/// operator performed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineageArranged {
    /// Lineage standings before this transaction, in their prior order.
    pub from: Vec<LineageStanding>,
    /// Lineage standings after this transaction, in the arranged order.
    pub lineages: Vec<LineageStanding>,
    /// Boundaries for the actively claimed lanes this arrangement moved.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<LaneSourceBoundary>,
}

/// One seated member moved to a durable position, possibly in another lineage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneArranged {
    /// Stable member identity that moved.
    pub lane: CanonicalUuid,
    /// Lineage the member occupied before the move.
    pub from_lineage_id: CanonicalUuid,
    /// Lineage owned by the target member after the move.
    pub lineage_id: CanonicalUuid,
    /// Stable member identity immediately following the moved member.
    pub before: CanonicalUuid,
    /// Boundary for the moved lane, when it is actively claimed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<LaneSourceBoundary>,
}

/// Exact model, effort, and observable Git outcome for one vendor launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneLaunchIdentity {
    /// Concrete model supplied to the vendor launch.
    pub model: String,
    /// Reasoning effort supplied to the vendor launch.
    pub effort: String,
    /// GIT-04 outcome observed before this launch read repository configuration.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub git: Option<String>,
}

impl LaneLaunchIdentity {
    /// Build one complete recorded launch identity.
    pub fn new(model: impl Into<String>, effort: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            effort: effort.into(),
            git: None,
        }
    }

    /// Attach the observable GIT-04 launch outcome to a current launch.
    #[must_use]
    pub fn with_git(mut self, git: impl Into<String>) -> Self {
        self.git = Some(git.into());
        self
    }

    pub(crate) fn from_parts(
        model: Option<String>,
        effort: Option<String>,
        git: Option<String>,
    ) -> Result<Option<Self>, &'static str> {
        match (model, effort, git) {
            (Some(model), Some(effort), git) => Ok(Some(Self { model, effort, git })),
            (None, None, None) => Ok(None),
            (Some(_), None, _) => Err("lane launch identity is missing effort"),
            (None, Some(_), _) => Err("lane launch identity is missing model"),
            (None, None, Some(_)) => Err("lane launch identity is missing model and effort"),
        }
    }

    pub(crate) fn into_parts(
        identity: Option<Self>,
    ) -> (Option<String>, Option<String>, Option<String>) {
        identity.map_or((None, None, None), |identity| {
            (Some(identity.model), Some(identity.effort), identity.git)
        })
    }
}

/// A selected vendor record declared the exact resumable lane session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "LaneSessionDeclaredWire", into = "LaneSessionDeclaredWire")]
pub struct LaneSessionDeclared {
    /// Lane whose session was declared.
    pub lane: CanonicalUuid,
    /// Occupant whose selected stream emitted the declaration.
    pub occupant: CanonicalUuid,
    /// Canonical worktree bound to the launch.
    pub worktree: String,
    /// Vendor dialect that admitted the declaration.
    pub vendor: Name,
    /// Launch identity, absent when the declaration predates its recording.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub launch: Option<LaneLaunchIdentity>,
    /// Resident generation that observed the declaration.
    pub epoch: CanonicalUuid,
    /// Selected child stream carrying records.
    pub stream: String,
    /// Exact resumable vendor identity.
    pub identity: String,
    /// Durable tape member holding the declaration.
    pub member: u64,
    /// Declaration offset within the member.
    pub offset: u64,
    /// Number of declaration bytes.
    pub length: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaneSessionDeclaredWire {
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    worktree: String,
    vendor: Name,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    model: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    effort: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    git: Option<String>,
    epoch: CanonicalUuid,
    stream: String,
    identity: String,
    member: u64,
    offset: u64,
    length: u64,
}

impl TryFrom<LaneSessionDeclaredWire> for LaneSessionDeclared {
    type Error = &'static str;

    fn try_from(wire: LaneSessionDeclaredWire) -> Result<Self, Self::Error> {
        Ok(Self {
            lane: wire.lane,
            occupant: wire.occupant,
            worktree: wire.worktree,
            vendor: wire.vendor,
            launch: LaneLaunchIdentity::from_parts(wire.model, wire.effort, wire.git)?,
            epoch: wire.epoch,
            stream: wire.stream,
            identity: wire.identity,
            member: wire.member,
            offset: wire.offset,
            length: wire.length,
        })
    }
}

impl From<LaneSessionDeclared> for LaneSessionDeclaredWire {
    fn from(declaration: LaneSessionDeclared) -> Self {
        let (model, effort, git) = LaneLaunchIdentity::into_parts(declaration.launch);
        Self {
            lane: declaration.lane,
            occupant: declaration.occupant,
            worktree: declaration.worktree,
            vendor: declaration.vendor,
            model,
            effort,
            git,
            epoch: declaration.epoch,
            stream: declaration.stream,
            identity: declaration.identity,
            member: declaration.member,
            offset: declaration.offset,
            length: declaration.length,
        }
    }
}

/// An occupant became live under the lane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneStarted {
    /// Occupied lane.
    pub lane: CanonicalUuid,
    /// Occupant that became live.
    pub occupant: CanonicalUuid,
    /// Resident epoch that observed the start.
    pub epoch: CanonicalUuid,
    /// Process-group identity returned by the sole process door.
    pub group: u32,
}

/// A vacant lane seat moved to a fresh linked worktree before exact-session seating.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneImported {
    /// Lane whose lineage seat is retained.
    pub lane: CanonicalUuid,
    /// Latest occupant in the retained lineage.
    pub ancestor: CanonicalUuid,
    /// Worktree the seat was authoritatively bound to.
    pub from_worktree: String,
    /// Fresh linked worktree receiving the seat.
    pub worktree: String,
    /// Driver dialect retained by the transfer.
    pub vendor: Name,
    /// Exact resumable identity proved before the transfer.
    pub identity: String,
    /// Resident generation that admitted the transfer.
    pub epoch: CanonicalUuid,
}

/// The resident observed the occupant end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneExited {
    /// Vacated lane.
    pub lane: CanonicalUuid,
    /// Occupant that ended.
    pub occupant: CanonicalUuid,
    /// Exit status the resident observed.
    pub code: i32,
}

/// The occupant crossed a context compaction boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneCompacted {
    /// Lane the occupant runs under.
    pub lane: CanonicalUuid,
    /// Occupant that compacted.
    pub occupant: CanonicalUuid,
    /// Selected child stream carrying the boundary record.
    pub stream: String,
    /// Durable tape member holding the boundary record.
    pub member: u64,
    /// Boundary-record offset within the member.
    pub offset: u64,
    /// Number of bytes in the boundary record.
    pub length: u64,
}

/// A successor occupant was seated with its ancestor named.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneRenewed {
    /// Reseated lane.
    pub lane: CanonicalUuid,
    /// Successor occupant.
    pub occupant: CanonicalUuid,
    /// Occupant the successor descends from.
    pub ancestor: CanonicalUuid,
    /// Resident epoch that observed the successor start.
    pub epoch: CanonicalUuid,
    /// Process-group identity returned by the sole process door.
    pub group: u32,
}

/// A graceful occupant end completed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneDrained {
    /// Drained lane.
    pub lane: CanonicalUuid,
    /// Occupant that ended gracefully.
    pub occupant: CanonicalUuid,
}

/// A signal this resident sent ended an occupant.
///
/// Separate from [`LaneExited`] because being signalled is a different ending,
/// not an exit status the occupant chose. A signalled process has no status of
/// its own to report, so recording one would mean publishing the resident's
/// substitute for a missing number as though the occupant had returned it. This
/// kind carries no code for the same reason.
///
/// Both signalling stages record this, not only the kill. An occupant that never
/// trapped the termination signal dies from that signal rather than outliving
/// the grace, and this resident is the reason for the ending either way. What
/// this kind separates is whose decision the ending was, not which signal
/// carried it; the stage is already durable in the shutdown ladder facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneKilled {
    /// Vacated lane.
    pub lane: CanonicalUuid,
    /// Occupant the resident ended.
    pub occupant: CanonicalUuid,
}

/// Work run without the resident left a marker that the next resident recognized once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneDegraded {
    /// Lane the daemonless work ran in.
    pub lane: CanonicalUuid,
    /// Marker the next resident recognized.
    pub marker: String,
}

/// Repeated young occupant deaths paused automatic re-seating.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneHeld {
    /// Held lane.
    pub lane: CanonicalUuid,
    /// Evidence the hold names.
    pub evidence: String,
    /// Deadline the hold names.
    pub until: Instant,
}

/// A held lane returned to seating eligibility by operator wake or expired hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneResumed {
    /// Resumed lane.
    pub lane: CanonicalUuid,
    /// What released the hold.
    pub release: Release,
}
