#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStateReceipt {
    version: u64,
    before: Digest,
    after: Digest,
    accepted: bool,
}

#[derive(Deserialize)]
struct VersionEvidence {
    version: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStateLedger {
    version: u64,
    latches: Vec<LatchEntry>,
    claims: Vec<PeerClaim>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPathLedger {
    #[serde(rename = "version")]
    _version: u64,
    paths: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFunctionLedger {
    #[serde(rename = "version")]
    _version: u64,
    functions: Vec<(String, String)>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawClaimLedger {
    #[serde(rename = "version")]
    _version: u64,
    ttl_seconds: u64,
    claims: Vec<PeerClaim>,
}

/// Stable state load/import/persist refusal category.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateErrorKind {
    Read,
    Decode,
    UnsupportedVersion,
    Malformed,
    Collision,
    Stale,
    NonCanonical,
    MissingActor,
    Encode,
    Persist,
}

impl fmt::Display for StateErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Read => "read",
            Self::Decode => "decode",
            Self::UnsupportedVersion => "unsupported-version",
            Self::Malformed => "malformed",
            Self::Collision => "collision",
            Self::Stale => "stale",
            Self::NonCanonical => "non-canonical",
            Self::MissingActor => "missing-actor",
            Self::Encode => "encode",
            Self::Persist => "persist",
        })
    }
}

/// One loud state failure with exact ownership and recovery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StateError {
    kind: StateErrorKind,
    source_path: PathBuf,
    release: String,
    message: String,
    recovery: String,
}

impl StateError {
    fn current(
        kind: StateErrorKind,
        source_path: &Path,
        message: impl Into<String>,
        recovery: impl Into<String>,
    ) -> Self {
        Self::new(kind, source_path, CURRENT_STATE_RELEASE, message, recovery)
    }

    fn predecessor(
        kind: StateErrorKind,
        source_path: &Path,
        message: impl Into<String>,
        recovery: impl Into<String>,
    ) -> Self {
        Self::new(
            kind,
            source_path,
            PREDECESSOR_STATE_RELEASE,
            message,
            recovery,
        )
    }

    fn new(
        kind: StateErrorKind,
        source_path: &Path,
        release: &str,
        message: impl Into<String>,
        recovery: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            source_path: source_path.to_path_buf(),
            release: release.to_owned(),
            message: message.into(),
            recovery: recovery.into(),
        }
    }

    /// Stable failure category.
    #[must_use]
    pub const fn kind(&self) -> StateErrorKind {
        self.kind
    }

    /// Exact state source or destination path.
    #[must_use]
    pub fn source_path(&self) -> &Path {
        &self.source_path
    }

    /// Release owning the failed spelling.
    #[must_use]
    pub fn release(&self) -> &str {
        &self.release
    }

    /// Concrete recovery that preserves prior evidence.
    #[must_use]
    pub fn recovery(&self) -> &str {
        &self.recovery
    }
}

impl fmt::Display for StateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "gate state release={} kind={} source={}: {}; recovery={}",
            self.release,
            self.kind,
            self.source_path.display(),
            self.message,
            self.recovery
        )
    }
}

impl Error for StateError {}

fn decode_current(path: &Path, bytes: &[u8]) -> Result<StateLedger, StateError> {
    let version: VersionEvidence = wire::decode(bytes).map_err(|error| {
        StateError::current(
            StateErrorKind::Decode,
            path,
            format!("malformed current ledger JSON: {error}"),
            "restore a valid current ledger from evidence; do not fall back to predecessor state",
        )
    })?;
    if version.version != STATE_LEDGER_VERSION {
        return Err(StateError::current(
            StateErrorKind::UnsupportedVersion,
            path,
            format!("unsupported current ledger version {}", version.version),
            "use a build that admits this ledger version; do not rewrite it",
        ));
    }
    let raw: RawStateLedger = wire::decode(bytes).map_err(|error| {
        StateError::current(
            StateErrorKind::Decode,
            path,
            format!("malformed current ledger JSON: {error}"),
            "restore canonical ledger bytes whose digest matches the last accepted receipt",
        )
    })?;
    let ledger = StateLedger {
        version: raw.version,
        latches: raw.latches,
        claims: raw.claims,
    };
    validate_ledger(&ledger, path, CURRENT_STATE_RELEASE)?;
    let canonical = ledger.encode().map_err(|error| {
        StateError::current(
            StateErrorKind::Encode,
            path,
            format!("could not encode decoded current ledger: {error}"),
            "report the current ledger schema defect",
        )
    })?;
    if canonical != bytes {
        return Err(StateError::current(
            StateErrorKind::NonCanonical,
            path,
            "current ledger JSON is not canonical",
            "restore canonical bytes whose digest matches accepted transition evidence",
        ));
    }
    Ok(ledger)
}

