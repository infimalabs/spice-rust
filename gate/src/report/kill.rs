/// Complete mutation inventory retained without an executable type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportMutationInventory {
    tool: String,
    targets: Vec<String>,
    mutants: Vec<String>,
    attempts: u64,
    seed: u64,
    timeout_seconds: u64,
}

impl ReportMutationInventory {
    fn current(run: &KillRun) -> Self {
        let inventory = run.inventory();
        Self {
            tool: PINNED_CARGO_MUTANTS.to_owned(),
            targets: inventory.targets().to_vec(),
            mutants: inventory.mutants().to_vec(),
            attempts: u64::try_from(inventory.attempts())
                .expect("a Rust collection length fits in the report count"),
            seed: inventory.seed(),
            timeout_seconds: inventory.timeout_seconds(),
        }
    }

    /// Pinned mutation tool identity observed before the run.
    #[must_use]
    pub fn tool(&self) -> &str {
        &self.tool
    }

    /// Canonically ordered target selection.
    #[must_use]
    pub fn targets(&self) -> &[String] {
        &self.targets
    }

    /// Canonically ordered mutant identities.
    #[must_use]
    pub fn mutants(&self) -> &[String] {
        &self.mutants
    }

    /// Attempts required for each mutant.
    #[must_use]
    pub const fn attempts(&self) -> u64 {
        self.attempts
    }

    /// Seed repeated into each attempt.
    #[must_use]
    pub const fn seed(&self) -> u64 {
        self.seed
    }

    /// Integer mutation-tool timeout retained as evidence.
    #[must_use]
    pub const fn timeout_seconds(&self) -> u64 {
        self.timeout_seconds
    }
}

/// One exact rational rate in a report-only shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportRate {
    numerator: u64,
    denominator: u64,
}

impl ReportRate {
    /// Reduced survivor numerator.
    #[must_use]
    pub const fn numerator(self) -> u64 {
        self.numerator
    }

    /// Reduced confirmed-mutant denominator.
    #[must_use]
    pub const fn denominator(self) -> u64 {
        self.denominator
    }

    fn checked(self) -> Result<SurvivorRate, &'static str> {
        let rate = SurvivorRate::new(self.numerator, self.denominator)
            .map_err(|_| "a survivor rate is outside its admitted range")?;
        if Self::from(rate) != self {
            return Err("a survivor rate is not reduced canonically");
        }
        Ok(rate)
    }
}

impl From<SurvivorRate> for ReportRate {
    fn from(rate: SurvivorRate) -> Self {
        Self {
            numerator: rate.numerator(),
            denominator: rate.denominator(),
        }
    }
}

/// Captured stream evidence or explicit absence before capture existed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case", tag = "availability")]
pub enum ReportStream {
    /// Capture was never established.
    Unavailable,
    /// Exact retained bytes and both capture completeness flags.
    Captured {
        /// Retained bytes.
        bytes: Vec<u8>,
        /// Whether bytes beyond the capture bound were discarded.
        truncated: bool,
        /// Whether the stream reached EOF before the drain deadline.
        complete: bool,
    },
}

impl ReportStream {
    fn current(stream: Option<&StreamEvidence>) -> Self {
        match stream {
            None => Self::Unavailable,
            Some(stream) => Self::Captured {
                bytes: stream.bytes.clone(),
                truncated: stream.truncated,
                complete: stream.complete,
            },
        }
    }

    fn complete(&self) -> bool {
        matches!(
            self,
            Self::Captured {
                truncated: false,
                complete: true,
                ..
            }
        )
    }

    fn evidence(&self) -> Option<StreamEvidence> {
        match self {
            Self::Unavailable => None,
            Self::Captured {
                bytes,
                truncated,
                complete,
            } => Some(StreamEvidence {
                bytes: bytes.clone(),
                truncated: *truncated,
                complete: *complete,
            }),
        }
    }
}

/// How one retained bounded process completed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case", tag = "kind")]
pub enum ReportCompletion {
    /// Ordinary process exit.
    Exited {
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present"
        )]
        code: Option<i32>,
    },
    /// Runtime deadline termination.
    TimedOut {
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present"
        )]
        code: Option<i32>,
    },
    /// Cancellation termination.
    Cancelled {
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present"
        )]
        code: Option<i32>,
    },
    /// Executable launch failure.
    LaunchError { evidence: String },
    /// Waiting or reaping failure.
    WaitError { evidence: String },
    /// Fixed process description failure.
    SpecError { evidence: String },
}

