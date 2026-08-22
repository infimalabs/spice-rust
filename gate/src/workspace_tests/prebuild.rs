use super::*;

pub(super) fn prebuild_all_tests(
    root: &Path,
    environment: &Environment,
    cargo: &OsStr,
    inventory: &Inventory,
    output: &mut impl Write,
) -> Result<(), WorkspaceTestError> {
    let features = qualified_required_features(inventory.packages.iter().flat_map(|package| {
        package.targets.iter().flat_map(|target| {
            target
                .required_features
                .iter()
                .map(|feature| (package.name.as_str(), feature.as_str()))
        })
    }));
    let mut arguments = ["test", "--locked", "--workspace", "--all-targets"]
        .into_iter()
        .map(OsString::from)
        .collect::<Vec<_>>();
    let mut command = "cargo test --locked --workspace --all-targets".to_owned();
    if let Some(features) = features {
        arguments.extend([OsString::from("--features"), OsString::from(&features)]);
        command.push_str(" --features ");
        command.push_str(&features);
    }
    arguments.push(OsString::from("--no-run"));
    command.push_str(" --no-run");
    run_prebuild_command(
        root,
        environment,
        cargo,
        output,
        &command,
        arguments,
        "workspace-test prebuild",
    )?;
    repair_missing_binary_outputs(root, environment, cargo, inventory, output)
}

pub(super) fn prebuild_selected_tests(
    root: &Path,
    environment: &Environment,
    cargo: &OsStr,
    inventory: &Inventory,
    budget: Option<WorkspaceTestBudget>,
    concurrency: usize,
    output: &mut impl Write,
) -> Result<Vec<PrebuiltTest>, WorkspaceTestError> {
    let batches = selected_prebuild_batches(inventory)?;
    let lanes = concurrency.min(batches.len());
    let environments = match budget {
        Some(budget) => budget
            .prebuild_job_grants(lanes)
            .into_iter()
            .map(|jobs| with_cargo_build_jobs(environment, jobs))
            .collect::<Result<Vec<_>, _>>()?,
        None => vec![environment.clone(); lanes],
    };
    debug_assert_eq!(environments.len(), lanes);
    let artifacts = run_selected_prebuild_batches(&batches, concurrency, output, |batch, lane| {
        let mut progress = Vec::new();
        let artifacts = run_prebuild_command_capture(
            root,
            &environments[lane],
            cargo,
            &mut progress,
            &batch.command,
            batch.arguments.clone(),
            &batch.owner,
        );
        SelectedPrebuildResult {
            progress,
            artifacts,
        }
    })?;
    repair_missing_binary_outputs(root, environment, cargo, inventory, output)?;
    prebuilt_test_plan(inventory, &artifacts)
}

#[derive(Clone, Debug)]
pub(super) struct SelectedPrebuildBatch {
    pub(super) arguments: Vec<OsString>,
    command: String,
    owner: String,
    targets: usize,
}

pub(super) struct SelectedPrebuildResult {
    pub(super) progress: Vec<u8>,
    pub(super) artifacts: Result<Vec<u8>, WorkspaceTestError>,
}

pub(super) fn selected_prebuild_batches(
    inventory: &Inventory,
) -> Result<Vec<SelectedPrebuildBatch>, WorkspaceTestError> {
    let mut batches = Vec::new();
    for package in &inventory.packages {
        let mut arguments = ["test", "--locked", "--package"]
            .into_iter()
            .map(OsString::from)
            .collect::<Vec<_>>();
        arguments.push(OsString::from(&package.id));
        for target in &package.targets {
            arguments.extend(target_arguments(target)?);
        }
        if let Some(features) =
            qualified_required_features(package.targets.iter().flat_map(|target| {
                target
                    .required_features
                    .iter()
                    .map(|feature| (package.name.as_str(), feature.as_str()))
            }))
        {
            arguments.extend([OsString::from("--features"), OsString::from(features)]);
        }
        arguments.extend([
            OsString::from("--no-run"),
            OsString::from("--message-format=json-render-diagnostics"),
        ]);
        let command = std::iter::once(OsStr::new("cargo"))
            .chain(arguments.iter().map(OsString::as_os_str))
            .map(|argument| format!("{argument:?}"))
            .collect::<Vec<_>>()
            .join(" ");
        let owner = format!(
            "selected workspace-test prebuild for package {:?} with {} exact target(s)",
            package.name,
            package.targets.len(),
        );
        batches.push(SelectedPrebuildBatch {
            arguments,
            command,
            owner,
            targets: package.targets.len(),
        });
    }
    Ok(batches)
}

