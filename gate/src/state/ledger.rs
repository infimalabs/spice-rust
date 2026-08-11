use super::*;

/// Which predecessor sticky ledger owns an entry.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LatchKind {
    FileLoc,
    FileBytes,
    ComplexityCcn,
    ComplexityLength,
    RepoDocChars,
}

impl LatchKind {
    pub(super) const fn admits(self, key: &StateKey) -> bool {
        match self {
            Self::FileLoc | Self::FileBytes | Self::RepoDocChars => key.is_path(),
            Self::ComplexityCcn | Self::ComplexityLength => key.is_function(),
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Serialized canonical path-key body.
pub struct PathKey {
    path: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Serialized canonical path/function-key body.
pub struct FunctionKey {
    path: String,
    function: String,
}

/// A sticky key in one of the two predecessor shapes.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StateKey {
    Function(FunctionKey),
    Path(PathKey),
}

impl StateKey {
    /// Construct a canonical path key.
    pub fn path(path: impl AsRef<str>) -> Result<Self, KeyError> {
        Ok(Self::Path(PathKey {
            path: normalize_repo_path(path.as_ref()).map_err(|reason| KeyError::InvalidPath {
                path: path.as_ref().to_owned(),
                reason,
            })?,
        }))
    }

    /// Construct a canonical path/function key.
    pub fn function(path: impl AsRef<str>, function: impl Into<String>) -> Result<Self, KeyError> {
        let function = function.into();
        if function.is_empty() {
            return Err(KeyError::EmptyFunction);
        }
        Ok(Self::Function(FunctionKey {
            path: normalize_repo_path(path.as_ref()).map_err(|reason| KeyError::InvalidPath {
                path: path.as_ref().to_owned(),
                reason,
            })?,
            function,
        }))
    }

    /// Canonical repository-relative path component.
    #[must_use]
    pub fn repo_path(&self) -> &str {
        match self {
            Self::Function(key) => &key.path,
            Self::Path(key) => &key.path,
        }
    }

    /// Function component when this is a path/function key.
    #[must_use]
    pub fn function_name(&self) -> Option<&str> {
        match self {
            Self::Function(key) => Some(&key.function),
            Self::Path(_) => None,
        }
    }

    const fn is_path(&self) -> bool {
        matches!(self, Self::Path(_))
    }

    const fn is_function(&self) -> bool {
        matches!(self, Self::Function(_))
    }

    pub(super) fn renamed(&self, path: &str) -> Self {
        match self {
            Self::Function(key) => Self::Function(FunctionKey {
                path: path.to_owned(),
                function: key.function.clone(),
            }),
            Self::Path(_) => Self::Path(PathKey {
                path: path.to_owned(),
            }),
        }
    }
}

/// One ledger-qualified sticky key.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LatchEntry {
    pub(super) kind: LatchKind,
    pub(super) key: StateKey,
}

impl LatchEntry {
    /// Validate that the key shape belongs to this ledger kind.
    pub fn new(kind: LatchKind, key: StateKey) -> Result<Self, KeyError> {
        if !kind.admits(&key) {
            return Err(KeyError::WrongShape { kind });
        }
        Ok(Self { kind, key })
    }

    /// Owning sticky ledger class.
    #[must_use]
    pub const fn kind(&self) -> LatchKind {
        self.kind
    }

    /// Path or path/function key.
    #[must_use]
    pub const fn key(&self) -> &StateKey {
        &self.key
    }
}

/// A malformed state key or rename map.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KeyError {
    InvalidPath { path: String, reason: String },
    EmptyFunction,
    WrongShape { kind: LatchKind },
    RenameCollision { path: String },
}

impl fmt::Display for KeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPath { path, reason } => {
                write!(formatter, "invalid repository path {path:?}: {reason}")
            }
            Self::EmptyFunction => formatter.write_str("function key must be non-empty"),
            Self::WrongShape { kind } => {
                write!(formatter, "key shape does not belong to {kind:?} ledger")
            }
            Self::RenameCollision { path } => {
                write!(formatter, "staged rename destination collides at {path:?}")
            }
        }
    }
}

impl Error for KeyError {}

/// One active shared flex claim.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerClaim {
    pub(super) path: String,
    pub(super) actor: String,
    pub(super) created_at: f64,
    pub(super) expires_at: f64,
}