impl ReportCompletion {
    fn current(completion: &ProcessCompletion) -> Self {
        match completion {
            ProcessCompletion::Exited(code) => Self::Exited { code: *code },
            ProcessCompletion::TimedOut(code) => Self::TimedOut { code: *code },
            ProcessCompletion::Cancelled(code) => Self::Cancelled { code: *code },
            ProcessCompletion::LaunchError(evidence) => Self::LaunchError {
                evidence: evidence.clone(),
            },
            ProcessCompletion::WaitError(evidence) => Self::WaitError {
                evidence: evidence.clone(),
            },
            ProcessCompletion::SpecError(evidence) => Self::SpecError {
                evidence: evidence.clone(),
            },
        }
    }

    fn evidence(&self) -> ProcessCompletion {
        match self {
            Self::Exited { code } => ProcessCompletion::Exited(*code),
            Self::TimedOut { code } => ProcessCompletion::TimedOut(*code),
            Self::Cancelled { code } => ProcessCompletion::Cancelled(*code),
            Self::LaunchError { evidence } => ProcessCompletion::LaunchError(evidence.clone()),
            Self::WaitError { evidence } => ProcessCompletion::WaitError(evidence.clone()),
            Self::SpecError { evidence } => ProcessCompletion::SpecError(evidence.clone()),
        }
    }
}

/// Complete retained evidence for one bounded process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportProcess {
    completion: ReportCompletion,
    stdout: ReportStream,
    stderr: ReportStream,
}

impl ReportProcess {
    fn current(process: &ProcessEvidence) -> Self {
        Self {
            completion: ReportCompletion::current(&process.completion),
            stdout: ReportStream::current(process.stdout.as_ref()),
            stderr: ReportStream::current(process.stderr.as_ref()),
        }
    }

    /// Completion evidence.
    #[must_use]
    pub const fn completion(&self) -> &ReportCompletion {
        &self.completion
    }

    /// Standard output evidence.
    #[must_use]
    pub const fn stdout(&self) -> &ReportStream {
        &self.stdout
    }

    /// Standard error evidence.
    #[must_use]
    pub const fn stderr(&self) -> &ReportStream {
        &self.stderr
    }

    fn outputs_complete(&self) -> bool {
        self.stdout.complete() && self.stderr.complete()
    }

    fn evidence(&self) -> ProcessEvidence {
        ProcessEvidence {
            completion: self.completion.evidence(),
            stdout: self.stdout.evidence(),
            stderr: self.stderr.evidence(),
        }
    }
}

/// Report-only meaning of one mutation attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReportAttemptKind {
    Killed,
    Survived,
    TimedOut,
    LaunchError,
    WaitError,
    Undecodable,
}

impl ReportAttemptKind {
    fn current(kind: &AttemptKind) -> Self {
        match kind {
            AttemptKind::Killed => Self::Killed,
            AttemptKind::Survived => Self::Survived,
            AttemptKind::TimedOut => Self::TimedOut,
            AttemptKind::LaunchError => Self::LaunchError,
            AttemptKind::WaitError => Self::WaitError,
            AttemptKind::Undecodable => Self::Undecodable,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Killed => "killed",
            Self::Survived => "survived",
            Self::TimedOut => "timed-out",
            Self::LaunchError => "launch-error",
            Self::WaitError => "wait-error",
            Self::Undecodable => "undecodable",
        }
    }
}

/// Retained mutation outcome artifact evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case", tag = "kind")]
pub enum ReportOutcome {
    /// Exact outcome artifact bytes.
    Present { bytes: Vec<u8> },
    /// No artifact was published.
    Absent,
    /// Artifact replacement or reading failed.
    Unreadable { evidence: String },
}