pub(super) fn run_selected_prebuild_batches<F>(
    batches: &[SelectedPrebuildBatch],
    concurrency: usize,
    output: &mut impl Write,
    run: F,
) -> Result<Vec<u8>, WorkspaceTestError>
where
    F: Fn(&SelectedPrebuildBatch, usize) -> SelectedPrebuildResult + Sync,
{
    if concurrency == 0 {
        return Err(WorkspaceTestError::new(
            "selected prebuild concurrency must be nonzero",
        ));
    }
    if batches.is_empty() {
        return Ok(Vec::new());
    }
    let lanes = concurrency.min(batches.len());
    let targets = batches.iter().map(|batch| batch.targets).sum::<usize>();
    writeln!(
        output,
        "[rust-test] stage=selected-prebuild targets={targets} batches={} lanes={lanes}",
        batches.len(),
    )
    .map_err(output_error)?;
    output.flush().map_err(output_error)?;

    let next = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let mut completed = thread::scope(|scope| {
        let mut workers = Vec::with_capacity(lanes);
        for lane in 0..lanes {
            let run = &run;
            let next = &next;
            let failed = &failed;
            workers.push(scope.spawn(move || {
                let mut completed = Vec::new();
                while !failed.load(AtomicOrdering::Acquire) {
                    let ordinal = next.fetch_add(1, AtomicOrdering::AcqRel);
                    let Some(batch) = batches.get(ordinal) else {
                        break;
                    };
                    let result = run(batch, lane);
                    if result.artifacts.is_err() {
                        failed.store(true, AtomicOrdering::Release);
                    }
                    completed.push((ordinal, result));
                }
                completed
            }));
        }
        workers
            .into_iter()
            .map(|worker| {
                worker.join().map_err(|_| {
                    WorkspaceTestError::new("selected prebuild worker thread panicked")
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|completed| completed.into_iter().flatten().collect::<Vec<_>>())
    })?;
    completed.sort_by_key(|(ordinal, _)| *ordinal);

    let mut artifacts = Vec::new();
    let mut failure = None;
    for (_, result) in completed {
        output.write_all(&result.progress).map_err(output_error)?;
        match result.artifacts {
            Ok(mut selected) => {
                artifacts.append(&mut selected);
                if artifacts.last().is_some_and(|byte| *byte != b'\n') {
                    artifacts.push(b'\n');
                }
            }
            Err(error) if failure.is_none() => failure = Some(error),
            Err(_) => {}
        }
    }
    output.flush().map_err(output_error)?;
    if let Some(error) = failure {
        return Err(error);
    }
    if next.load(AtomicOrdering::Acquire) < batches.len() {
        return Err(WorkspaceTestError::new(
            "selected prebuild stopped before every exact package batch completed",
        ));
    }
    Ok(artifacts)
}

pub(super) fn prebuilt_test_plan(
    inventory: &Inventory,
    messages: &[u8],
) -> Result<Vec<PrebuiltTest>, WorkspaceTestError> {
    let selected = inventory
        .packages
        .iter()
        .flat_map(|package| {
            package.targets.iter().map(|target| {
                (
                    (package.id.clone(), target.kind.clone(), target.name.clone()),
                    (package.name.clone(), package.root.clone(), target.clone()),
                )
            })
        })
        .collect::<BTreeMap<_, _>>();
    let mut artifacts = BTreeMap::new();
    for (line_index, line) in messages.split(|byte| *byte == b'\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        let message: CargoBuildMessage = serde_json::from_slice(line).map_err(|error| {
            WorkspaceTestError::new(format!(
                "selected prebuild message {} was invalid Cargo JSON: {error}",
                line_index + 1,
            ))
        })?;
        if message.reason != "compiler-artifact"
            || !message.profile.is_some_and(|profile| profile.test)
        {
            continue;
        }
        let (Some(package_id), Some(target), Some(executable)) =
            (message.package_id, message.target, message.executable)
        else {
            continue;
        };
        let key = (package_id, target.kind.join("+"), target.name);
        if !selected.contains_key(&key) {
            continue;
        }
        if !executable_file(&executable) {
            return Err(WorkspaceTestError::new(format!(
                "selected prebuild artifact {} is missing or not executable",
                executable.display(),
            )));
        }
        if let Some(previous) = artifacts.insert(key.clone(), executable.clone()) {
            return Err(WorkspaceTestError::new(format!(
                "selected prebuild published duplicate test artifact {key:?}: {} and {}",
                previous.display(),
                executable.display(),
            )));
        }
    }
    let missing = selected
        .keys()
        .filter(|key| !artifacts.contains_key(*key))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(WorkspaceTestError::new(format!(
            "selected prebuild omitted test artifact(s): {missing:?}",
        )));
    }
    let mut plan = selected
        .into_iter()
        .map(|(key, (package, package_root, target))| PrebuiltTest {
            package,
            package_root,
            target,
            executable: artifacts
                .remove(&key)
                .expect("complete artifact comparison retained every selected key"),
            partition: None,
        })
        .collect::<Vec<_>>();
    plan.sort_by(|left, right| {
        right
            .target
            .source_bytes
            .cmp(&left.target.source_bytes)
            .then_with(|| left.package.cmp(&right.package))
            .then_with(|| left.target.cmp(&right.target))
    });
    Ok(plan)
}

