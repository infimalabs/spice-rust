//! Current-process working state without wrapper-owned policy.
//!
//! A command wrapper may collect live facts and hand them here, but this module
//! owns how those facts become one working-state announcement. The boundary is
//! deliberately narrow: process identity comes only from values captured from
//! the current environment, clocks arrive as arguments, and a collection or
//! persistence failure can be turned into silence without changing the command
//! being decorated.

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

#[cfg(not(target_arch = "wasm32"))]
mod live;
#[cfg(not(target_arch = "wasm32"))]
pub use live::LiveMeter;

/// Remaining lease below this duration enters the warning tier.
pub const CLAIM_WARNING_BOUNDARY: Duration = Duration::from_secs(10 * 60);
/// Remaining lease below this duration enters the urgent tier.
pub const CLAIM_URGENT_BOUNDARY: Duration = Duration::from_secs(5 * 60);
/// Remaining lease below this duration enters the critical tier.
pub const CLAIM_CRITICAL_BOUNDARY: Duration = Duration::from_secs(60);
/// Warning-tier reminders repeat at this cadence.
pub const CLAIM_WARNING_REPEAT: Duration = Duration::from_secs(2 * 60);
/// Urgent-tier reminders repeat at this cadence.
pub const CLAIM_URGENT_REPEAT: Duration = Duration::from_secs(60);
/// Critical-tier reminders repeat at this cadence.
pub const CLAIM_CRITICAL_REPEAT: Duration = Duration::from_secs(15);
/// No scheduled refresh may collapse to a zero-delay loop.
pub const MINIMUM_REFRESH: Duration = Duration::from_millis(50);

/// One actor identity proven by the running process environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrentActor<'a> {
    /// Declared vendor whose environment variable carried the identity.
    pub vendor: &'static crate::extension::Vendor,
    /// Non-empty vendor thread or session identity.
    pub identity: &'a str,
}

/// Vendor identities captured at the binary rim from this process only.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProcessEnvironment<'a> {
    /// Vendor values paired with the ambient values captured for their
    /// vendor-owned identity variables.
    pub identities: &'a [(&'static crate::extension::Vendor, Option<&'a str>)],
}

impl<'a> ProcessEnvironment<'a> {
    /// Return the one unambiguous actor the current process proves.
    ///
    /// An absent identity and two simultaneous vendor identities both return
    /// `None`; persisted tree state is intentionally not an input.
    #[must_use]
    pub fn current_actor(self) -> Option<CurrentActor<'a>> {
        let mut current = self.identities.iter().filter_map(|(vendor, value)| {
            nonempty(*value).map(|identity| CurrentActor { vendor, identity })
        });
        let selected = current.next()?;
        current.next().is_none().then_some(selected)
    }
}

/// Live claim fields included in one working-state snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClaimState {
    /// Public task handle.
    pub handle: String,
    /// Current task phase.
    pub phase: String,
    /// Time since this claim began, when its timestamp was valid.
    pub elapsed: Option<Duration>,
    /// Time until this claim expires, when its timestamp was valid.
    pub remaining: Option<Duration>,
    /// Whether the default task reclaim can extend expiry in place.
    pub reclaim_extends: bool,
}

/// One active-claim observation read from the command boundary's live board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimObservation {
    /// Declared vendor whose live actor owns the claim.
    pub vendor: &'static crate::extension::Vendor,
    /// Claim holder identity.
    pub actor: String,
    /// Fields eligible for the working-state line.
    pub state: ClaimState,
}

/// Live facts a command-boundary collector observed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkingState {
    /// Pending steering messages for this lane.
    pub pending_inbox_count: usize,
    /// Whether this lane continuously drains allocator-selected work.
    pub drain_queue: bool,
    /// Current process actor's claim, if one is active in this worktree.
    pub claim: Option<ClaimState>,
    /// Most recently fired taste bag.
    pub last_taste: String,
}