impl ReportOutcome {
    fn current(outcome: &OutcomeEvidence) -> Self {
        match outcome {
            OutcomeEvidence::Present(bytes) => Self::Present {
                bytes: bytes.clone(),
            },
            OutcomeEvidence::Absent => Self::Absent,
            OutcomeEvidence::Unreadable(evidence) => Self::Unreadable {
                evidence: evidence.clone(),
            },
        }
    }

    fn evidence(&self) -> OutcomeEvidence {
        match self {
            Self::Present { bytes } => OutcomeEvidence::Present(bytes.clone()),
            Self::Absent => OutcomeEvidence::Absent,
            Self::Unreadable { evidence } => OutcomeEvidence::Unreadable(evidence.clone()),
        }
    }
}

/// One ordered mutation attempt and every retained observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportAttempt {
    number: u64,
    seed: u64,
    kind: ReportAttemptKind,
    process: ReportProcess,
    outcome: ReportOutcome,
}

impl ReportAttempt {
    fn current(attempt: &AttemptEvidence) -> Self {
        Self {
            number: u64::try_from(attempt.number)
                .expect("a Rust collection index fits in the report count"),
            seed: attempt.seed,
            kind: ReportAttemptKind::current(&attempt.kind),
            process: ReportProcess::current(&attempt.process),
            outcome: ReportOutcome::current(&attempt.outcome),
        }
    }

    /// One-based attempt number.
    #[must_use]
    pub const fn number(&self) -> u64 {
        self.number
    }

    /// Repeated deterministic seed.
    #[must_use]
    pub const fn seed(&self) -> u64 {
        self.seed
    }

    /// Decoded attempt meaning.
    #[must_use]
    pub const fn kind(&self) -> ReportAttemptKind {
        self.kind
    }

    /// Complete process evidence.
    #[must_use]
    pub const fn process(&self) -> &ReportProcess {
        &self.process
    }

    /// Outcome artifact evidence.
    #[must_use]
    pub const fn outcome(&self) -> &ReportOutcome {
        &self.outcome
    }
}

/// Retry disposition retained without a scorer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReportMutantDisposition {
    ConfirmedKilled,
    ConfirmedSurvived,
    Unconfirmed,
}

impl ReportMutantDisposition {
    fn current(disposition: MutantDisposition) -> Self {
        match disposition {
            MutantDisposition::ConfirmedKilled => Self::ConfirmedKilled,
            MutantDisposition::ConfirmedSurvived => Self::ConfirmedSurvived,
            MutantDisposition::Unconfirmed => Self::Unconfirmed,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::ConfirmedKilled => "confirmed-killed",
            Self::ConfirmedSurvived => "confirmed-survived",
            Self::Unconfirmed => "unconfirmed",
        }
    }
}

/// One mutant and all ordered attempt evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportMutant {
    identity: String,
    attempts: Vec<ReportAttempt>,
    disposition: ReportMutantDisposition,
}

impl ReportMutant {
    fn current(mutant: &crate::kill::MutantEvidence) -> Self {
        Self {
            identity: mutant.identity.clone(),
            attempts: mutant.attempts.iter().map(ReportAttempt::current).collect(),
            disposition: ReportMutantDisposition::current(mutant.disposition),
        }
    }

    /// Stable inventory identity.
    #[must_use]
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// Attempts in execution order.
    #[must_use]
    pub fn attempts(&self) -> &[ReportAttempt] {
        &self.attempts
    }

    /// Disposition derived from every attempt.
    #[must_use]
    pub const fn disposition(&self) -> ReportMutantDisposition {
        self.disposition
    }
}

/// Lower-is-better mutation movement in report-only form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case", tag = "movement")]
pub enum ReportMutationMovement {
    Held {
        floor: ReportRate,
    },
    Shrank {
        floor: ReportRate,
        measured: ReportRate,
    },
    Regressed {
        floor: ReportRate,
        measured: ReportRate,
    },
    Unbaselined {
        measured: ReportRate,
    },
    Unconfirmed {
        confirmed: u64,
        total: u64,
    },
}