impl PeerClaim {
    /// Canonical claimed path.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Claim owner.
    #[must_use]
    pub fn actor(&self) -> &str {
        &self.actor
    }

    /// Original ownership timestamp.
    #[must_use]
    pub const fn created_at(&self) -> f64 {
        self.created_at
    }

    /// Exclusive expiry timestamp.
    #[must_use]
    pub const fn expires_at(&self) -> f64 {
        self.expires_at
    }
}

/// Complete current Git-common state.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StateLedger {
    pub(super) version: u64,
    pub(super) latches: Vec<LatchEntry>,
    pub(super) claims: Vec<PeerClaim>,
}

impl StateLedger {
    /// Current schema number.
    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }

    /// Sticky entries in canonical order.
    #[must_use]
    pub fn latches(&self) -> &[LatchEntry] {
        &self.latches
    }

    /// Active claims in canonical order.
    #[must_use]
    pub fn claims(&self) -> &[PeerClaim] {
        &self.claims
    }

    /// Canonical current bytes.
    pub fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        wire::encode(self)
    }

    /// Domain-separated identity of the complete ledger.
    #[must_use]
    pub fn digest(&self) -> Digest {
        let bytes = self
            .encode()
            .expect("the state ledger contains only finite JSON values");
        Digest::compute(Domain::GateStateLedger, &bytes)
    }
}

/// Canonical staged old-to-new paths.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RenameMap {
    paths: BTreeMap<String, String>,
}

impl RenameMap {
    /// Normalize a complete staged rename set and refuse ambiguous targets.
    pub fn new<I, O, N>(renames: I) -> Result<Self, KeyError>
    where
        I: IntoIterator<Item = (O, N)>,
        O: AsRef<str>,
        N: AsRef<str>,
    {
        let mut paths = BTreeMap::new();
        let mut destinations = BTreeSet::new();
        for (old, new) in renames {
            let old = StateKey::path(old)?.repo_path().to_owned();
            let new = StateKey::path(new)?.repo_path().to_owned();
            if old == new {
                continue;
            }
            if paths.insert(old.clone(), new.clone()).is_some() {
                return Err(KeyError::RenameCollision { path: old });
            }
            if !destinations.insert(new.clone()) {
                return Err(KeyError::RenameCollision { path: new });
            }
        }
        Ok(Self { paths })
    }

    pub(crate) fn destination<'a>(&'a self, path: &'a str) -> &'a str {
        self.paths.get(path).map_or(path, String::as_str)
    }
}

/// One complete current observation used to reconcile a sticky entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LatchObservation {
    pub(super) entry: LatchEntry,
    pub(super) bound: BoundRule,
    pub(super) measured: u64,
    pub(super) actor_inputs: ActorInputs,
    pub(super) variation_percent: u64,
    pub(super) prior_latched: Option<bool>,
}

impl LatchObservation {
    /// Bind one measured key to its rule and actor evidence.
    pub fn new(
        entry: LatchEntry,
        bound: BoundRule,
        measured: u64,
        actor_inputs: ActorInputs,
        variation_percent: u64,
    ) -> Result<Self, BoundError> {
        if entry.key().repo_path() != actor_inputs.candidate_path() {
            return Err(BoundError::InvalidPath {
                path: actor_inputs.candidate_path().to_owned(),
                reason: format!(
                    "actor candidate path must match observed key path {:?}",
                    entry.key().repo_path()
                ),
            });
        }
        Ok(Self {
            entry,
            bound,
            measured,
            actor_inputs,
            variation_percent,
            prior_latched: None,
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn with_prior_latched(mut self, prior_latched: bool) -> Self {
        self.prior_latched = Some(prior_latched);
        self
    }

    /// Sticky key this observation reconciles.
    #[must_use]
    pub const fn entry(&self) -> &LatchEntry {
        &self.entry
    }
}

/// One latch-qualified decision returned by reconciliation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StateDecision {
    pub(super) entry: LatchEntry,
    pub(super) bound: BoundDecision,
}

impl StateDecision {
    /// Sticky entry this decision controls.
    #[must_use]
    pub const fn entry(&self) -> &LatchEntry {
        &self.entry
    }

    /// Complete bound decision.
    #[must_use]
    pub const fn bound(&self) -> &BoundDecision {
        &self.bound
    }
}
