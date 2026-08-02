const COMPLETE_HEADROOM_REDUCTION_PERCENT: i128 = -100;
const BINARY64_FRACTION_BITS: i32 = 52;
const PERCENT_DENOMINATOR: u128 = 100;

/// Candidate provenance used by elastic headroom and peer claims.
///
/// Jitter uses the first present value in this exact order: published task
/// session, published commit, active actor, candidate blob, candidate path.
/// Operations requiring a real actor admit only the actor-bearing members of
/// the same order: published task session, then active actor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ActorInputs {
    #[serde(skip_serializing_if = "Option::is_none")]
    published_task_session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    published_commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    active_actor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    candidate_blob: Option<String>,
    candidate_path: String,
}

impl ActorInputs {
    /// Start provenance for one canonical repository-relative path.
    pub fn new(candidate_path: impl AsRef<str>) -> Result<Self, BoundError> {
        Ok(Self {
            published_task_session: None,
            published_commit: None,
            active_actor: None,
            candidate_blob: None,
            candidate_path: normalize_repo_path(candidate_path.as_ref()).map_err(|reason| {
                BoundError::InvalidPath {
                    path: candidate_path.as_ref().to_owned(),
                    reason,
                }
            })?,
        })
    }

    /// Record the published `Task-Session` trailer actor when present.
    #[must_use]
    pub fn with_published_task_session(mut self, value: impl Into<String>) -> Self {
        self.published_task_session = non_empty(value);
        self
    }

    /// Record the commit carrying matching published content when present.
    #[must_use]
    pub fn with_published_commit(mut self, value: impl Into<String>) -> Self {
        self.published_commit = non_empty(value);
        self
    }

    /// Record the active worktree actor when present.
    #[must_use]
    pub fn with_active_actor(mut self, value: impl Into<String>) -> Self {
        self.active_actor = non_empty(value);
        self
    }

    /// Record the candidate content identity when present.
    #[must_use]
    pub fn with_candidate_blob(mut self, value: impl Into<String>) -> Self {
        self.candidate_blob = non_empty(value);
        self
    }

    /// Canonical candidate path.
    #[must_use]
    pub fn candidate_path(&self) -> &str {
        &self.candidate_path
    }

    /// Resolve the deterministic jitter seed through the documented order.
    #[must_use]
    pub fn resolve_seed(&self) -> ActorResolution {
        if let Some(value) = &self.published_task_session {
            return ActorResolution::new(ActorSource::PublishedTaskSession, value);
        }
        if let Some(value) = &self.published_commit {
            return ActorResolution::new(ActorSource::PublishedCommit, value);
        }
        if let Some(value) = &self.active_actor {
            return ActorResolution::new(ActorSource::ActiveActor, value);
        }
        if let Some(value) = &self.candidate_blob {
            return ActorResolution::new(ActorSource::CandidateBlob, value);
        }
        ActorResolution::new(ActorSource::CandidatePath, &self.candidate_path)
    }

    /// Resolve a human/session actor, refusing provenance-only fallbacks.
    pub fn require_actor(&self) -> Result<ActorResolution, BoundError> {
        if let Some(value) = &self.published_task_session {
            return Ok(ActorResolution::new(
                ActorSource::PublishedTaskSession,
                value,
            ));
        }
        if let Some(value) = &self.active_actor {
            return Ok(ActorResolution::new(ActorSource::ActiveActor, value));
        }
        Err(BoundError::MissingActor)
    }
}

fn non_empty(value: impl Into<String>) -> Option<String> {
    let value = value.into();
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// Which provenance input supplied a selected actor or jitter seed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActorSource {
    PublishedTaskSession,
    PublishedCommit,
    ActiveActor,
    CandidateBlob,
    CandidatePath,
}

/// One selected provenance value and its source.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ActorResolution {
    source: ActorSource,
    value: String,
}

impl ActorResolution {
    fn new(source: ActorSource, value: &str) -> Self {
        Self {
            source,
            value: value.to_owned(),
        }
    }

    /// Input class that supplied this value.
    #[must_use]
    pub const fn source(&self) -> ActorSource {
        self.source
    }