impl ReportMutationMovement {
    fn current(movement: MutationMovement) -> Self {
        match movement {
            MutationMovement::Held { floor } => Self::Held {
                floor: floor.into(),
            },
            MutationMovement::Shrank { floor, measured } => Self::Shrank {
                floor: floor.into(),
                measured: measured.into(),
            },
            MutationMovement::Regressed { floor, measured } => Self::Regressed {
                floor: floor.into(),
                measured: measured.into(),
            },
            MutationMovement::Unbaselined { measured } => Self::Unbaselined {
                measured: measured.into(),
            },
            MutationMovement::Unconfirmed { confirmed, total } => Self::Unconfirmed {
                confirmed: u64::try_from(confirmed)
                    .expect("a Rust collection length fits in the report count"),
                total: u64::try_from(total)
                    .expect("a Rust collection length fits in the report count"),
            },
        }
    }

    const fn proposal(self) -> Option<ReportRate> {
        match self {
            Self::Held { floor } => Some(floor),
            Self::Shrank { measured, .. } | Self::Unbaselined { measured } => Some(measured),
            Self::Regressed { .. } | Self::Unconfirmed { .. } => None,
        }
    }
}

/// One strict, versioned mutation gate result body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KillReport {
    version: u64,
    inventory: ReportMutationInventory,
    baseline: ReportProcess,
    mutants: Vec<ReportMutant>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    rate: Option<ReportRate>,
    movement: ReportMutationMovement,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    proposed_floor: Option<ReportRate>,
}

impl KillReport {
    /// Copy one completed mutation run into a report-only value.
    #[must_use]
    pub fn current(run: &KillRun) -> Self {
        Self {
            version: KILL_REPORT_VERSION,
            inventory: ReportMutationInventory::current(run),
            baseline: ReportProcess::current(run.baseline()),
            mutants: run.mutants().iter().map(ReportMutant::current).collect(),
            rate: run.rate().map(ReportRate::from),
            movement: ReportMutationMovement::current(run.movement()),
            proposed_floor: run.proposed_floor().map(ReportRate::from),
        }
    }

    /// Current mutation report schema.
    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    /// Complete canonical inventory in report-only form.
    #[must_use]
    pub const fn inventory(&self) -> &ReportMutationInventory {
        &self.inventory
    }

    /// Green unmutated baseline evidence.
    #[must_use]
    pub const fn baseline(&self) -> &ReportProcess {
        &self.baseline
    }

    /// Mutants in inventory order with attempts in execution order.
    #[must_use]
    pub fn mutants(&self) -> &[ReportMutant] {
        &self.mutants
    }

    /// Confirmed survivor rate, absent when any mutant was unconfirmed.
    #[must_use]
    pub const fn rate(&self) -> Option<ReportRate> {
        self.rate
    }

    /// Lower-is-better movement.
    #[must_use]
    pub const fn movement(&self) -> ReportMutationMovement {
        self.movement
    }

    /// Optional shrink-only floor proposal.
    #[must_use]
    pub const fn proposed_floor(&self) -> Option<ReportRate> {
        self.proposed_floor
    }

    /// Deterministic canonical text carrying every retained observation.
    #[must_use]
    pub fn render(&self) -> String {
        let mut rendered = format!(
            "kill-report {}\ntool {}\nattempts {}\nseed {}\ntimeout-seconds {}\n",
            self.version,
            self.inventory.tool,
            self.inventory.attempts,
            self.inventory.seed,
            self.inventory.timeout_seconds,
        );
        for target in &self.inventory.targets {
            let _ = writeln!(rendered, "target {}", quoted(target));
        }
        for mutant in &self.inventory.mutants {
            let _ = writeln!(rendered, "inventory-mutant {}", quoted(mutant));
        }
        render_process(&mut rendered, "baseline", &self.baseline);
        for mutant in &self.mutants {
            let _ = writeln!(
                rendered,
                "mutant {} disposition {}",
                quoted(&mutant.identity),
                mutant.disposition.as_str()
            );
            for attempt in &mutant.attempts {
                let prefix = format!("attempt {} {}", quoted(&mutant.identity), attempt.number);
                let _ = writeln!(
                    rendered,
                    "{prefix} seed {} kind {}",
                    attempt.seed,
                    attempt.kind.as_str()
                );
                render_process(&mut rendered, &prefix, &attempt.process);
                render_outcome(&mut rendered, &prefix, &attempt.outcome);
            }
        }
        render_optional_rate(&mut rendered, "rate", self.rate);
        render_movement(&mut rendered, self.movement);
        render_optional_rate(&mut rendered, "proposed-floor", self.proposed_floor);
        rendered
    }