fn verify_destination_unchanged(
    path: &Path,
    before: &StateLedger,
    imported: bool,
) -> Result<(), StateError> {
    let observed = read_optional(path, CURRENT_STATE_RELEASE)?;
    match (imported, observed) {
        (true, None) => Ok(()),
        (true, Some(_)) => Err(StateError::current(
            StateErrorKind::Stale,
            path,
            "current state appeared after predecessor import began",
            "reload current state and replay the run against its accepted ledger",
        )),
        (false, None) => Err(StateError::current(
            StateErrorKind::Stale,
            path,
            "current state disappeared after this run loaded it",
            "restore or reload current state before retrying the run",
        )),
        (false, Some(bytes)) => {
            let observed = decode_current(path, &bytes)?;
            if observed.digest() == before.digest() {
                Ok(())
            } else {
                Err(StateError::current(
                    StateErrorKind::Stale,
                    path,
                    "current state changed after this run loaded it",
                    "reload current state and replay the run against its accepted ledger",
                ))
            }
        }
    }
}

fn import_predecessor(sources: &PredecessorSources) -> Result<StateLedger, StateError> {
    let mut latches = BTreeSet::new();
    import_path_ledger(&sources.file_loc, LatchKind::FileLoc, &mut latches)?;
    import_path_ledger(&sources.file_bytes, LatchKind::FileBytes, &mut latches)?;
    import_function_ledger(
        &sources.complexity_ccn,
        LatchKind::ComplexityCcn,
        &mut latches,
    )?;
    import_function_ledger(
        &sources.complexity_length,
        LatchKind::ComplexityLength,
        &mut latches,
    )?;
    import_path_ledger(
        &sources.repo_doc_chars,
        LatchKind::RepoDocChars,
        &mut latches,
    )?;
    let claims = import_claim_ledger(&sources.claims)?;
    Ok(StateLedger {
        version: STATE_LEDGER_VERSION,
        latches: latches.into_iter().collect(),
        claims,
    })
}

fn import_path_ledger(
    path: &Path,
    kind: LatchKind,
    target: &mut BTreeSet<LatchEntry>,
) -> Result<(), StateError> {
    let Some(bytes) = read_optional(path, PREDECESSOR_STATE_RELEASE)? else {
        return Ok(());
    };
    ensure_predecessor_version(path, &bytes)?;
    let raw: RawPathLedger = wire::decode(&bytes).map_err(|error| {
        StateError::predecessor(
            StateErrorKind::Decode,
            path,
            format!("malformed predecessor path ledger: {error}"),
            "repair or remove this predecessor ledger before retrying import",
        )
    })?;
    let mut local = BTreeSet::new();
    for raw_path in raw.paths {
        let key = StateKey::path(&raw_path).map_err(|error| {
            StateError::predecessor(
                StateErrorKind::Malformed,
                path,
                error.to_string(),
                "repair the malformed path key in this predecessor ledger",
            )
        })?;
        let entry = LatchEntry::new(kind, key).expect("path ledgers admit path keys");
        if !local.insert(entry.clone()) {
            return Err(StateError::predecessor(
                StateErrorKind::Collision,
                path,
                format!("path keys collide after normalization at {raw_path:?}"),
                "deduplicate the predecessor ledger without changing its live latch meaning",
            ));
        }
        if !target.insert(entry) {
            return Err(StateError::predecessor(
                StateErrorKind::Collision,
                path,
                format!("duplicate imported {kind:?} latch at {raw_path:?}"),
                "retain exactly one source entry for this sticky key",
            ));
        }
    }
    Ok(())
}