    /// Selected actor or deterministic provenance seed.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// One typed elastic bound before a measured value is considered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundRule {
    rule: String,
    metric: String,
    base_limit: u64,
    elastic_limit: u64,
    unlimited: bool,
    actor_required: bool,
}

impl BoundRule {
    /// Validate a complete rule and its configured limits.
    pub fn new(
        rule: impl Into<String>,
        metric: impl Into<String>,
        base_limit: u64,
        elastic_limit: u64,
        unlimited: bool,
        actor_required: bool,
    ) -> Result<Self, BoundError> {
        let rule = rule.into();
        let metric = metric.into();
        if rule.trim().is_empty() {
            return Err(BoundError::EmptyRule);
        }
        if metric.trim().is_empty() {
            return Err(BoundError::EmptyMetric);
        }
        if base_limit == 0 {
            return Err(BoundError::ZeroBaseLimit);
        }
        if elastic_limit < base_limit {
            return Err(BoundError::ElasticBelowBase {
                base_limit,
                elastic_limit,
            });
        }
        Ok(Self {
            rule: rule.trim().to_owned(),
            metric: metric.trim().to_owned(),
            base_limit,
            elastic_limit,
            unlimited,
            actor_required,
        })
    }

    /// Evaluate one value in the predecessor's explicit calculation order.
    ///
    /// The effective flex limit is selected first, then the active limit, then
    /// enabled/over-limit, over-base, flex-breach, and finally latch transition.
    pub fn decide(
        &self,
        measured: u64,
        latched: bool,
        actor_inputs: &ActorInputs,
        variation_percent: u64,
    ) -> Result<BoundDecision, BoundError> {
        let actor = if self.actor_required {
            Some(actor_inputs.require_actor()?)
        } else if variation_percent > 0 && self.elastic_limit > self.base_limit && !self.unlimited {
            Some(actor_inputs.resolve_seed())
        } else {
            None
        };
        let elastic_limit = match &actor {
            Some(actor) if variation_percent > 0 && !self.unlimited => varied_elastic_limit(
                self.base_limit,
                self.elastic_limit,
                actor_inputs.candidate_path(),
                actor.value(),
                variation_percent,
            )?,
            _ => self.elastic_limit,
        };
        let active_limit = if latched {
            self.base_limit
        } else {
            elastic_limit
        };
        let enabled = !self.unlimited;
        let over_limit = enabled && measured > active_limit;
        let over_base = enabled && measured > self.base_limit;
        let elastic_breach = enabled && measured > elastic_limit;
        let latch_transition = if latched && !over_base {
            LatchTransition::Healed
        } else if latched {
            LatchTransition::Held
        } else if elastic_breach {
            LatchTransition::Latched
        } else {
            LatchTransition::Clear
        };
        Ok(BoundDecision {
            rule: self.rule.clone(),
            metric: self.metric.clone(),
            measured,
            base_limit: self.base_limit,
            configured_elastic_limit: self.elastic_limit,
            variation_percent,
            elastic_limit,
            active_limit,
            unlimited: self.unlimited,
            over_limit,
            over_base,
            elastic_breach,
            latch_transition,
            actor_inputs: actor_inputs.clone(),
            actor,
        })
    }
}