    /// Canonical JSON representation.
    ///
    /// # Errors
    ///
    /// Returns the canonical encoder failure.
    pub fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        wire::encode(self)
    }

    /// Decode only the current, canonical, internally coherent report.
    ///
    /// # Errors
    ///
    /// Returns malformed fields, a named foreign version, a second JSON
    /// spelling, or the retained evidence that disagreed.
    pub fn decode(bytes: &[u8]) -> Result<Self, ReportDecodeError> {
        let version: ReportVersionEvidence =
            wire::decode(bytes).map_err(ReportDecodeError::Json)?;
        if version.version != KILL_REPORT_VERSION {
            return Err(ReportDecodeError::unsupported(
                "kill report",
                version.version,
                KILL_REPORT_VERSION,
            ));
        }
        let report: Self = wire::decode(bytes).map_err(ReportDecodeError::Json)?;
        report.validate()?;
        let canonical = report.encode().map_err(ReportDecodeError::Json)?;
        if canonical != bytes {
            return Err(ReportDecodeError::NonCanonical { canonical });
        }
        Ok(report)
    }

    fn validate(&self) -> Result<(), ReportDecodeError> {
        validate_kill_inventory(&self.inventory)?;
        if !matches!(
            self.baseline.completion,
            ReportCompletion::Exited { code: Some(0) }
        ) || !self.baseline.outputs_complete()
        {
            return Err(ReportDecodeError::inconsistent(
                "kill report",
                "the unmutated baseline is not complete and green",
            ));
        }
        if self.mutants.len() != self.inventory.mutants.len()
            || self
                .mutants
                .iter()
                .map(|mutant| mutant.identity.as_str())
                .ne(self.inventory.mutants.iter().map(String::as_str))
        {
            return Err(ReportDecodeError::inconsistent(
                "kill report",
                "mutant evidence does not equal the canonical inventory order",
            ));
        }
        for mutant in &self.mutants {
            validate_mutant(mutant, &self.inventory)?;
        }
        let confirmed = self
            .mutants
            .iter()
            .filter(|mutant| mutant.disposition != ReportMutantDisposition::Unconfirmed)
            .count();
        let survivors = self
            .mutants
            .iter()
            .filter(|mutant| mutant.disposition == ReportMutantDisposition::ConfirmedSurvived)
            .count();
        let expected_rate = if confirmed == self.mutants.len() {
            Some(ReportRate::from(
                SurvivorRate::new(survivors as u64, confirmed as u64)
                    .expect("a non-empty complete inventory forms a rate"),
            ))
        } else {
            None
        };
        if self.rate != expected_rate {
            return Err(ReportDecodeError::inconsistent(
                "kill report",
                "confirmed survivor rate disagrees with mutant dispositions",
            ));
        }
        validate_mutation_movement(self.movement, expected_rate, confirmed, self.mutants.len())?;
        if self.proposed_floor != self.movement.proposal() {
            return Err(ReportDecodeError::inconsistent(
                "kill report",
                "floor proposal is not the shrink-only result of movement",
            ));
        }
        Ok(())
    }
}

impl KillRun {
    /// Copy this completed mutation run into a report-only crossing value.
    #[must_use]
    pub fn report(&self) -> KillReport {
        KillReport::current(self)
    }
}

fn validate_kill_inventory(inventory: &ReportMutationInventory) -> Result<(), ReportDecodeError> {
    if inventory.tool != PINNED_CARGO_MUTANTS {
        return Err(ReportDecodeError::inconsistent(
            "kill report",
            "inventory tool is not the pinned current mutation tool",
        ));
    }
    if inventory.attempts == 0 || inventory.timeout_seconds == 0 {
        return Err(ReportDecodeError::inconsistent(
            "kill report",
            "inventory attempts and timeout must be positive",
        ));
    }
    if inventory.targets.is_empty() || inventory.mutants.is_empty() {
        return Err(ReportDecodeError::inconsistent(
            "kill report",
            "inventory targets and mutants must be nonempty",
        ));
    }
    require_strict_names("kill report", "inventory targets", inventory.targets.iter())?;
    require_strict_names("kill report", "inventory mutants", inventory.mutants.iter())
}