fn import_function_ledger(
    path: &Path,
    kind: LatchKind,
    target: &mut BTreeSet<LatchEntry>,
) -> Result<(), StateError> {
    let Some(bytes) = read_optional(path, PREDECESSOR_STATE_RELEASE)? else {
        return Ok(());
    };
    ensure_predecessor_version(path, &bytes)?;
    let raw: RawFunctionLedger = wire::decode(&bytes).map_err(|error| {
        StateError::predecessor(
            StateErrorKind::Decode,
            path,
            format!("malformed predecessor function ledger: {error}"),
            "repair or remove this predecessor ledger before retrying import",
        )
    })?;
    let mut local = BTreeSet::new();
    for (raw_path, function) in raw.functions {
        let key = StateKey::function(&raw_path, function.clone()).map_err(|error| {
            StateError::predecessor(
                StateErrorKind::Malformed,
                path,
                error.to_string(),
                "repair the malformed path/function key in this predecessor ledger",
            )
        })?;
        let entry = LatchEntry::new(kind, key).expect("function ledgers admit function keys");
        if !local.insert(entry.clone()) {
            return Err(StateError::predecessor(
                StateErrorKind::Collision,
                path,
                format!(
                    "path/function keys collide after normalization at {raw_path:?}, {function:?}"
                ),
                "deduplicate the predecessor ledger without changing its live latch meaning",
            ));
        }
        if !target.insert(entry) {
            return Err(StateError::predecessor(
                StateErrorKind::Collision,
                path,
                format!("duplicate imported {kind:?} function latch"),
                "retain exactly one source entry for this sticky key",
            ));
        }
    }
    Ok(())
}

fn import_claim_ledger(path: &Path) -> Result<Vec<PeerClaim>, StateError> {
    let Some(bytes) = read_optional(path, PREDECESSOR_STATE_RELEASE)? else {
        return Ok(Vec::new());
    };
    ensure_predecessor_version(path, &bytes)?;
    let raw: RawClaimLedger = wire::decode(&bytes).map_err(|error| {
        StateError::predecessor(
            StateErrorKind::Decode,
            path,
            format!("malformed predecessor claim ledger: {error}"),
            "repair or remove this predecessor claim ledger before retrying import",
        )
    })?;
    if raw.ttl_seconds != PEER_CLAIM_TTL_SECONDS {
        return Err(StateError::predecessor(
            StateErrorKind::Malformed,
            path,
            format!(
                "claim TTL {} has no exact current meaning; expected {}",
                raw.ttl_seconds, PEER_CLAIM_TTL_SECONDS
            ),
            "expire the incompatible claims or restore the release-owned TTL",
        ));
    }
    let mut claims = Vec::with_capacity(raw.claims.len());
    for mut claim in raw.claims {
        let raw_path = claim.path.clone();
        claim.path = normalize_repo_path(&raw_path).map_err(|reason| {
            StateError::predecessor(
                StateErrorKind::Malformed,
                path,
                format!("invalid predecessor claim path {raw_path:?}: {reason}"),
                "repair the malformed path in this predecessor claim ledger",
            )
        })?;
        claim.actor = claim.actor.trim().to_owned();
        claims.push(claim);
    }
    validate_claims(&claims, path, PREDECESSOR_STATE_RELEASE, false)?;
    sort_claims(&mut claims);
    Ok(claims)
}

fn ensure_predecessor_version(path: &Path, bytes: &[u8]) -> Result<(), StateError> {
    let version: VersionEvidence = wire::decode(bytes).map_err(|error| {
        StateError::predecessor(
            StateErrorKind::Decode,
            path,
            format!("malformed predecessor ledger JSON: {error}"),
            "repair the release-owned JSON before retrying import",
        )
    })?;
    if version.version != 1 {
        return Err(StateError::predecessor(
            StateErrorKind::UnsupportedVersion,
            path,
            format!("unsupported predecessor ledger version {}", version.version),
            "use a release-specific migrator or restore a version 1 predecessor ledger",
        ));
    }
    Ok(())
}

fn read_optional(path: &Path, release: &str) -> Result<Option<Vec<u8>>, StateError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(StateError::new(
                StateErrorKind::Read,
                path,
                release,
                format!("could not inspect state: {error}"),
                "repair path permissions or restore the state file before retrying",
            ));
        }
    };
    if !metadata.file_type().is_file() {
        return Err(StateError::new(
            StateErrorKind::Read,
            path,
            release,
            "state source is not a regular file",
            "replace the source with the release-owned regular state file",
        ));
    }
    fs::read(path).map(Some).map_err(|error| {
        StateError::new(
            StateErrorKind::Read,
            path,
            release,
            format!("could not read state: {error}"),
            "repair path permissions or restore the state file before retrying",
        )
    })
}

