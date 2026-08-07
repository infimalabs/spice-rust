#[derive(Clone, Debug)]
struct RunContext {
    now: f64,
    live_paths: BTreeSet<String>,
    reconciled: bool,
}

/// One loaded current ledger with an in-memory run in progress.
#[derive(Clone, Debug)]
pub struct CurrentState {
    destination: PathBuf,
    before: StateLedger,
    working: StateLedger,
    initially_absent: bool,
    run: Option<RunContext>,
}

impl CurrentState {
    /// Load current state, starting from an empty current ledger when the
    /// destination does not exist.
    pub fn load(destination: impl Into<PathBuf>) -> Result<Self, StateError> {
        let destination = destination.into();
        let (ledger, initially_absent) = match read_optional(&destination, CURRENT_STATE_RELEASE)? {
            Some(bytes) => (decode_current(&destination, &bytes)?, false),
            None => (
                StateLedger {
                    version: STATE_LEDGER_VERSION,
                    latches: Vec::new(),
                    claims: Vec::new(),
                },
                true,
            ),
        };
        Ok(Self {
            destination,
            before: ledger.clone(),
            working: ledger,
            initially_absent,
            run: None,
        })
    }

    /// Logical ledger loaded at the start of this transaction.
    #[must_use]
    pub const fn before(&self) -> &StateLedger {
        &self.before
    }

    /// Begin one deterministic run, applying staged renames and pruning
    /// missing or expired state before any decisions are made.
    pub fn begin_run<I, P>(
        &mut self,
        renames: &RenameMap,
        live_paths: I,
        now: f64,
    ) -> Result<(), StateError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<str>,
    {
        if self.run.is_some() {
            return Err(StateError::current(
                StateErrorKind::Malformed,
                &self.destination,
                "a state transaction may begin only one run",
                "finish or discard the current transaction before starting another",
            ));
        }
        if !now.is_finite() {
            return Err(StateError::current(
                StateErrorKind::Malformed,
                &self.destination,
                "run clock must be finite",
                "supply a finite Unix timestamp from the run clock",
            ));
        }
        let mut live = BTreeSet::new();
        for path in live_paths {
            let raw = path.as_ref();
            let normalized = normalize_repo_path(raw).map_err(|reason| {
                StateError::current(
                    StateErrorKind::Malformed,
                    &self.destination,
                    format!("invalid live path {raw:?}: {reason}"),
                    "provide canonical repository-relative live paths",
                )
            })?;
            live.insert(normalized);
        }

        let mut latches: BTreeSet<LatchEntry> = self.working.latches.iter().cloned().collect();
        for entry in self.working.latches.clone() {
            let renamed = renames.destination(entry.key.repo_path());
            if renamed != entry.key.repo_path() {
                latches.insert(LatchEntry {
                    kind: entry.kind,
                    key: entry.key.renamed(renamed),
                });
            }
        }
        latches.retain(|entry| live.contains(entry.key.repo_path()));

        let mut claims = Vec::new();
        let mut claim_keys = BTreeSet::new();
        for claim in &self.working.claims {
            if claim.expires_at <= now {
                continue;
            }
            let path = renames.destination(&claim.path).to_owned();
            if !live.contains(&path) {
                continue;
            }
            let key = (path.clone(), claim.actor.clone());
            if !claim_keys.insert(key) {
                return Err(StateError::current(
                    StateErrorKind::Collision,
                    &self.destination,
                    format!(
                        "staged rename collides for claim path {path:?} actor {:?}",
                        claim.actor
                    ),
                    "remove the duplicate claim or resolve the staged rename collision",
                ));
            }
            claims.push(PeerClaim {
                path,
                actor: claim.actor.clone(),
                created_at: claim.created_at,
                expires_at: claim.expires_at,
            });
        }
        sort_claims(&mut claims);
        self.working.latches = latches.into_iter().collect();
        self.working.claims = claims;
        self.run = Some(RunContext {
            now,
            live_paths: live,
            reconciled: false,
        });
        Ok(())
    }

