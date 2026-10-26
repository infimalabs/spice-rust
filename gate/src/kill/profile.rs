use super::*;

/// The pinned mutation executable and its complete explicit environment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MutationTool {
    pub(super) cargo_program: PathBuf,
    mutants_program: PathBuf,
    identity: String,
    environment: Environment,
}

impl MutationTool {
    /// Declare direct paths for Cargo and cargo-mutants and their environment.
    ///
    /// # Errors
    ///
    /// Refuses an empty executable or any environment key the gate owns.
    pub fn new(
        cargo_program: impl Into<PathBuf>,
        mutants_program: impl Into<PathBuf>,
        identity: impl Into<String>,
        environment: Environment,
    ) -> Result<Self, RateError> {
        let cargo_program = cargo_program.into();
        let mutants_program = mutants_program.into();
        let identity = identity.into();
        if cargo_program.as_os_str().is_empty() || mutants_program.as_os_str().is_empty() {
            return Err(RateError::EmptyProgram);
        }
        if !cargo_program.is_absolute() {
            return Err(RateError::NonAbsoluteProgram {
                program: cargo_program,
            });
        }
        if !mutants_program.is_absolute() {
            return Err(RateError::NonAbsoluteProgram {
                program: mutants_program,
            });
        }
        check_identity(&identity)?;
        for (key, _) in environment.entries() {
            if GATE_OWNED_ENVIRONMENT
                .iter()
                .any(|owned| key == OsStr::new(owned))
            {
                return Err(RateError::ReservedEnvironment {
                    key: key.to_string_lossy().into_owned(),
                });
            }
        }
        Ok(Self {
            cargo_program,
            mutants_program,
            identity,
            environment,
        })
    }

    /// Resolve the declared programs on the host that runs the battery.
    ///
    /// Every program resolves through the rim's directory and search path,
    /// exactly as the compile arms resolve theirs. The process environment is
    /// the inherited compiler environment with a hermetic `PATH` holding only
    /// the resolved programs' directories, plus the fixed entries.
    ///
    /// # Errors
    ///
    /// Names the first declared program the rim does not resolve, or refuses
    /// an environment the resolved programs cannot form.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn resolve(
        identity: &str,
        rim: &crate::toolchain::RimCapture,
        inherited: &[(OsString, OsString)],
    ) -> Result<Self, KillError> {
        let resolve = |program: &'static str| {
            rim.resolve(program)
                .ok_or(KillError::ToolMissing { program })
        };
        let cargo_program = resolve(CARGO_PROGRAM)?;
        let mutants_program = resolve(MUTANTS_PROGRAM)?;
        let mut programs = vec![cargo_program.clone(), mutants_program.clone()];
        for program in NATIVE_BUILD_PROGRAMS {
            programs.push(resolve(program)?);
        }
        let search_path = crate::toolchain::tool_search_path("mutation", &programs)
            .map_err(KillError::ToolEnvironment)?;
        let mut entries = inherited
            .iter()
            .filter(|(name, _)| {
                name != OsStr::new(SEARCH_PATH_ENVIRONMENT)
                    && !GATE_OWNED_ENVIRONMENT
                        .iter()
                        .any(|owned| name == OsStr::new(owned))
            })
            .cloned()
            .collect::<std::collections::BTreeMap<_, _>>();
        entries.insert(OsString::from(SEARCH_PATH_ENVIRONMENT), search_path);
        for (name, value) in FIXED_ENVIRONMENT {
            entries.insert(OsString::from(name), OsString::from(value));
        }
        let environment = Environment::new(entries)
            .map_err(|error| KillError::ToolEnvironment(error.to_string()))?;
        Self::new(cargo_program, mutants_program, identity, environment)
            .map_err(|error| KillError::ToolEnvironment(error.to_string()))
    }

    /// Render the host-independent declaration: program names, the pinned
    /// identity, and how each environment entry is formed on the running host.
    pub(crate) fn render_declaration(identity: &str, into: &mut String) {
        use std::fmt::Write as _;

        let _ = writeln!(into, "kill cargo-program {CARGO_PROGRAM}");
        let _ = writeln!(into, "kill mutants-program {MUTANTS_PROGRAM}");
        for program in NATIVE_BUILD_PROGRAMS {
            let _ = writeln!(into, "kill native-program {program}");
        }
        let _ = writeln!(into, "kill identity {identity}");
        let _ = writeln!(into, "kill environment inherited compiler-environment");
        let _ = writeln!(
            into,
            "kill environment {SEARCH_PATH_ENVIRONMENT} resolved-program-directories"
        );
        for (name, value) in FIXED_ENVIRONMENT {
            let _ = writeln!(into, "kill environment {name} {value}");
        }
        render_owned_environment("resolved-cargo-program", into);
    }

    /// Direct Cargo executable used for the unmutated baseline.
    #[must_use]
    pub fn cargo_program(&self) -> &Path {
        &self.cargo_program
    }

    /// Direct cargo-mutants executable used for attestation and mutation.
    #[must_use]
    pub fn mutants_program(&self) -> &Path {
        &self.mutants_program
    }

    /// Exact version line required from the direct cargo-mutants executable.
    #[must_use]
    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// Explicit base environment before the gate-owned execution fields.
    #[must_use]
    pub const fn base_environment(&self) -> &Environment {
        &self.environment
    }

    pub(crate) fn render(&self, into: &mut String) {
        use std::fmt::Write as _;

        let _ = writeln!(into, "kill cargo-program {}", self.cargo_program.display());
        let _ = writeln!(
            into,
            "kill mutants-program {}",
            self.mutants_program.display()
        );
        let _ = writeln!(into, "kill identity {}", self.identity);
        let _ = writeln!(into, "kill environment clear");
        for (name, value) in self.environment.entries() {
            let _ = writeln!(
                into,
                "kill environment {} {}",
                name.to_string_lossy(),
                value.to_string_lossy()
            );
        }
        render_owned_environment(self.cargo_program.display(), into);
    }

    pub(super) fn identify(&self) -> Result<MutationToolIdentity, KillError> {
        let declaration = {
            let mut rendered = String::new();
            self.render(&mut rendered);
            Digest::compute(Domain::GateTool, rendered.as_bytes())
        };
        let cargo = executable_digest("cargo", &self.cargo_program)?;
        let mutants = executable_digest("cargo-mutants", &self.mutants_program)?;
        Ok(MutationToolIdentity::from_parts(
            self.identity.clone(),
            declaration,
            cargo,
            mutants,
        ))
    }

    pub(super) fn environment(
        &self,
        build_root: &Path,
        seed: u64,
        attempt: Option<usize>,
    ) -> Result<Environment, SpecError> {
        let mut entries = self.environment.entries().to_vec();
        entries.push((
            OsString::from(CARGO_ENVIRONMENT),
            self.cargo_program.as_os_str().to_owned(),
        ));
        entries.push((
            OsString::from(CARGO_BUILD_DIRECTORY_ENVIRONMENT),
            build_root.join(MUTATION_BUILD_DIRECTORY).into_os_string(),
        ));
        entries.push((
            OsString::from(CARGO_TARGET_DIRECTORY_ENVIRONMENT),
            build_root.join(MUTATION_TARGET_DIRECTORY).into_os_string(),
        ));
        entries.push((
            OsString::from(SEED_ENVIRONMENT),
            OsString::from(seed.to_string()),
        ));
        if let Some(attempt) = attempt {
            entries.push((
                OsString::from(ATTEMPT_ENVIRONMENT),
                OsString::from(attempt.to_string()),
            ));
        }
        Environment::new(entries)
    }
}