fn validate_mutant(
    mutant: &ReportMutant,
    inventory: &ReportMutationInventory,
) -> Result<(), ReportDecodeError> {
    if u64::try_from(mutant.attempts.len()).ok() != Some(inventory.attempts) {
        return Err(ReportDecodeError::inconsistent(
            "kill report",
            format!("mutant {:?} has the wrong attempt count", mutant.identity),
        ));
    }
    for (offset, attempt) in mutant.attempts.iter().enumerate() {
        let expected = u64::try_from(offset + 1).expect("a Rust collection index fits in u64");
        if attempt.number != expected || attempt.seed != inventory.seed {
            return Err(ReportDecodeError::inconsistent(
                "kill report",
                format!(
                    "mutant {:?} attempt number or seed is out of sequence",
                    mutant.identity
                ),
            ));
        }
        if attempt.kind != derive_attempt_kind(&attempt.process, &attempt.outcome) {
            return Err(ReportDecodeError::inconsistent(
                "kill report",
                format!("mutant {:?} attempt kind disagrees", mutant.identity),
            ));
        }
    }
    let disposition = if mutant
        .attempts
        .iter()
        .all(|attempt| attempt.kind == ReportAttemptKind::Killed)
    {
        ReportMutantDisposition::ConfirmedKilled
    } else if mutant
        .attempts
        .iter()
        .all(|attempt| attempt.kind == ReportAttemptKind::Survived)
    {
        ReportMutantDisposition::ConfirmedSurvived
    } else {
        ReportMutantDisposition::Unconfirmed
    };
    if mutant.disposition != disposition {
        return Err(ReportDecodeError::inconsistent(
            "kill report",
            format!(
                "mutant {:?} disposition disagrees with its attempts",
                mutant.identity
            ),
        ));
    }
    Ok(())
}

fn validate_mutation_movement(
    movement: ReportMutationMovement,
    rate: Option<ReportRate>,
    confirmed: usize,
    total: usize,
) -> Result<(), ReportDecodeError> {
    let inconsistent = || {
        ReportDecodeError::inconsistent(
            "kill report",
            "mutation movement disagrees with the confirmed survivor rate",
        )
    };
    match (movement, rate) {
        (
            ReportMutationMovement::Unconfirmed {
                confirmed: observed_confirmed,
                total: observed_total,
            },
            None,
        ) if observed_confirmed == confirmed as u64 && observed_total == total as u64 => Ok(()),
        (ReportMutationMovement::Held { floor }, Some(measured)) if floor == measured => {
            floor.checked().map(|_| ()).map_err(|_| inconsistent())
        }
        (ReportMutationMovement::Shrank { floor, measured }, Some(observed))
            if measured == observed && rate_order(measured, floor)? == std::cmp::Ordering::Less =>
        {
            Ok(())
        }
        (ReportMutationMovement::Regressed { floor, measured }, Some(observed))
            if measured == observed
                && rate_order(measured, floor)? == std::cmp::Ordering::Greater =>
        {
            Ok(())
        }
        (ReportMutationMovement::Unbaselined { measured }, Some(observed))
            if measured == observed =>
        {
            measured.checked().map(|_| ()).map_err(|_| inconsistent())
        }
        _ => Err(inconsistent()),
    }
}

fn rate_order(
    left: ReportRate,
    right: ReportRate,
) -> Result<std::cmp::Ordering, ReportDecodeError> {
    left.checked()
        .map_err(|evidence| ReportDecodeError::inconsistent("kill report", evidence))?;
    right
        .checked()
        .map_err(|evidence| ReportDecodeError::inconsistent("kill report", evidence))?;
    Ok((u128::from(left.numerator) * u128::from(right.denominator))
        .cmp(&(u128::from(right.numerator) * u128::from(left.denominator))))
}

fn derive_attempt_kind(process: &ReportProcess, outcome: &ReportOutcome) -> ReportAttemptKind {
    ReportAttemptKind::current(&crate::kill::decode_attempt(
        &process.evidence(),
        &outcome.evidence(),
    ))
}