    /// Reconcile a complete current observation inventory.
    ///
    /// Entries omitted from this inventory are absent and therefore pruned.
    /// Current flex breaches are added; prior latches remain only while the
    /// enabled rule is over base and otherwise heal.
    pub fn reconcile(
        &mut self,
        observations: impl IntoIterator<Item = LatchObservation>,
    ) -> Result<Vec<StateDecision>, StateError> {
        let run = self.run.as_ref().ok_or_else(|| {
            StateError::current(
                StateErrorKind::Malformed,
                &self.destination,
                "state reconciliation requires begin_run",
                "begin the transaction with its fixed clock, renames, and live paths",
            )
        })?;
        if run.reconciled {
            return Err(StateError::current(
                StateErrorKind::Malformed,
                &self.destination,
                "state reconciliation may run exactly once per transaction",
                "start a new transaction for a different observation inventory",
            ));
        }
        let prior: BTreeSet<LatchEntry> = self.working.latches.iter().cloned().collect();
        let mut observations: Vec<_> = observations.into_iter().collect();
        observations.sort_by(|left, right| left.entry.cmp(&right.entry));
        for pair in observations.windows(2) {
            if pair[0].entry == pair[1].entry {
                return Err(StateError::current(
                    StateErrorKind::Collision,
                    &self.destination,
                    format!("duplicate current observation for {:?}", pair[0].entry),
                    "emit exactly one observation for each current sticky key",
                ));
            }
        }

        let mut updated = BTreeSet::new();
        let mut decisions = Vec::with_capacity(observations.len());
        for observation in observations {
            if !run.live_paths.contains(observation.entry.key.repo_path()) {
                return Err(StateError::current(
                    StateErrorKind::Malformed,
                    &self.destination,
                    format!(
                        "observation path {:?} is absent from the live inventory",
                        observation.entry.key.repo_path()
                    ),
                    "include the path in the run inventory or omit its observation",
                ));
            }
            let latched = prior.contains(&observation.entry);
            let bound = observation
                .bound
                .decide(
                    observation.measured,
                    latched,
                    &observation.actor_inputs,
                    observation.variation_percent,
                )
                .map_err(|error| {
                    StateError::current(
                        StateErrorKind::Malformed,
                        &self.destination,
                        error.to_string(),
                        "repair the bound or provide the required actor identity",
                    )
                })?;
            if matches!(
                bound.latch_transition,
                LatchTransition::Latched | LatchTransition::Held
            ) {
                updated.insert(observation.entry.clone());
            }
            decisions.push(StateDecision {
                entry: observation.entry,
                bound,
            });
        }
        self.working.latches = updated.into_iter().collect();
        self.run
            .as_mut()
            .expect("the run remains active through reconciliation")
            .reconciled = true;
        Ok(decisions)
    }

    /// Claim live flex paths for the required actor, preserving an existing
    /// same-actor creation time and redirecting when a peer owns the path.
    pub fn claim<I, P>(
        &mut self,
        paths: I,
        actors: &ActorInputs,
    ) -> Result<Vec<ClaimDecision>, StateError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<str>,
    {
        let run = self.run.as_ref().ok_or_else(|| {
            StateError::current(
                StateErrorKind::Malformed,
                &self.destination,
                "peer claims require begin_run",
                "begin the transaction with its fixed clock and live inventory",
            )
        })?;
        let claimant = actors.require_actor().map_err(|error| {
            StateError::current(
                StateErrorKind::MissingActor,
                &self.destination,
                error.to_string(),
                "provide a published task session or active actor",
            )
        })?;
        let mut requested = BTreeSet::new();
        for path in paths {
            let raw = path.as_ref();
            let normalized = normalize_repo_path(raw).map_err(|reason| {
                StateError::current(
                    StateErrorKind::Malformed,
                    &self.destination,
                    format!("invalid claim path {raw:?}: {reason}"),
                    "claim canonical repository-relative paths",
                )
            })?;
            if !run.live_paths.contains(&normalized) {
                return Err(StateError::current(
                    StateErrorKind::Malformed,
                    &self.destination,
                    format!("claim path {normalized:?} is not live"),
                    "claim only paths in the fixed run inventory",
                ));
            }
            requested.insert(normalized);
        }

        let active = self.working.claims.clone();
        let mut updated = active.clone();
        let mut decisions = Vec::new();
        for path in requested {
            let owner = owning_claim(&active, &path);
            if let Some(owner) = owner
                && owner.actor != claimant.value()
            {
                decisions.push(ClaimDecision {
                    path,
                    claimant: claimant.clone(),
                    claim: owner.clone(),
                    status: ClaimStatus::PeerHeld,
                });
                continue;
            }
            let claim = PeerClaim {
                path: path.clone(),
                actor: claimant.value().to_owned(),
                created_at: owner.map_or(run.now, |claim| claim.created_at),
                expires_at: run.now + PEER_CLAIM_TTL_SECONDS as f64,
            };
            let status = if owner.is_some() {
                ClaimStatus::Owned
            } else {
                ClaimStatus::Claimed
            };
            updated.retain(|existing| existing.path != path || existing.actor != claimant.value());
            updated.push(claim.clone());
            decisions.push(ClaimDecision {
                path,
                claimant: claimant.clone(),
                claim,
                status,
            });
        }
        sort_claims(&mut updated);
        self.working.claims = updated;
        Ok(decisions)
    }