/// Check one exact pinned version line.
pub(crate) fn check_identity(identity: &str) -> Result<(), RateError> {
    if identity.is_empty() || identity.contains(['\n', '\r']) {
        return Err(RateError::InvalidToolIdentity {
            identity: identity.to_owned(),
        });
    }
    Ok(())
}

/// Render the entries the gate sets on every mutation process.
fn render_owned_environment(cargo: impl std::fmt::Display, into: &mut String) {
    use std::fmt::Write as _;

    let _ = writeln!(into, "kill environment {CARGO_ENVIRONMENT} {cargo}");
    let _ = writeln!(
        into,
        "kill environment {CARGO_BUILD_DIRECTORY_ENVIRONMENT} \
         selected-snapshot/{MUTATION_BUILD_DIRECTORY}"
    );
    let _ = writeln!(
        into,
        "kill environment {CARGO_TARGET_DIRECTORY_ENVIRONMENT} \
         selected-snapshot/{MUTATION_TARGET_DIRECTORY}"
    );
    let _ = writeln!(into, "kill environment {SEED_ENVIRONMENT} compiled-seed");
    let _ = writeln!(
        into,
        "kill environment {ATTEMPT_ENVIRONMENT} one-based-attempt"
    );
}

/// Content-bound identity of the exact mutation executables and declaration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MutationToolIdentity {
    expected: String,
    declaration: Digest,
    cargo: Digest,
    mutants: Digest,
    identity: Digest,
}

impl MutationToolIdentity {
    pub(crate) fn from_parts(
        expected: String,
        declaration: Digest,
        cargo: Digest,
        mutants: Digest,
    ) -> Self {
        let identity = operational_identity(&expected, &declaration, &cargo, &mutants);
        Self {
            expected,
            declaration,
            cargo,
            mutants,
            identity,
        }
    }

    /// Semantic version line required from cargo-mutants.
    #[must_use]
    pub fn expected(&self) -> &str {
        &self.expected
    }

    /// Identity of the complete executable and environment declaration.
    #[must_use]
    pub const fn declaration(&self) -> &Digest {
        &self.declaration
    }

    /// Content identity of the direct Cargo executable.
    #[must_use]
    pub const fn cargo(&self) -> &Digest {
        &self.cargo
    }