/// Split only the largest prebuilt artifacts into deterministic libtest complements.
///
/// The direct coordinator orders ordinary artifacts by selected source bytes. Inspecting
/// at most five ordinary candidates per worker lane keeps discovery bounded while
/// reaching the process-heavy integration harnesses that otherwise become the terminal
/// execution tail. One additional binary candidate admits explicitly isolated
/// process-timing tests without displacing an ordinary large harness. Every partition
/// executes one disjoint complement under libtest's exact skip semantics. Exclusive
/// complements lead the queue so their barriers cannot strand a process lane behind an
/// already-running ordinary wave. Ordinary complements retain their parent artifact's
/// scheduling priority so a split long-running harness cannot fall behind smaller
/// unsplit targets.
pub(super) fn partition_largest_prebuilt_tests(
    tests: Vec<PrebuiltTest>,
    environment: &Environment,
    concurrency: usize,
    output: &mut impl Write,
) -> Result<Vec<PrebuiltTest>, WorkspaceTestError> {
    const CANDIDATES_PER_LANE: usize = 5;
    const CASES_PER_PARTITION: usize = 5;

    let original_count = tests.len();
    if concurrency <= 1 {
        writeln!(
            output,
            "[rust-test] stage=partition artifacts={original_count} candidates=0 shards={original_count}",
        )
        .map_err(output_error)?;
        output.flush().map_err(output_error)?;
        return Ok(tests);
    }
    let candidate_limit = concurrency
        .saturating_mul(CANDIDATES_PER_LANE)
        .saturating_add(usize::from(
            tests.iter().any(|test| test.target.kind == "bin"),
        ));
    let mut candidates = 0;
    let mut partitioned = Vec::new();
    for test in tests {
        if candidates >= candidate_limit {
            partitioned.push(test);
            continue;
        }
        candidates += 1;
        let cases = prebuilt_test_cases(&test, environment)?;
        let partitions = cases
            .len()
            .div_ceil(CASES_PER_PARTITION)
            .clamp(1, concurrency);
        partitioned.extend(test.into_partitions(&cases, partitions));
    }
    partitioned.sort_by(|left, right| {
        right
            .requires_exclusive_lane()
            .cmp(&left.requires_exclusive_lane())
            .then_with(|| right.target.source_bytes.cmp(&left.target.source_bytes))
            .then_with(|| left.package.cmp(&right.package))
            .then_with(|| left.target.cmp(&right.target))
            .then_with(|| left.partition_index().cmp(&right.partition_index()))
    });
    writeln!(
        output,
        "[rust-test] stage=partition artifacts={original_count} candidates={candidates} shards={}",
        partitioned.len(),
    )
    .map_err(output_error)?;
    output.flush().map_err(output_error)?;
    Ok(partitioned)
}