    /// Freeze the proposed after-ledger for acceptance or rejection.
    pub fn finish(self) -> Result<PendingState, StateError> {
        match self.run.as_ref() {
            None => {
                return Err(StateError::current(
                    StateErrorKind::Malformed,
                    &self.destination,
                    "cannot finish a state transaction before begin_run",
                    "begin and complete the run before finishing its state",
                ));
            }
            Some(run) if !run.reconciled => {
                return Err(StateError::current(
                    StateErrorKind::Malformed,
                    &self.destination,
                    "cannot finish before reconciling the complete observation inventory",
                    "replay the run and reconcile exactly once before finishing its state",
                ));
            }
            Some(_) => {}
        }
        Ok(PendingState {
            destination: self.destination,
            before: self.before,
            after: self.working,
            initially_absent: self.initially_absent,
        })
    }
}

/// Claim outcome for one requested path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaimStatus {
    Claimed,
    Owned,
    PeerHeld,
}

/// One claimant's deterministic disposition for a path.
#[derive(Clone, Debug, PartialEq)]
pub struct ClaimDecision {
    path: String,
    claimant: ActorResolution,
    claim: PeerClaim,
    status: ClaimStatus,
}

impl ClaimDecision {
    /// Requested canonical path.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Required actor that requested ownership.
    #[must_use]
    pub const fn claimant(&self) -> &ActorResolution {
        &self.claimant
    }

    /// Effective owner claim.
    #[must_use]
    pub const fn claim(&self) -> &PeerClaim {
        &self.claim
    }

    /// Claimed, refreshed ownership, or peer-held redirection.
    #[must_use]
    pub const fn status(&self) -> ClaimStatus {
        self.status
    }
}

/// Frozen state mutation awaiting the run's acceptance decision.
#[derive(Clone, Debug)]
pub struct PendingState {
    destination: PathBuf,
    before: StateLedger,
    after: StateLedger,
    initially_absent: bool,
}

impl PendingState {
    /// Exact proposed before-ledger.
    #[must_use]
    pub const fn before(&self) -> &StateLedger {
        &self.before
    }

    /// Exact proposed after-ledger.
    #[must_use]
    pub const fn after(&self) -> &StateLedger {
        &self.after
    }

    /// Reject the run without touching the destination.
    #[must_use]
    pub fn reject(self) -> StateReceipt {
        StateReceipt::new(&self.before, &self.after, false)
    }