    /// Content identity of the direct cargo-mutants executable.
    #[must_use]
    pub const fn mutants(&self) -> &Digest {
        &self.mutants
    }

    /// Composite identity recomputed from every preceding field.
    #[must_use]
    pub const fn identity(&self) -> &Digest {
        &self.identity
    }

    pub(crate) fn coherent(&self) -> bool {
        self.identity
            == operational_identity(
                &self.expected,
                &self.declaration,
                &self.cargo,
                &self.mutants,
            )
    }
}

fn executable_digest(role: &'static str, program: &Path) -> Result<Digest, KillError> {
    let bytes = fs::read(program).map_err(|source| KillError::ToolRead {
        role,
        program: program.to_path_buf(),
        source,
    })?;
    Ok(Digest::compute(Domain::GateTool, &bytes))
}

fn operational_identity(
    expected: &str,
    declaration: &Digest,
    cargo: &Digest,
    mutants: &Digest,
) -> Digest {
    use std::fmt::Write as _;

    let mut rendered = String::from("mutation-tool-identity 1\n");
    let _ = writeln!(rendered, "expected {expected}");
    let _ = writeln!(rendered, "declaration {declaration}");
    let _ = writeln!(rendered, "cargo {cargo}");
    let _ = writeln!(rendered, "cargo-mutants {mutants}");
    Digest::compute(Domain::GateTool, rendered.as_bytes())
}

/// Stable target selection, retry count, and seed for one mutation battery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MutationBattery {
    pub(super) targets: Vec<String>,
    pub(super) attempts: usize,
    pub(super) seed: u64,
}

impl MutationBattery {
    /// Declare a non-empty target set and positive attempt count.
    ///
    /// Target and input order do not influence the stored selection: targets
    /// are sorted, and duplicates are refused rather than silently collapsed.
    ///
    /// # Errors
    ///
    /// Refuses empty, line-breaking, repeated targets and zero attempts.
    pub fn new(
        targets: impl IntoIterator<Item = impl Into<String>>,
        attempts: usize,
        seed: u64,
    ) -> Result<Self, RateError> {
        if attempts == 0 {
            return Err(RateError::ZeroAttempts);
        }
        let mut targets: Vec<String> = targets.into_iter().map(Into::into).collect();
        if targets.is_empty() {
            return Err(RateError::EmptyTargets);
        }
        for target in &targets {
            if target.is_empty() || target.contains(['\n', '\r']) {
                return Err(RateError::InvalidTarget {
                    target: target.clone(),
                });
            }
        }
        targets.sort();
        if let Some(pair) = targets.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(RateError::DuplicateTarget {
                target: pair[0].clone(),
            });
        }
        Ok(Self {
            targets,
            attempts,
            seed,
        })
    }

    /// Selected file targets in canonical order.
    #[must_use]
    pub fn targets(&self) -> &[String] {
        &self.targets
    }

    /// Attempts required for every mutant.
    #[must_use]
    pub const fn attempts(&self) -> usize {
        self.attempts
    }

    /// Seed repeated into every bounded process environment.
    #[must_use]
    pub const fn seed(&self) -> u64 {
        self.seed
    }
}

/// Sill bounds shared by tool attestation, inventory, baseline, and attempts.
#[derive(Clone, Debug)]
pub struct KillBounds {
    pub(super) deadlines: Deadlines,
    pub(super) wait: WaitPolicy,
    pub(super) grace: Duration,
    pub(super) capture: usize,
}

impl KillBounds {
    /// Declare positive runtime and capture bounds.
    ///
    /// # Errors
    ///
    /// Refuses zero runtime or capture, and forwards invalid wait ownership.
    pub fn new(
        owner: impl Into<String>,
        deadlines: Deadlines,
        grace: Duration,
        capture: usize,
    ) -> Result<Self, RateError> {
        if deadlines.runtime().is_zero() {
            return Err(RateError::ZeroRuntime);
        }
        if capture == 0 {
            return Err(RateError::ZeroCapture);
        }
        let wait = WaitPolicy::new(owner, Cancellation::Never, RestartDisposition::Never)
            .map_err(RateError::Spec)?;
        Ok(Self {
            deadlines,
            wait,
            grace,
            capture,
        })
    }

    pub(super) fn timeout_seconds(&self) -> u64 {
        let runtime = self.deadlines.runtime();
        runtime
            .as_secs()
            .saturating_add(u64::from(runtime.subsec_nanos() != 0))
            .max(1)
    }

    /// Runtime, output-drain, and reap deadlines for every process.
    #[must_use]
    pub const fn deadlines(&self) -> Deadlines {
        self.deadlines
    }

    /// Grace between termination and forced termination.
    #[must_use]
    pub const fn grace(&self) -> Duration {
        self.grace
    }

    /// Maximum bytes retained for each output stream.
    #[must_use]
    pub const fn capture(&self) -> usize {
        self.capture
    }
}