impl WorkingState {
    /// Whether this snapshot contains any signal worth announcing.
    #[must_use]
    pub fn has_signal(&self) -> bool {
        self.pending_inbox_count > 0
            || (self.drain_queue && self.claim.is_none())
            || self
                .claim
                .as_ref()
                .is_some_and(|claim| !clean(&claim.handle).is_empty())
            || !clean(&self.last_taste).is_empty()
    }
}

/// Select one snapshot for the actor proven by the current process.
///
/// Persisted identities are not accepted by this boundary. An absent or
/// ambiguous environment, or duplicate live claims for one actor, yields no
/// claim while retaining the independently observed inbox and taste signals.
#[must_use]
pub fn collect_working_state(
    environment: ProcessEnvironment<'_>,
    pending_inbox_count: usize,
    claims: &[ClaimObservation],
    last_taste: impl Into<String>,
) -> WorkingState {
    let claim = environment.current_actor().and_then(|current| {
        let mut matching = claims.iter().filter(|observation| {
            observation.vendor == current.vendor && observation.actor == current.identity
        });
        let selected = matching.next()?;
        matching.next().is_none().then(|| selected.state.clone())
    });
    WorkingState {
        pending_inbox_count,
        drain_queue: false,
        claim,
        last_taste: last_taste.into(),
    }
}

/// One meter evaluation at a command or timer boundary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Decision {
    /// One short stderr line when state changed or a reminder became due.
    pub announcement: Option<String>,
    /// Bound for the next expiry transition or reminder, if one exists.
    pub refresh_after: Option<Duration>,
}

/// The decorated command outcome and its optional meter decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decorated<O> {
    /// Command outcome, independent of meter success or failure.
    pub outcome: O,
    /// Meter output; empty when collection or persistence failed.
    pub meter: Decision,
}

/// Durable change-announcement reducer for one command surface.
#[derive(Debug, Clone)]
pub struct Meter {
    state_path: PathBuf,
}

impl Meter {
    /// Bind the reducer to its durable last-announcement record.
    #[must_use]
    pub fn new(state_path: impl Into<PathBuf>) -> Self {
        Self {
            state_path: state_path.into(),
        }
    }

    /// Reduce one snapshot at the supplied clock instant.
    ///
    /// Missing or malformed prior state means the meter speaks. Read and write
    /// failures are returned so a decorating caller can degrade them to silence.
    ///
    /// # Errors
    ///
    /// Returns [`MeterError`] when the state path cannot be read or atomically
    /// replaced, or the new record cannot be encoded.
    pub fn evaluate(&self, snapshot: &WorkingState, now: Duration) -> Result<Decision, MeterError> {
        if !snapshot.has_signal() {
            return Ok(Decision::default());
        }
        let key = StateKey::from(snapshot);
        let stored = self.read_state()?;
        let matching_age = stored.as_ref().and_then(|stored| {
            (stored.key == key && now >= stored.displayed_at)
                .then(|| now.saturating_sub(stored.displayed_at))
        });
        let repeat = key.tier.repeat();
        let suppressed = matching_age.is_some_and(|age| repeat.is_none_or(|bound| age < bound));
        let announcement = if suppressed {
            None
        } else {
            let line = render(snapshot);
            self.write_state(&StoredState {
                displayed_at: now,
                key: key.clone(),
            })?;
            Some(line)
        };
        let repeat_remaining = repeat.map(|bound| {
            if announcement.is_some() {
                bound
            } else {
                bound.saturating_sub(matching_age.unwrap_or_default())
            }
        });
        Ok(Decision {
            announcement,
            refresh_after: next_refresh(snapshot, key.tier, repeat_remaining),
        })
    }

    fn read_state(&self) -> Result<Option<StoredState>, MeterError> {
        let bytes = match fs::read(&self.state_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(MeterError::Read(error)),
        };
        Ok(serde_json::from_slice(&bytes).ok())
    }