    /// Atomically publish the after-ledger at the sole accepted-run boundary.
    ///
    /// The first accepted run always writes its current ledger, even when
    /// reconciliation did not otherwise move it.
    pub fn accept(self) -> Result<StateReceipt, StateError> {
        let changed = self.initially_absent || self.before != self.after;
        let bytes = changed
            .then(|| self.after.encode())
            .transpose()
            .map_err(|error| {
                StateError::current(
                    StateErrorKind::Encode,
                    &self.destination,
                    format!("could not encode current state: {error}"),
                    "report the schema defect; do not replace the prior ledger",
                )
            })?;
        let parent = self.destination.parent().ok_or_else(|| {
            StateError::current(
                StateErrorKind::Persist,
                &self.destination,
                "current state destination has no parent directory",
                "select the fixed Git-common state path",
            )
        })?;
        fs::create_dir_all(parent).map_err(|error| {
            StateError::current(
                StateErrorKind::Persist,
                &self.destination,
                format!("could not create current state directory: {error}"),
                "repair the Git-common state directory and retry the accepted run",
            )
        })?;
        let lock_name = LockName::new("gate-state")
            .expect("the fixed current state lock name is one normal component");
        let lock = LockFile::new(lock_name, parent.join(CURRENT_STATE_LOCK_FILE));
        let _possession = lock.acquire().map_err(|error| {
            StateError::current(
                StateErrorKind::Persist,
                lock.path(),
                format!("could not acquire current state publication lock: {error}"),
                "repair the Git-common state directory and retry the accepted run",
            )
        })?;
        verify_destination_unchanged(
            &self.destination,
            &self.before,
            self.initially_absent,
        )?;
        if let Some(bytes) = bytes {
            atomic::Destination::new(&self.destination)
                .map_err(|error| {
                    StateError::current(
                        StateErrorKind::Persist,
                        &self.destination,
                        format!("could not prepare atomic state publication: {error}"),
                        "repair the Git-common state directory and retry the accepted run",
                    )
                })?
                .publish(|staged| staged.write_all(&bytes))
                .map_err(|error| {
                    StateError::current(
                        StateErrorKind::Persist,
                        &self.destination,
                        format!("could not atomically publish current state: {error}"),
                        "repair the Git-common state directory and retry the accepted run",
                    )
                })?;
        }
        Ok(StateReceipt::new(&self.before, &self.after, true))
    }
}

/// Immutable evidence for one proposed ledger transition.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StateReceipt {
    version: u64,
    before: Digest,
    after: Digest,
    accepted: bool,
}

impl StateReceipt {
    fn new(before: &StateLedger, after: &StateLedger, accepted: bool) -> Self {
        Self {
            version: STATE_RECEIPT_VERSION,
            before: before.digest(),
            after: after.digest(),
            accepted,
        }
    }

    /// Receipt schema admitted by this build.
    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    /// Exact logical ledger identity before the run.
    #[must_use]
    pub const fn before(&self) -> &Digest {
        &self.before
    }

    /// Exact proposed ledger identity after the run.
    #[must_use]
    pub const fn after(&self) -> &Digest {
        &self.after
    }

    /// Whether the after-ledger crossed the persistence boundary.
    #[must_use]
    pub const fn accepted(&self) -> bool {
        self.accepted
    }

    /// Canonical immutable receipt bytes.
    pub fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        wire::encode(self)
    }

    /// Decode only the current canonical receipt spelling.
    pub fn decode(bytes: &[u8]) -> Result<Self, StateError> {
        let source = Path::new("<state-receipt>");
        let version: VersionEvidence = wire::decode(bytes).map_err(|error| {
            StateError::current(
                StateErrorKind::Decode,
                source,
                format!("malformed state receipt JSON: {error}"),
                "retain the bytes as unsupported evidence or regenerate the receipt",
            )
        })?;
        if version.version != STATE_RECEIPT_VERSION {
            return Err(StateError::current(
                StateErrorKind::UnsupportedVersion,
                source,
                format!("unsupported state receipt version {}", version.version),
                "use a build that admits this receipt version; do not rewrite the bytes",
            ));
        }
        let raw: RawStateReceipt = wire::decode(bytes).map_err(|error| {
            StateError::current(
                StateErrorKind::Decode,
                source,
                format!("malformed state receipt JSON: {error}"),
                "retain the original bytes and repair the receipt producer",
            )
        })?;
        let receipt = Self {
            version: raw.version,
            before: raw.before,
            after: raw.after,
            accepted: raw.accepted,
        };
        let canonical = receipt.encode().map_err(|error| {
            StateError::current(
                StateErrorKind::Encode,
                source,
                format!("could not encode decoded state receipt: {error}"),
                "report the current receipt schema defect",
            )
        })?;
        if canonical != bytes {
            return Err(StateError::current(
                StateErrorKind::NonCanonical,
                source,
                "state receipt JSON is not canonical",
                "retain the original bytes as evidence; only current producers mint receipts",
            ));
        }
        Ok(receipt)
    }

    /// Domain-separated identity of the frozen receipt.
    #[must_use]
    pub fn digest(&self) -> Digest {
        let bytes = self
            .encode()
            .expect("the state receipt contains infallible JSON values");
        Digest::compute(Domain::GateStateReceipt, &bytes)
    }
}