fn prebuilt_test_cases(
    test: &PrebuiltTest,
    environment: &Environment,
) -> Result<Vec<String>, WorkspaceTestError> {
    let owner = format!(
        "prebuilt test discovery for package {:?} target {:?}",
        test.package, test.target.name,
    );
    let spec = process_spec(
        test.executable.as_os_str(),
        ["--list", "--format", "terse"],
        &test.package_root,
        environment.clone(),
        METADATA_DEADLINE,
        Cancellation::Never,
        &owner,
    )?;
    let outcome = sill::spawn(spec)
        .map_err(|error| WorkspaceTestError::new(format!("{owner} launch failed: {error}")))?
        .wait()
        .map_err(|error| WorkspaceTestError::new(format!("{owner} wait failed: {error}")))?;
    let capture_failed = !outcome.outputs_drained()
        || outcome.stdout().is_some_and(|output| output.truncated())
        || outcome.stderr().is_some_and(|output| output.truncated());
    if !matches!(outcome.completion(), Completion::Exited(status) if status.success())
        || capture_failed
    {
        return Err(WorkspaceTestError::new(format!(
            "{owner} ended as {}: stdout-tail={:?}; stderr-tail={:?}",
            completion_name(outcome.completion()),
            captured_excerpt(outcome.stdout()),
            captured_excerpt(outcome.stderr()),
        )));
    }
    let stdout = outcome
        .stdout()
        .ok_or_else(|| WorkspaceTestError::new(format!("{owner} captured no stdout")))?;
    let listing = std::str::from_utf8(stdout.bytes())
        .map_err(|error| WorkspaceTestError::new(format!("{owner} emitted non-UTF-8: {error}")))?;
    let mut cases = listing
        .lines()
        .map(|line| {
            line.strip_suffix(": test")
                .or_else(|| line.strip_suffix(": benchmark"))
                .map(str::to_owned)
                .ok_or_else(|| {
                    WorkspaceTestError::new(format!(
                        "{owner} emitted an unrecognized list row {line:?}",
                    ))
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    cases.sort();
    if cases.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(WorkspaceTestError::new(format!(
            "{owner} emitted duplicate test identities",
        )));
    }
    Ok(cases)
}

fn repair_missing_binary_outputs(
    root: &Path,
    environment: &Environment,
    cargo: &OsStr,
    inventory: &Inventory,
    output: &mut impl Write,
) -> Result<(), WorkspaceTestError> {
    for missing in missing_binary_outputs(inventory, environment)? {
        let command = format!(
            "cargo build --locked --package {:?} --bin {:?}",
            missing.binary.package_id, missing.binary.target_name,
        );
        let owner = format!(
            "CARGO_BIN_EXE repair for package {:?} binary target {:?} at {}",
            missing.binary.package_name,
            missing.binary.target_name,
            missing.path.display(),
        );
        run_prebuild_command(
            root,
            environment,
            cargo,
            output,
            &command,
            [
                OsStr::new("build"),
                OsStr::new("--locked"),
                OsStr::new("--package"),
                OsStr::new(&missing.binary.package_id),
                OsStr::new("--bin"),
                OsStr::new(&missing.binary.target_name),
            ],
            &owner,
        )?;
    }
    validate_prebuilt_binary_outputs(inventory, environment)
}

pub(super) fn validate_prebuilt_binary_outputs(
    inventory: &Inventory,
    environment: &Environment,
) -> Result<(), WorkspaceTestError> {
    let missing = missing_binary_outputs(inventory, environment)?;
    if missing.is_empty() {
        return Ok(());
    }
    let detail = missing
        .iter()
        .map(|missing| {
            format!(
                "{} (package {:?}, binary target {:?})",
                missing.path.display(),
                missing.binary.package_name,
                missing.binary.target_name,
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    Err(WorkspaceTestError::new(format!(
        "prebuilt CARGO_BIN_EXE output is missing or not executable: {detail}"
    )))
}

fn missing_binary_outputs<'a>(
    inventory: &'a Inventory,
    environment: &Environment,
) -> Result<Vec<MissingBinaryOutput<'a>>, WorkspaceTestError> {
    let target_directory = environment
        .entries()
        .iter()
        .find_map(|(name, value)| {
            (name == OsStr::new(CARGO_TARGET_DIR)).then(|| PathBuf::from(value))
        })
        .ok_or_else(|| {
            WorkspaceTestError::new("prebuilt CARGO_BIN_EXE validation requires CARGO_TARGET_DIR")
        })?;
    Ok(inventory
        .binary_outputs
        .iter()
        .filter_map(|binary| {
            let path = target_directory.join("debug").join(format!(
                "{}{}",
                binary.target_name,
                std::env::consts::EXE_SUFFIX,
            ));
            (!executable_file(&path)).then_some(MissingBinaryOutput { binary, path })
        })
        .collect())
}

pub(super) fn executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(target_family = "unix")]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(target_family = "unix"))]
    {
        true
    }
}