    fn write_state(&self, state: &StoredState) -> Result<(), MeterError> {
        let bytes = serde_json::to_vec(state).map_err(MeterError::Encode)?;
        let destination =
            atomic::Destination::new(&self.state_path).map_err(MeterError::Publish)?;
        destination
            .publish(|staged| staged.write_all(&bytes))
            .map_err(MeterError::Publish)
    }
}

/// Decorate a command while making all meter failures non-authoritative.
///
/// Collection and meter persistence run before the command, matching the shell
/// boundary. The command still runs exactly once and its outcome is returned
/// unchanged when either operation fails.
pub fn decorate<O, E>(
    meter: &Meter,
    now: Duration,
    collect: impl FnOnce() -> Result<WorkingState, E>,
    command: impl FnOnce() -> O,
) -> Decorated<O> {
    let decision = collect()
        .ok()
        .and_then(|snapshot| meter.evaluate(&snapshot, now).ok())
        .unwrap_or_default();
    Decorated {
        outcome: command(),
        meter: decision,
    }
}

/// One admitted context-usage position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextSnapshot {
    /// Canonical time used for ordered reporting.
    pub at: wire::Instant,
    /// Provider context limit, remaining unknown when the provider omitted it.
    pub model_context_window: Option<u64>,
}

impl ContextSnapshot {
    /// Admit only a normalizable timestamp without inventing a context limit.
    #[must_use]
    pub fn admit(raw_timestamp: &str, model_context_window: Option<u64>) -> Option<Self> {
        Some(Self {
            at: normalize_timestamp(raw_timestamp)?,
            model_context_window,
        })
    }
}

/// Normalize a provider timestamp into the one wire spelling.
///
/// Invalid or out-of-range values return `None`; callers drop the record rather
/// than substituting an ordering key.
#[must_use]
pub fn normalize_timestamp(raw: &str) -> Option<wire::Instant> {
    let timestamp = raw.parse::<jiff::Timestamp>().ok()?;
    wire::Instant::new(timestamp).ok()
}

/// Failure to access the durable announcement record.
#[derive(Debug)]
pub enum MeterError {
    /// Existing state could not be read.
    Read(io::Error),
    /// The new state could not be encoded.
    Encode(serde_json::Error),
    /// The new state could not be atomically published.
    Publish(atomic::Error),
}

impl fmt::Display for MeterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => write!(formatter, "working-state record unreadable: {error}"),
            Self::Encode(error) => write!(formatter, "working-state record unencodable: {error}"),
            Self::Publish(error) => write!(formatter, "working-state record unpublished: {error}"),
        }
    }
}

impl std::error::Error for MeterError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read(error) => Some(error),
            Self::Encode(error) => Some(error),
            Self::Publish(error) => Some(error),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum LeaseTier {
    None,
    Warning,
    Urgent,
    Critical,
}

impl LeaseTier {
    fn from_remaining(remaining: Option<Duration>) -> Self {
        match remaining {
            Some(value) if value < CLAIM_CRITICAL_BOUNDARY => Self::Critical,
            Some(value) if value < CLAIM_URGENT_BOUNDARY => Self::Urgent,
            Some(value) if value < CLAIM_WARNING_BOUNDARY => Self::Warning,
            None | Some(_) => Self::None,
        }
    }

    const fn repeat(self) -> Option<Duration> {
        match self {
            Self::None => None,
            Self::Warning => Some(CLAIM_WARNING_REPEAT),
            Self::Urgent => Some(CLAIM_URGENT_REPEAT),
            Self::Critical => Some(CLAIM_CRITICAL_REPEAT),
        }
    }