/// Reproduce Python's two-byte BLAKE2b path/actor headroom variation.
pub fn varied_elastic_limit(
    base_limit: u64,
    elastic_limit: u64,
    path: &str,
    actor: &str,
    variation_percent: u64,
) -> Result<u64, BoundError> {
    if elastic_limit <= base_limit {
        return Ok(elastic_limit);
    }
    let path = normalize_repo_path(path).map_err(|reason| BoundError::InvalidPath {
        path: path.to_owned(),
        reason,
    })?;
    let mut hasher = Blake2bVar::new(2).expect("two-byte BLAKE2b output is valid");
    hasher.update(format!("{path}\0{}", actor.trim()).as_bytes());
    let mut digest = [0_u8; 2];
    hasher
        .finalize_variable(&mut digest)
        .expect("the requested BLAKE2b output length is unchanged");
    let buckets = u128::from(variation_percent) * 2 + 1;
    let bucket = u128::from(u16::from_be_bytes(digest)) % buckets;
    let offset = i128::try_from(bucket).expect("a two-byte bucket fits i128")
        - i128::from(variation_percent);
    let headroom = i128::from(elastic_limit - base_limit);
    // Any offset below -100 consumes more than the complete headroom even
    // after binary64 rounding. At exactly -100, predecessor rounding can leave
    // a small positive remainder, so it must take the exact division path.
    if offset < COMPLETE_HEADROOM_REDUCTION_PERCENT {
        return Ok(base_limit);
    }
    let scaled = headroom
        .checked_mul(offset)
        .ok_or(BoundError::VariationOverflow)?;
    let variation = predecessor_truncated_percentage(scaled)?;
    let varied = i128::from(elastic_limit) + variation;
    let varied = varied.max(i128::from(base_limit));
    u64::try_from(varied).map_err(|_| BoundError::VariationOverflow)
}

fn predecessor_truncated_percentage(scaled: i128) -> Result<i128, BoundError> {
    if scaled == 0 {
        return Ok(0);
    }
    let negative = scaled.is_negative();
    let magnitude = scaled.unsigned_abs();
    let mut exponent = binary_exponent(magnitude, PERCENT_DENOMINATOR);
    let (numerator, denominator) = if exponent <= BINARY64_FRACTION_BITS {
        let shift = u32::try_from(BINARY64_FRACTION_BITS - exponent)
            .map_err(|_| BoundError::VariationOverflow)?;
        (
            magnitude
                .checked_shl(shift)
                .ok_or(BoundError::VariationOverflow)?,
            PERCENT_DENOMINATOR,
        )
    } else {
        let shift = u32::try_from(exponent - BINARY64_FRACTION_BITS)
            .map_err(|_| BoundError::VariationOverflow)?;
        (
            magnitude,
            PERCENT_DENOMINATOR
                .checked_shl(shift)
                .ok_or(BoundError::VariationOverflow)?,
        )
    };

    // CPython rounds the exact integer rational to nearest-even binary64
    // before int(...) truncates it. Build that 53-bit significand directly so
    // an earlier integer-to-f64 conversion cannot introduce double rounding.
    let mut significand = numerator / denominator;
    let remainder = numerator % denominator;
    let twice_remainder = remainder
        .checked_mul(2)
        .ok_or(BoundError::VariationOverflow)?;
    if twice_remainder > denominator
        || (twice_remainder == denominator && significand % 2 == 1)
    {
        significand += 1;
    }
    if significand == 1_u128 << (BINARY64_FRACTION_BITS + 1) {
        significand >>= 1;
        exponent += 1;
    }

    let truncated = if exponent >= BINARY64_FRACTION_BITS {
        let shift = u32::try_from(exponent - BINARY64_FRACTION_BITS)
            .map_err(|_| BoundError::VariationOverflow)?;
        significand
            .checked_shl(shift)
            .ok_or(BoundError::VariationOverflow)?
    } else {
        let shift = u32::try_from(BINARY64_FRACTION_BITS - exponent)
            .map_err(|_| BoundError::VariationOverflow)?;
        significand >> shift
    };
    let truncated = i128::try_from(truncated).map_err(|_| BoundError::VariationOverflow)?;
    Ok(if negative { -truncated } else { truncated })
}

fn binary_exponent(numerator: u128, denominator: u128) -> i32 {
    let numerator_bits = i32::try_from(u128::BITS - numerator.leading_zeros())
        .expect("u128 bit width fits i32");
    let denominator_bits = i32::try_from(u128::BITS - denominator.leading_zeros())
        .expect("u128 bit width fits i32");
    let mut exponent = numerator_bits - denominator_bits;
    let below_binade = if exponent >= 0 {
        numerator < denominator << u32::try_from(exponent).expect("non-negative exponent")
    } else {
        numerator << u32::try_from(-exponent).expect("negated exponent is positive") < denominator
    };
    if below_binade {
        exponent -= 1;
    }
    exponent
}