pub(super) fn prebuild_canvas_launcher(
    root: &Path,
    environment: &Environment,
    cargo: &OsStr,
    output: &mut impl Write,
) -> Result<(), WorkspaceTestError> {
    run_prebuild_command(
        root,
        environment,
        cargo,
        output,
        "cargo build --locked -p xtask --bin xtask",
        ["build", "--locked", "-p", "xtask", "--bin", "xtask"],
        "Canvas launcher prebuild",
    )
}

fn run_prebuild_command<I, S>(
    root: &Path,
    environment: &Environment,
    cargo: &OsStr,
    output: &mut impl Write,
    command: &str,
    arguments: I,
    owner: &str,
) -> Result<(), WorkspaceTestError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    run_prebuild_command_capture(root, environment, cargo, output, command, arguments, owner)
        .map(|_| ())
}

fn run_prebuild_command_capture<I, S>(
    root: &Path,
    environment: &Environment,
    cargo: &OsStr,
    output: &mut impl Write,
    command: &str,
    arguments: I,
    owner: &str,
) -> Result<Vec<u8>, WorkspaceTestError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    writeln!(output, "[rust-test] stage=prebuild command={command:?}").map_err(output_error)?;
    output.flush().map_err(output_error)?;
    let started = Instant::now();
    let spec = process_spec(
        cargo,
        arguments,
        root,
        environment.clone(),
        SHARD_DEADLINE,
        Cancellation::Never,
        owner,
    )?;
    let outcome = sill::spawn(spec)
        .map_err(|error| WorkspaceTestError::new(format!("prebuild launch failed: {error}")))?
        .wait()
        .map_err(|error| WorkspaceTestError::new(format!("prebuild wait failed: {error}")))?;
    let capture_failed = !outcome.outputs_drained()
        || outcome
            .stdout()
            .is_some_and(|captured| captured.truncated())
        || outcome
            .stderr()
            .is_some_and(|captured| captured.truncated());
    let disposition = match outcome.completion() {
        Completion::Exited(status) if status.success() && !capture_failed => "success".to_owned(),
        Completion::Exited(status) if capture_failed => {
            format!(
                "capture-failed after {}",
                completion_name(outcome.completion())
            )
        }
        completion => completion_name(completion),
    };
    writeln!(
        output,
        "[rust-test] stage=prebuild-terminal elapsed_ms={} disposition={}",
        started.elapsed().as_millis(),
        disposition,
    )
    .map_err(output_error)?;
    output.flush().map_err(output_error)?;
    match outcome.completion() {
        Completion::Exited(status) if status.success() && !capture_failed => outcome
            .stdout()
            .map(|stdout| stdout.bytes().to_vec())
            .ok_or_else(|| WorkspaceTestError::new(format!("{owner} captured no stdout"))),
        completion => Err(WorkspaceTestError::new(format!(
            "{owner} ended as {}: stdout-tail={:?}; stderr-tail={:?}",
            completion_name(completion),
            captured_excerpt(outcome.stdout()),
            captured_excerpt(outcome.stderr()),
        ))),
    }
}