fn validate_ledger(ledger: &StateLedger, path: &Path, release: &str) -> Result<(), StateError> {
    for entry in &ledger.latches {
        if !entry.kind.admits(&entry.key) {
            return Err(StateError::new(
                StateErrorKind::Malformed,
                path,
                release,
                format!("key shape does not belong to {:?}", entry.kind),
                "restore a key shape admitted by its sticky ledger kind",
            ));
        }
        let normalized = normalize_repo_path(entry.key.repo_path()).map_err(|reason| {
            StateError::new(
                StateErrorKind::Malformed,
                path,
                release,
                format!("invalid latch path {:?}: {reason}", entry.key.repo_path()),
                "restore a canonical repository-relative latch key",
            )
        })?;
        if normalized != entry.key.repo_path() {
            return Err(StateError::new(
                StateErrorKind::NonCanonical,
                path,
                release,
                format!("latch path {:?} is not canonical", entry.key.repo_path()),
                "restore canonical current ledger bytes",
            ));
        }
        if entry.key.function_name().is_some_and(str::is_empty) {
            return Err(StateError::new(
                StateErrorKind::Malformed,
                path,
                release,
                "function key is empty",
                "restore the exact non-empty function identity",
            ));
        }
    }
    if ledger.latches.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(StateError::new(
            StateErrorKind::Collision,
            path,
            release,
            "current latch keys are duplicated or out of canonical order",
            "restore the canonical accepted ledger bytes",
        ));
    }
    validate_claims(&ledger.claims, path, release, true)
}

fn validate_claims(
    claims: &[PeerClaim],
    path: &Path,
    release: &str,
    require_order: bool,
) -> Result<(), StateError> {
    let mut keys = BTreeSet::new();
    for claim in claims {
        let normalized = normalize_repo_path(&claim.path).map_err(|reason| {
            StateError::new(
                StateErrorKind::Malformed,
                path,
                release,
                format!("invalid claim path {:?}: {reason}", claim.path),
                "repair the claim to a canonical repository-relative path",
            )
        })?;
        if normalized != claim.path {
            return Err(StateError::new(
                StateErrorKind::NonCanonical,
                path,
                release,
                format!("claim path {:?} is not canonical", claim.path),
                "normalize the claim path without changing its ownership meaning",
            ));
        }
        if claim.actor.trim().is_empty() || claim.actor.trim() != claim.actor {
            return Err(StateError::new(
                StateErrorKind::Malformed,
                path,
                release,
                "claim actor must be non-empty and trimmed",
                "restore the exact non-empty actor identity",
            ));
        }
        if !claim.created_at.is_finite()
            || !claim.expires_at.is_finite()
            || claim.expires_at < claim.created_at
        {
            return Err(StateError::new(
                StateErrorKind::Malformed,
                path,
                release,
                "claim timestamps must be finite and expiry must not precede creation",
                "repair or expire the malformed claim",
            ));
        }
        if !keys.insert((claim.path.clone(), claim.actor.clone())) {
            return Err(StateError::new(
                StateErrorKind::Collision,
                path,
                release,
                format!(
                    "multiple claims collide for path {:?} actor {:?}",
                    claim.path, claim.actor
                ),
                "retain exactly one claim for this path and actor",
            ));
        }
    }
    if require_order
        && claims
            .windows(2)
            .any(|pair| compare_claims(&pair[0], &pair[1]) != Ordering::Less)
    {
        return Err(StateError::new(
            StateErrorKind::NonCanonical,
            path,
            release,
            "current claims are not in canonical order",
            "restore the exact bytes published by an accepted current run",
        ));
    }
    Ok(())
}

fn owning_claim<'a>(claims: &'a [PeerClaim], path: &str) -> Option<&'a PeerClaim> {
    claims
        .iter()
        .filter(|claim| claim.path == path)
        .min_by(|left, right| {
            left.created_at
                .total_cmp(&right.created_at)
                .then_with(|| left.actor.cmp(&right.actor))
        })
}

fn sort_claims(claims: &mut [PeerClaim]) {
    claims.sort_by(compare_claims);
}

fn compare_claims(left: &PeerClaim, right: &PeerClaim) -> Ordering {
    left.path
        .cmp(&right.path)
        .then_with(|| left.actor.cmp(&right.actor))
        .then_with(|| left.created_at.total_cmp(&right.created_at))
        .then_with(|| left.expires_at.total_cmp(&right.expires_at))
}

fn normalize_repo_path(raw: &str) -> Result<String, String> {
    let normalized = raw.trim().replace('\\', "/");
    let path = Path::new(&normalized);
    if normalized.is_empty() || path.is_absolute() {
        return Err("path must be non-empty and repository-relative".to_owned());
    }
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let part = part
                    .to_str()
                    .ok_or_else(|| "path must be valid UTF-8".to_owned())?;
                if part.is_empty() {
                    continue;
                }
                parts.push(part);
            }
            Component::CurDir => {}
            Component::ParentDir => {
                return Err("path must not contain a parent component".to_owned());
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err("path must be repository-relative".to_owned());
            }
        }
    }
    if parts.is_empty() {
        return Err("path must identify a repository entry, not the root".to_owned());
    }
    Ok(parts.join("/"))
}