/// Every input and output behind one bound decision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BoundDecision {
    rule: String,
    metric: String,
    measured: u64,
    base_limit: u64,
    configured_elastic_limit: u64,
    variation_percent: u64,
    elastic_limit: u64,
    active_limit: u64,
    unlimited: bool,
    over_limit: bool,
    over_base: bool,
    elastic_breach: bool,
    latch_transition: LatchTransition,
    actor_inputs: ActorInputs,
    #[serde(skip_serializing_if = "Option::is_none")]
    actor: Option<ActorResolution>,
}

impl BoundDecision {
    /// Stable rule name.
    #[must_use]
    pub fn rule(&self) -> &str {
        &self.rule
    }

    /// Stable metric name.
    #[must_use]
    pub fn metric(&self) -> &str {
        &self.metric
    }

    /// Measured value.
    #[must_use]
    pub const fn measured(&self) -> u64 {
        self.measured
    }

    /// Base limit.
    #[must_use]
    pub const fn base_limit(&self) -> u64 {
        self.base_limit
    }

    /// Configured flex limit before actor variation.
    #[must_use]
    pub const fn configured_elastic_limit(&self) -> u64 {
        self.configured_elastic_limit
    }

    /// Configured actor/path variation percentage.
    #[must_use]
    pub const fn variation_percent(&self) -> u64 {
        self.variation_percent
    }

    /// Effective actor-varied flex limit.
    #[must_use]
    pub const fn elastic_limit(&self) -> u64 {
        self.elastic_limit
    }

    /// Active limit, base when latched and flex otherwise.
    #[must_use]
    pub const fn active_limit(&self) -> u64 {
        self.active_limit
    }

    /// Whether this rule disables all over-limit outcomes.
    #[must_use]
    pub const fn unlimited(&self) -> bool {
        self.unlimited
    }

    /// Whether the measured value exceeds the active limit.
    #[must_use]
    pub const fn over_limit(&self) -> bool {
        self.over_limit
    }

    /// Whether the measured value exceeds base while enabled.
    #[must_use]
    pub const fn over_base(&self) -> bool {
        self.over_base
    }

    /// Whether the measured value exceeds flex while enabled.
    #[must_use]
    pub const fn elastic_breach(&self) -> bool {
        self.elastic_breach
    }

    /// Resulting latch movement.
    #[must_use]
    pub const fn latch_transition(&self) -> LatchTransition {
        self.latch_transition
    }

    /// Complete actor/provenance inputs consulted by this decision.
    #[must_use]
    pub const fn actor_inputs(&self) -> &ActorInputs {
        &self.actor_inputs
    }

    /// Selected actor or jitter seed, when the rule used one.
    #[must_use]
    pub fn actor(&self) -> Option<&ActorResolution> {
        self.actor.as_ref()
    }
}

/// How one observation changes sticky state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LatchTransition {
    Clear,
    Latched,
    Held,
    Healed,
}

/// A malformed bound or unresolved actor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BoundError {
    EmptyRule,
    EmptyMetric,
    ZeroBaseLimit,
    ElasticBelowBase { base_limit: u64, elastic_limit: u64 },
    MissingActor,
    InvalidPath { path: String, reason: String },
    VariationOverflow,
}

impl fmt::Display for BoundError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyRule => formatter.write_str("bound rule must be non-empty"),
            Self::EmptyMetric => formatter.write_str("bound metric must be non-empty"),
            Self::ZeroBaseLimit => formatter.write_str("bound base limit must be positive"),
            Self::ElasticBelowBase {
                base_limit,
                elastic_limit,
            } => write!(
                formatter,
                "bound elastic limit {elastic_limit} must be at least base limit {base_limit}"
            ),
            Self::MissingActor => formatter.write_str(
                "rule requires actor identity; provide published task session or active actor",
            ),
            Self::InvalidPath { path, reason } => {
                write!(formatter, "invalid repository path {path:?}: {reason}")
            }
            Self::VariationOverflow => formatter.write_str("actor-varied elastic limit overflowed"),
        }
    }
}

impl Error for BoundError {}