#[derive(Clone, Debug)]
pub(super) struct PrebuiltTest {
    pub(super) package: String,
    pub(super) package_root: PathBuf,
    pub(super) target: TargetIdentity,
    pub(super) executable: PathBuf,
    pub(super) partition: Option<PrebuiltTestPartition>,
}

impl PrebuiltTest {
    pub(super) fn requires_exclusive_lane(&self) -> bool {
        self.partition
            .as_ref()
            .is_some_and(|partition| partition.exclusive)
    }

    pub(super) fn into_partitions(self, cases: &[String], count: usize) -> Vec<Self> {
        const EXCLUSIVE_LANE_PREFIX: &str = "exclusive_lane_";

        let (exclusive, ordinary): (Vec<_>, Vec<_>) = cases.iter().partition(|case| {
            case.rsplit("::")
                .next()
                .is_some_and(|name| name.starts_with(EXCLUSIVE_LANE_PREFIX))
        });
        if count <= 1 && exclusive.is_empty() || cases.is_empty() {
            return vec![self];
        }

        let ordinary_partitions = if ordinary.is_empty() {
            0
        } else {
            count
                .saturating_sub(usize::from(!exclusive.is_empty()))
                .max(1)
                .min(ordinary.len())
        };
        let mut groups = Vec::new();
        if !exclusive.is_empty() {
            groups.push((true, exclusive));
        }
        for index in 0..ordinary_partitions {
            groups.push((
                false,
                ordinary
                    .iter()
                    .enumerate()
                    .filter(|(case_index, _)| case_index % ordinary_partitions == index)
                    .map(|(_, case)| *case)
                    .collect(),
            ));
        }
        let partition_count = groups.len();
        groups
            .into_iter()
            .enumerate()
            .map(|(index, (exclusive, included))| {
                let skipped = cases
                    .iter()
                    .filter(|case| !included.contains(case))
                    .cloned()
                    .collect();
                Self {
                    partition: Some(PrebuiltTestPartition {
                        index,
                        count: partition_count,
                        skipped,
                        exclusive,
                    }),
                    ..self.clone()
                }
            })
            .collect()
    }

    pub(super) fn partition_index(&self) -> usize {
        self.partition
            .as_ref()
            .map_or(0, |partition| partition.index)
    }

    pub(super) fn partition_count(&self) -> usize {
        self.partition
            .as_ref()
            .map_or(1, |partition| partition.count)
    }

    pub(super) fn direct_arguments(&self, threads: usize) -> Vec<OsString> {
        let mut arguments = Vec::new();
        if let Some(partition) = &self.partition {
            arguments.push(OsString::from("--exact"));
            for skipped in &partition.skipped {
                arguments.extend([OsString::from("--skip"), OsString::from(skipped)]);
            }
        }
        let threads = if self.requires_exclusive_lane() {
            1
        } else {
            threads
        };
        arguments.push(OsString::from(format!("--test-threads={threads}")));
        arguments
    }
}

#[derive(Clone, Debug)]
pub(super) struct PrebuiltTestPartition {
    pub(super) index: usize,
    pub(super) count: usize,
    pub(super) skipped: Vec<String>,
    pub(super) exclusive: bool,
}

#[derive(Deserialize)]
struct CargoBuildMessage {
    reason: String,
    #[serde(default)]
    package_id: Option<String>,
    #[serde(default)]
    target: Option<CargoBuildTarget>,
    #[serde(default)]
    profile: Option<CargoBuildProfile>,
    #[serde(default)]
    executable: Option<PathBuf>,
}

#[derive(Deserialize)]
struct CargoBuildTarget {
    name: String,
    kind: Vec<String>,
}

#[derive(Deserialize)]
struct CargoBuildProfile {
    test: bool,
}