    const fn label(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Warning => Some("WARNING"),
            Self::Urgent => Some("URGENT"),
            Self::Critical => Some("CRITICAL"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StateKey {
    pending_inbox_count: u64,
    drain_queue: bool,
    claim_handle: String,
    claim_phase: String,
    last_taste: String,
    tier: LeaseTier,
    reclaim_extends: bool,
}

impl From<&WorkingState> for StateKey {
    fn from(snapshot: &WorkingState) -> Self {
        let claim = snapshot.claim.as_ref();
        Self {
            pending_inbox_count: u64::try_from(snapshot.pending_inbox_count).unwrap_or(u64::MAX),
            drain_queue: snapshot.drain_queue,
            claim_handle: claim.map_or_else(String::new, |claim| clean(&claim.handle)),
            claim_phase: claim.map_or_else(String::new, |claim| clean(&claim.phase)),
            last_taste: clean(&snapshot.last_taste),
            tier: LeaseTier::from_remaining(claim.and_then(|claim| claim.remaining)),
            reclaim_extends: claim.is_some_and(|claim| claim.reclaim_extends),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredState {
    #[serde(with = "duration_millis")]
    displayed_at: Duration,
    key: StateKey,
}

fn render(snapshot: &WorkingState) -> String {
    let mut parts = Vec::new();
    if snapshot.pending_inbox_count > 0 {
        let noun = if snapshot.pending_inbox_count == 1 {
            "inbox"
        } else {
            "inboxes"
        };
        parts.push(format!("{} pending {noun}", snapshot.pending_inbox_count));
    }
    if snapshot.drain_queue && snapshot.claim.is_none() {
        let direction = if snapshot.pending_inbox_count > 0 {
            "handle operator direction first, then DRAIN QUEUE ASAP: spice task next"
        } else {
            "DRAIN QUEUE ASAP: spice task next"
        };
        parts.push(direction.to_owned());
    }
    if let Some(claim) = snapshot.claim.as_ref() {
        let handle = clean(&claim.handle);
        if !handle.is_empty() {
            let mut text = format!("claim {handle}");
            let phase = clean(&claim.phase);
            if !phase.is_empty() {
                text.push(' ');
                text.push_str(&phase);
            }
            if let Some(elapsed) = claim.elapsed {
                text.push_str(&format!(" held for {}s", elapsed.as_secs()));
            }
            parts.push(text);
            let tier = LeaseTier::from_remaining(claim.remaining);
            if let (Some(label), Some(remaining)) = (tier.label(), claim.remaining) {
                let recovery = if claim.reclaim_extends {
                    format!("run spice task reclaim {handle}")
                } else {
                    format!("run spice task free {handle}, then spice task next")
                };
                parts.push(format!(
                    "CLAIM LEASE {label}: {handle} has {}s remaining; {recovery}",
                    remaining.as_secs(),
                ));
            }
        }
    }
    let taste = clean(&snapshot.last_taste);
    if !taste.is_empty() {
        parts.push(format!("last taste {taste}"));
    }
    format!("🌶️ Working state: {}.", parts.join("; "))
}

fn next_refresh(
    snapshot: &WorkingState,
    tier: LeaseTier,
    repeat_remaining: Option<Duration>,
) -> Option<Duration> {
    let remaining = snapshot.claim.as_ref().and_then(|claim| claim.remaining);
    let boundary = remaining.and_then(|remaining| {
        let target = match tier {
            LeaseTier::None => CLAIM_WARNING_BOUNDARY.saturating_sub(Duration::from_secs(1)),
            LeaseTier::Warning => CLAIM_URGENT_BOUNDARY.saturating_sub(Duration::from_secs(1)),
            LeaseTier::Urgent => CLAIM_CRITICAL_BOUNDARY.saturating_sub(Duration::from_secs(1)),
            LeaseTier::Critical => return None,
        };
        Some(remaining.saturating_sub(target))
    });
    [boundary, repeat_remaining]
        .into_iter()
        .flatten()
        .min()
        .map(|delay| delay.max(MINIMUM_REFRESH))
}

fn clean(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

mod duration_millis {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S>(value: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let millis = u64::try_from(value.as_millis()).unwrap_or(u64::MAX);
        serializer.serialize_u64(millis)
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Duration::from_millis(u64::deserialize(deserializer)?))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
    const WORKING_STATE_LINE_LIMIT: usize = 160;

    fn fixture(name: &str) -> PathBuf {
        let sequence = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "spice-session-meter-{name}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("fixture directory");
        path
    }

    fn state(remaining: Option<u64>) -> WorkingState {
        WorkingState {
            pending_inbox_count: 1,
            drain_queue: false,
            claim: Some(ClaimState {
                handle: "METER-00000001".to_owned(),
                phase: "todo".to_owned(),
                elapsed: Some(Duration::from_secs(90)),
                remaining: remaining.map(Duration::from_secs),
                reclaim_extends: true,
            }),
            last_taste: "fallbacks".to_owned(),
        }
    }

    #[test]
    fn short_working_state_line_contains_only_live_signal() {
        let root = fixture("render");
        let meter = Meter::new(root.join("working-state.json"));
        let decision = meter
            .evaluate(&state(None), Duration::from_secs(1))
            .expect("meter evaluates");
        let line = decision.announcement.expect("initial state speaks");

        assert_eq!(
            line,
            "🌶️ Working state: 1 pending inbox; claim METER-00000001 todo held for 90s; \
             last taste fallbacks."
        );
        assert!(!line.contains('\n'));
        assert_eq!(line.matches('.').count(), 1);
        assert!(line.len() < WORKING_STATE_LINE_LIMIT);
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn idle_drain_guidance_orders_operator_direction_and_stops_while_claimed() {
        let root = fixture("drain-guidance");
        let path = root.join("working-state.json");
        let idle = WorkingState {
            drain_queue: true,
            ..WorkingState::default()
        };
        let idle_line = Meter::new(&path)
            .evaluate(&idle, Duration::from_secs(1))
            .expect("idle drain state evaluates")
            .announcement
            .expect("idle drain state speaks");
        let directed = WorkingState {
            pending_inbox_count: 1,
            ..idle.clone()
        };
        let directed_line = Meter::new(&path)
            .evaluate(&directed, Duration::from_secs(2))
            .expect("directed drain state evaluates")
            .announcement
            .expect("new operator direction speaks");
        let claimed = WorkingState {
            claim: Some(ClaimState {
                handle: "DRAIN-00000001".to_owned(),
                phase: "todo".to_owned(),
                ..ClaimState::default()
            }),
            ..idle
        };
        let claimed_line = Meter::new(&path)
            .evaluate(&claimed, Duration::from_secs(3))
            .expect("claimed drain state evaluates")
            .announcement
            .expect("claim acquisition speaks");

        assert!(idle_line.contains("DRAIN QUEUE ASAP: spice task next"));
        assert_eq!(
            directed_line,
            "🌶️ Working state: 1 pending inbox; handle operator direction first, then \
             DRAIN QUEUE ASAP: spice task next."
        );
        assert!(!claimed_line.contains("spice task next"));
        for line in [&idle_line, &directed_line, &claimed_line] {
            assert!(line.len() < WORKING_STATE_LINE_LIMIT);
        }
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn only_the_running_process_environment_names_the_current_actor() {
        let claims = [
            ClaimObservation {
                vendor: &crate::extension::CODEX,
                actor: "stale-tree-thread".to_owned(),
                state: ClaimState {
                    handle: "STALE-00000001".to_owned(),
                    ..ClaimState::default()
                },
            },
            ClaimObservation {
                vendor: &crate::extension::CODEX,
                actor: "live-codex-thread".to_owned(),
                state: ClaimState {
                    handle: "LIVE-00000001".to_owned(),
                    ..ClaimState::default()
                },
            },
        ];
        let absent =
            collect_working_state(ProcessEnvironment::default(), 0, &claims, String::new());
        let current_identities = [(&crate::extension::CODEX, Some("live-codex-thread"))];
        let current = collect_working_state(
            ProcessEnvironment {
                identities: &current_identities,
            },
            0,
            &claims,
            String::new(),
        );

        assert_eq!(
            absent.claim, None,
            "a tree identity without a current environment is not current"
        );
        assert_eq!(
            current.claim.as_ref().map(|claim| claim.handle.as_str()),
            Some("LIVE-00000001")
        );
        assert_eq!(
            ProcessEnvironment {
                identities: &[
                    (&crate::extension::CODEX, Some("codex")),
                    (&crate::extension::CLAUDE, Some("claude")),
                ],
            }
            .current_actor(),
            None,
            "two ambient identities are ambiguous rather than guessed"
        );
    }

    #[test]
    fn announcement_identity_survives_exit_and_includes_expiry_tier() {
        let root = fixture("durable");
        let path = root.join("working-state.json");
        let first = Meter::new(&path)
            .evaluate(&state(Some(599)), Duration::from_secs(1))
            .expect("first process speaks");
        let next_process = Meter::new(&path)
            .evaluate(&state(Some(598)), Duration::from_secs(2))
            .expect("next process reads durable state");
        let urgent = Meter::new(&path)
            .evaluate(&state(Some(299)), Duration::from_secs(3))
            .expect("tier transition evaluates");

        assert!(first.announcement.is_some());
        assert_eq!(next_process.announcement, None);
        assert!(
            urgent
                .announcement
                .as_deref()
                .is_some_and(|line| line.contains("CLAIM LEASE URGENT"))
        );

        fs::write(&path, b"not-json").expect("corrupt prior announcement");
        let recovered = Meter::new(&path)
            .evaluate(&state(Some(299)), Duration::from_secs(4))
            .expect("malformed state is absence");
        assert!(recovered.announcement.is_some());
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn expiry_refresh_cadence_shortens_without_zero_delay() {
        let root = fixture("cadence");
        let cases = [
            (601, Duration::from_secs(2)),
            (599, CLAIM_WARNING_REPEAT),
            (299, CLAIM_URGENT_REPEAT),
            (59, CLAIM_CRITICAL_REPEAT),
            (0, CLAIM_CRITICAL_REPEAT),
        ];
        for (index, (remaining, expected)) in cases.into_iter().enumerate() {
            let meter = Meter::new(root.join(format!("state-{index}.json")));
            let decision = meter
                .evaluate(&state(Some(remaining)), Duration::from_secs(1))
                .expect("tier evaluates");
            assert_eq!(decision.refresh_after, Some(expected));
            assert!(decision.refresh_after.is_some_and(|delay| !delay.is_zero()));
        }
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn lease_tiers_are_strict_and_renewal_transitions_speak_immediately() {
        let root = fixture("strict-tiers");
        let cases = [
            (600, None),
            (599, Some("CLAIM LEASE WARNING")),
            (299, Some("CLAIM LEASE URGENT")),
            (59, Some("CLAIM LEASE CRITICAL")),
        ];
        for (index, (remaining, warning)) in cases.into_iter().enumerate() {
            let decision = Meter::new(root.join(format!("boundary-{index}.json")))
                .evaluate(&state(Some(remaining)), Duration::from_secs(1))
                .expect("strict boundary evaluates");
            let line = decision.announcement.expect("new state speaks once");
            assert_eq!(
                ["WARNING", "URGENT", "CRITICAL"]
                    .into_iter()
                    .find(|label| line.contains(&format!("CLAIM LEASE {label}"))),
                warning.map(|warning| warning.rsplit_once(' ').expect("label").1),
            );
            if warning.is_some() {
                assert!(line.contains("todo held for 90s"));
                assert!(!line.contains("todo for 90s"));
                assert!(line.contains(&format!("has {remaining}s remaining")));
                assert!(line.contains("run spice task reclaim METER-00000001"));
                assert!(!line.contains(" ACK"));
                assert!(!line.contains(" NACK"));
            }
        }

        let path = root.join("renewals.json");
        let meter = Meter::new(&path);
        let critical = meter
            .evaluate(&state(Some(59)), Duration::from_secs(10))
            .expect("critical evaluates");
        let same_tier = meter
            .evaluate(&state(Some(58)), Duration::from_secs(11))
            .expect("same tier renewal evaluates");
        let urgent = meter
            .evaluate(&state(Some(299)), Duration::from_secs(12))
            .expect("critical to urgent evaluates");
        let warning = meter
            .evaluate(&state(Some(599)), Duration::from_secs(13))
            .expect("urgent to warning evaluates");
        let quiet = meter
            .evaluate(&state(Some(600)), Duration::from_secs(14))
            .expect("warning exit evaluates");
        let renewed = meter
            .evaluate(&state(Some(700)), Duration::from_secs(15))
            .expect("renewed lease evaluates");

        assert!(critical.announcement.is_some());
        assert_eq!(same_tier.announcement, None);
        assert!(
            urgent
                .announcement
                .as_deref()
                .is_some_and(|line| { line.contains("CLAIM LEASE URGENT") })
        );
        assert!(
            warning
                .announcement
                .as_deref()
                .is_some_and(|line| { line.contains("CLAIM LEASE WARNING") })
        );
        assert!(
            quiet
                .announcement
                .as_deref()
                .is_some_and(|line| !line.contains("CLAIM LEASE"))
        );
        assert_eq!(renewed.announcement, None);
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn maximum_bound_claim_names_free_and_next_instead_of_reclaim() {
        let root = fixture("maximum-recovery");
        let path = root.join("working-state.json");
        let meter = Meter::new(&path);
        let remaining = (CLAIM_CRITICAL_BOUNDARY + CLAIM_CRITICAL_REPEAT).as_secs();
        let extendable = meter
            .evaluate(&state(Some(remaining)), Duration::ZERO)
            .expect("extendable warning evaluates")
            .announcement
            .expect("extendable warning speaks");
        let mut maximum_bound = state(Some(remaining));
        maximum_bound
            .claim
            .as_mut()
            .expect("fixture claim")
            .reclaim_extends = false;

        let line = meter
            .evaluate(&maximum_bound, Duration::ZERO)
            .expect("maximum-bound state evaluates")
            .announcement
            .expect("maximum transition speaks in the same lease tier");

        assert!(extendable.contains("run spice task reclaim METER-00000001"));
        assert!(line.contains("run spice task free METER-00000001, then spice task next"));
        assert!(!line.contains("task reclaim"));
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn no_claim_keeps_durable_suppression_for_same_key_reacquisition() {
        let root = fixture("reacquisition");
        let path = root.join("working-state.json");
        let meter = Meter::new(&path);
        let first = meter
            .evaluate(&state(Some(599)), Duration::from_secs(10))
            .expect("warning speaks");
        let stored = fs::read(&path).expect("suppression state reads");
        let absent = meter
            .evaluate(&WorkingState::default(), Duration::from_secs(11))
            .expect("no claim is silent");
        let reacquired = Meter::new(&path)
            .evaluate(&state(Some(598)), Duration::from_secs(12))
            .expect("fresh process honors prior cadence");

        assert!(first.announcement.is_some());
        assert_eq!(absent, Decision::default());
        assert_eq!(fs::read(&path).expect("state remains"), stored);
        assert_eq!(reacquired.announcement, None);
        assert_eq!(
            reacquired.refresh_after,
            Some(CLAIM_WARNING_REPEAT - Duration::from_secs(2))
        );
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn collection_failure_is_silent_and_cannot_change_the_command_outcome() {
        let root = fixture("failure");
        let meter = Meter::new(root.join("working-state.json"));
        let decorated = decorate(
            &meter,
            Duration::from_secs(1),
            || Err::<WorkingState, _>("board unavailable"),
            || 73,
        );

        assert_eq!(decorated.outcome, 73);
        assert_eq!(decorated.meter, Decision::default());
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn invalid_usage_time_is_omitted_and_unknown_context_stays_unknown() {
        assert_eq!(
            ContextSnapshot::admit("not-a-timestamp", Some(200_000)),
            None
        );
        let snapshot =
            ContextSnapshot::admit("2026-08-08T16:00:00Z", None).expect("valid usage timestamp");

        assert_eq!(snapshot.at.to_string(), "2026-08-08T16:00:00.000Z");
        assert_eq!(snapshot.model_context_window, None);
    }
}
