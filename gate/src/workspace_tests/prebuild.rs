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
    let repair_started = Instant::now();
    repair_missing_binary_outputs(root, environment, cargo, inventory, output)?;
    writeln!(
        output,
        "[rust-test] stage=binary-output-repair-terminal elapsed_ms={} disposition=success",
        repair_started.elapsed().as_millis(),
    )
    .map_err(output_error)?;
    let plan_started = Instant::now();
    let plan = prebuilt_test_plan(inventory, &artifacts)?;
    writeln!(
        output,
        "[rust-test] stage=artifact-plan-terminal elapsed_ms={} disposition=success artifacts={}",
        plan_started.elapsed().as_millis(),
        plan.len(),
    )
    .map_err(output_error)?;
    output.flush().map_err(output_error)?;
    Ok(plan)
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
    let started = Instant::now();

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
        writeln!(
            output,
            "[rust-test] stage=selected-prebuild-terminal elapsed_ms={} disposition=failed",
            started.elapsed().as_millis(),
        )
        .map_err(output_error)?;
        output.flush().map_err(output_error)?;
        return Err(error);
    }
    if next.load(AtomicOrdering::Acquire) < batches.len() {
        writeln!(
            output,
            "[rust-test] stage=selected-prebuild-terminal elapsed_ms={} disposition=incomplete",
            started.elapsed().as_millis(),
        )
        .map_err(output_error)?;
        output.flush().map_err(output_error)?;
        return Err(WorkspaceTestError::new(
            "selected prebuild stopped before every exact package batch completed",
        ));
    }
    writeln!(
        output,
        "[rust-test] stage=selected-prebuild-terminal elapsed_ms={} disposition=success",
        started.elapsed().as_millis(),
    )
    .map_err(output_error)?;
    output.flush().map_err(output_error)?;
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
    mut tests: Vec<PrebuiltTest>,
    environment: &Environment,
    concurrency: usize,
    output: &mut impl Write,
) -> Result<Vec<PrebuiltTest>, WorkspaceTestError> {
    const CANDIDATES_PER_LANE: usize = 5;
    const CASES_PER_PARTITION: usize = 5;

    let started = Instant::now();
    let original_count = tests.len();
    if concurrency <= 1 {
        writeln!(
            output,
            "[rust-test] stage=partition artifacts={original_count} candidates=0 shards={original_count}",
        )
        .map_err(output_error)?;
        writeln!(
            output,
            "[rust-test] stage=partition-terminal elapsed_ms={} disposition=success",
            started.elapsed().as_millis(),
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
    let candidate_count = candidate_limit.min(tests.len());
    let remaining = tests.split_off(candidate_count);
    let discovery_started = Instant::now();
    let discoveries = run_bounded_discovery(&tests, concurrency, |test, cancellation| {
        prebuilt_test_cases(test, environment, cancellation)
    })?;
    let discovery_elapsed = discovery_started.elapsed();
    report_prebuilt_discoveries(
        &tests,
        &discoveries.completed,
        discovery_elapsed,
        concurrency,
        output,
    )?;
    if let Some(failure_index) = discoveries.causal_failure {
        writeln!(
            output,
            "[rust-test] stage=partition-terminal elapsed_ms={} disposition=failed",
            started.elapsed().as_millis(),
        )
        .map_err(output_error)?;
        output.flush().map_err(output_error)?;
        return Err(discoveries
            .completed
            .into_iter()
            .find(|discovery| discovery.ordinal == failure_index)
            .and_then(|discovery| discovery.cases.err())
            .expect("recorded discovery failure retains its error"));
    }
    if discoveries
        .completed
        .iter()
        .any(|discovery| discovery.cases.is_err())
    {
        return Err(WorkspaceTestError::new(
            "prebuilt test discovery recorded sibling cancellation without a causal failure",
        ));
    }
    if discoveries.completed.len() != candidate_count {
        return Err(WorkspaceTestError::new(format!(
            "prebuilt test discovery completed {} of {candidate_count} candidate(s) without reporting a failure",
            discoveries.completed.len(),
        )));
    }

    let mut partitioned = Vec::new();
    for (test, discovery) in tests.into_iter().zip(discoveries.completed) {
        let cases = discovery
            .cases
            .expect("complete discovery was checked before partitioning");
        let partitions = cases
            .len()
            .div_ceil(CASES_PER_PARTITION)
            .clamp(1, concurrency);
        partitioned.extend(test.into_partitions(&cases, partitions));
    }
    partitioned.extend(remaining);
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
        "[rust-test] stage=partition artifacts={original_count} candidates={candidate_count} shards={}",
        partitioned.len(),
    )
    .map_err(output_error)?;
    writeln!(
        output,
        "[rust-test] stage=partition-terminal elapsed_ms={} disposition=success",
        started.elapsed().as_millis(),
    )
    .map_err(output_error)?;
    output.flush().map_err(output_error)?;
    Ok(partitioned)
}

#[derive(Debug)]
struct PrebuiltTestDiscovery {
    ordinal: usize,
    elapsed: Duration,
    cases: Result<Vec<String>, WorkspaceTestError>,
}

fn report_prebuilt_discoveries(
    tests: &[PrebuiltTest],
    discoveries: &[PrebuiltTestDiscovery],
    elapsed: Duration,
    concurrency: usize,
    output: &mut impl Write,
) -> Result<(), WorkspaceTestError> {
    let mut failed = false;
    for discovery in discoveries {
        let test = &tests[discovery.ordinal];
        let (disposition, cases) = match &discovery.cases {
            Ok(cases) => ("success", cases.len().to_string()),
            Err(_) => {
                failed = true;
                ("failed", "unknown".to_owned())
            }
        };
        writeln!(
            output,
            "[rust-test] stage=discovery-terminal package={:?} target={:?} elapsed_ms={} disposition={disposition} cases={cases}",
            test.package,
            test.target.name,
            discovery.elapsed.as_millis(),
        )
        .map_err(output_error)?;
    }
    let disposition = if failed { "failed" } else { "success" };
    writeln!(
        output,
        "[rust-test] stage=discovery-batch-terminal elapsed_ms={} disposition={disposition} candidates={} completed={} lanes={}",
        elapsed.as_millis(),
        tests.len(),
        discoveries.len(),
        concurrency.min(tests.len()),
    )
    .map_err(output_error)?;
    output.flush().map_err(output_error)?;
    Ok(())
}

#[derive(Debug)]
struct PrebuiltDiscoveryBatch {
    /// Progress records retained in candidate ordinal order.
    completed: Vec<PrebuiltTestDiscovery>,
    /// First independently observed failure, excluding cancellation caused by a sibling.
    causal_failure: Option<usize>,
}

struct PrebuiltDiscoveryAdmission {
    next: usize,
    causal_failure: Option<usize>,
}

fn run_bounded_discovery<T, F>(
    candidates: &[T],
    concurrency: usize,
    discover: F,
) -> Result<PrebuiltDiscoveryBatch, WorkspaceTestError>
where
    T: Sync,
    F: Fn(&T, CancellationToken) -> Result<Vec<String>, WorkspaceTestError> + Sync,
{
    if !(1..=crate::PREBUILT_WORKSPACE_TEST_CONCURRENCY_CAP).contains(&concurrency) {
        return Err(WorkspaceTestError::new(format!(
            "prebuilt test discovery concurrency must be between 1 and {}, got {concurrency}",
            crate::PREBUILT_WORKSPACE_TEST_CONCURRENCY_CAP,
        )));
    }
    if candidates.is_empty() {
        return Ok(PrebuiltDiscoveryBatch {
            completed: Vec::new(),
            causal_failure: None,
        });
    }

    let lanes = concurrency.min(candidates.len());
    let admission = Mutex::new(PrebuiltDiscoveryAdmission {
        next: 0,
        causal_failure: None,
    });
    let cancellation = CancellationToken::new();
    let mut completed = thread::scope(|scope| {
        let mut workers = Vec::with_capacity(lanes);
        for _ in 0..lanes {
            let discover = &discover;
            let admission = &admission;
            let cancellation = &cancellation;
            workers.push(scope.spawn(move || {
                let mut completed = Vec::new();
                loop {
                    let ordinal = {
                        let mut admission = admission.lock().map_err(|_| {
                            WorkspaceTestError::new(
                                "prebuilt test discovery admission lock was poisoned",
                            )
                        })?;
                        if cancellation.is_cancelled() {
                            break;
                        }
                        let ordinal = admission.next;
                        admission.next = admission.next.saturating_add(1);
                        ordinal
                    };
                    let Some(candidate) = candidates.get(ordinal) else {
                        break;
                    };
                    let started = Instant::now();
                    let cases = discover(candidate, cancellation.clone());
                    if cases.is_err() {
                        let mut admission = admission.lock().map_err(|_| {
                            WorkspaceTestError::new(
                                "prebuilt test discovery admission lock was poisoned",
                            )
                        })?;
                        if cases
                            .as_ref()
                            .is_err_and(|error| !error.is_synthetic_cancellation())
                        {
                            admission.causal_failure.get_or_insert(ordinal);
                        }
                        cancellation.cancel();
                    }
                    completed.push(PrebuiltTestDiscovery {
                        ordinal,
                        elapsed: started.elapsed(),
                        cases,
                    });
                }
                Ok(completed)
            }));
        }
        workers
            .into_iter()
            .map(|worker| {
                worker.join().map_err(|_| {
                    WorkspaceTestError::new("prebuilt test discovery worker thread panicked")
                })?
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|completed| completed.into_iter().flatten().collect::<Vec<_>>())
    })?;
    completed.sort_by_key(|result| result.ordinal);
    let causal_failure = admission
        .into_inner()
        .map_err(|_| {
            WorkspaceTestError::new("prebuilt test discovery admission lock was poisoned")
        })?
        .causal_failure;
    Ok(PrebuiltDiscoveryBatch {
        completed,
        causal_failure,
    })
}

fn prebuilt_test_cases(
    test: &PrebuiltTest,
    environment: &Environment,
    cancellation: CancellationToken,
) -> Result<Vec<String>, WorkspaceTestError> {
    prebuilt_test_cases_with_deadline(test, environment, cancellation, METADATA_DEADLINE)
}

fn prebuilt_test_cases_with_deadline(
    test: &PrebuiltTest,
    environment: &Environment,
    cancellation: CancellationToken,
    deadline: Duration,
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
        deadline,
        Cancellation::Token(cancellation),
        &owner,
    )?;
    let outcome = sill::spawn(spec)
        .map_err(|error| WorkspaceTestError::new(format!("{owner} launch failed: {error}")))?
        .wait()
        .map_err(|error| WorkspaceTestError::new(format!("{owner} wait failed: {error}")))?;
    let capture_failed = !outcome.outputs_drained()
        || outcome.stdout().is_some_and(|output| output.truncated())
        || outcome.stderr().is_some_and(|output| output.truncated());
    if matches!(outcome.completion(), Completion::Cancelled(_)) {
        return Err(WorkspaceTestError::synthetic_cancellation(format!(
            "{owner} ended as {}: stdout-tail={:?}; stderr-tail={:?}",
            completion_name(outcome.completion()),
            captured_excerpt(outcome.stdout()),
            captured_excerpt(outcome.stderr()),
        )));
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt as _;
    #[cfg(unix)]
    use std::process::{Command, Stdio};

    #[cfg(unix)]
    struct DiscoveryFixture {
        root: PathBuf,
    }

    #[cfg(unix)]
    impl DiscoveryFixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);

            for _ in 0..100 {
                let nonce = NEXT.fetch_add(1, AtomicOrdering::Relaxed);
                let root = env::temp_dir().join(format!(
                    "spice-gate-discovery-{}-{nonce}",
                    std::process::id(),
                ));
                match fs::create_dir(&root) {
                    Ok(()) => return Self { root },
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => panic!("create discovery fixture root: {error}"),
                }
            }
            panic!("create unique discovery fixture root");
        }

        fn script(&self, name: &str, body: &str) -> PathBuf {
            let path = self.root.join(name);
            fs::write(&path, format!("#!/bin/sh\n{body}")).expect("write discovery fixture script");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
                .expect("make discovery fixture script executable");
            path
        }

        fn test(&self, name: &str, executable: PathBuf) -> PrebuiltTest {
            PrebuiltTest {
                package: "fixture".to_owned(),
                package_root: self.root.clone(),
                target: TargetIdentity {
                    package: "fixture".to_owned(),
                    kind: "test".to_owned(),
                    name: name.to_owned(),
                    required_features: Vec::new(),
                    source: executable.clone(),
                    declared_path_sources: Vec::new(),
                    source_bytes: 1,
                },
                executable,
                partition: None,
            }
        }
    }

    #[cfg(unix)]
    impl Drop for DiscoveryFixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).expect("remove discovery fixture root");
        }
    }

    #[cfg(unix)]
    fn empty_environment() -> Environment {
        Environment::new(std::iter::empty::<(OsString, OsString)>())
            .expect("empty fixture environment is valid")
    }

    #[cfg(unix)]
    fn wait_for_paths(paths: &[PathBuf]) {
        let started = Instant::now();
        while !paths.iter().all(|path| path.is_file()) {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "fixture children did not publish {:?}",
                paths,
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[cfg(unix)]
    fn fixture_pid(path: &Path) -> u32 {
        fs::read_to_string(path)
            .expect("read fixture PID")
            .trim()
            .parse()
            .expect("fixture PID is numeric")
    }

    #[cfg(unix)]
    fn process_is_alive(pid: u32) -> bool {
        Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("inspect fixture process")
            .success()
    }

    #[test]
    fn single_lane_partition_reports_its_terminal_timing() {
        let environment = Environment::new(std::iter::empty::<(OsString, OsString)>())
            .expect("empty fixture environment is valid");
        let mut output = Vec::new();

        let partitioned =
            partition_largest_prebuilt_tests(Vec::new(), &environment, 1, &mut output)
                .expect("single-lane partition succeeds");

        assert!(partitioned.is_empty());
        let report = String::from_utf8(output).expect("partition report is UTF-8");
        assert!(report.contains("stage=partition artifacts=0 candidates=0 shards=0"));
        assert!(report.contains("stage=partition-terminal elapsed_ms="));
    }

    #[test]
    fn bounded_discovery_preserves_candidate_order_and_lane_bound() {
        let candidates = (0..9).collect::<Vec<_>>();
        let active = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);

        let discoveries = run_bounded_discovery(&candidates, 3, |candidate, _cancellation| {
            let now_active = active.fetch_add(1, AtomicOrdering::AcqRel) + 1;
            peak.fetch_max(now_active, AtomicOrdering::AcqRel);
            thread::sleep(Duration::from_millis(2));
            active.fetch_sub(1, AtomicOrdering::AcqRel);
            Ok(vec![candidate.to_string()])
        })
        .expect("bounded discovery succeeds");

        assert_eq!(
            discoveries
                .completed
                .iter()
                .map(|discovery| discovery.ordinal)
                .collect::<Vec<_>>(),
            (0..candidates.len()).collect::<Vec<_>>(),
        );
        assert_eq!(discoveries.causal_failure, None);
        assert!(peak.load(AtomicOrdering::Acquire) <= 3);
        assert_eq!(active.load(AtomicOrdering::Acquire), 0);
    }

    #[test]
    fn bounded_discovery_stops_admission_after_failure() {
        let candidates = (0..100).collect::<Vec<_>>();
        let first_wave = Barrier::new(2);
        let invoked = AtomicUsize::new(0);

        let discoveries = run_bounded_discovery(&candidates, 2, |candidate, _cancellation| {
            invoked.fetch_add(1, AtomicOrdering::AcqRel);
            if *candidate < 2 {
                first_wave.wait();
            }
            if *candidate == 0 {
                Err(WorkspaceTestError::new("fixture discovery failed"))
            } else {
                if *candidate == 1 {
                    thread::sleep(Duration::from_millis(2));
                }
                Ok(vec![candidate.to_string()])
            }
        })
        .expect("worker pool reports child failures through ordered results");

        assert_eq!(
            discoveries
                .completed
                .iter()
                .map(|discovery| discovery.ordinal)
                .collect::<Vec<_>>(),
            vec![0, 1],
        );
        assert!(discoveries.completed[0].cases.is_err());
        assert_eq!(discoveries.causal_failure, Some(0));
        assert_eq!(invoked.load(AtomicOrdering::Acquire), 2);
    }

    #[test]
    fn bounded_discovery_refuses_unbounded_concurrency() {
        let error = run_bounded_discovery(&[0], usize::MAX, |_, _cancellation| Ok(Vec::new()))
            .expect_err("unbounded discovery concurrency is refused");

        assert!(error.to_string().contains("concurrency must be between 1"));
    }

    #[cfg(unix)]
    #[test]
    fn real_discovery_failure_cancels_and_reaps_in_flight_sibling() {
        let fixture = DiscoveryFixture::new();
        let ready = fixture.root.join("sleeper-ready");
        let pid = fixture.root.join("sleeper-pid");
        let sleeper = fixture.script(
            "sleeper",
            &format!(
                "printf '%s\\n' \"$$\" > '{}'\nprintf 'ready\\n' > '{}'\nexec /bin/sleep 30\n",
                pid.display(),
                ready.display(),
            ),
        );
        let failure = fixture.script(
            "failure",
            &format!(
                "while [ ! -f '{}' ]; do /bin/sleep 0.01; done\nexit 19\n",
                ready.display(),
            ),
        );
        let tests = vec![
            fixture.test("failure", failure),
            fixture.test("sleeper", sleeper),
        ];
        let environment = empty_environment();
        let started = Instant::now();

        let discoveries = run_bounded_discovery(&tests, 2, |test, cancellation| {
            prebuilt_test_cases(test, &environment, cancellation)
        })
        .expect("worker pool captures real child failures");

        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(
            discoveries
                .completed
                .iter()
                .map(|discovery| discovery.ordinal)
                .collect::<Vec<_>>(),
            vec![0, 1],
        );
        assert!(
            discoveries.completed[0]
                .cases
                .as_ref()
                .expect_err("first child fails")
                .to_string()
                .contains("exit code=19"),
        );
        assert!(
            discoveries.completed[1]
                .cases
                .as_ref()
                .expect_err("sibling is cancelled")
                .to_string()
                .contains("cancelled"),
        );
        assert_eq!(discoveries.causal_failure, Some(0));
        assert!(!process_is_alive(fixture_pid(&pid)));
    }

    #[cfg(unix)]
    #[test]
    fn reversed_real_failure_survives_lower_ordinal_sibling_cancellation() {
        let fixture = DiscoveryFixture::new();
        let ready = fixture.root.join("reversed-sleeper-ready");
        let pid = fixture.root.join("reversed-sleeper-pid");
        let sleeper = fixture.script(
            "ordinal-0-sleeper",
            &format!(
                "printf '%s\\n' \"$$\" > '{}'\nprintf 'ready\\n' > '{}'\nexec /bin/sleep 30\n",
                pid.display(),
                ready.display(),
            ),
        );
        let failure = fixture.script(
            "ordinal-1-failure",
            &format!(
                "while [ ! -f '{}' ]; do /bin/sleep 0.01; done\nexit 19\n",
                ready.display(),
            ),
        );
        let tests = vec![
            fixture.test("ordinal-0-sleeper", sleeper),
            fixture.test("ordinal-1-failure", failure),
        ];
        let mut output = Vec::new();
        let started = Instant::now();

        let error = partition_largest_prebuilt_tests(tests, &empty_environment(), 2, &mut output)
            .expect_err("ordinal 1 exit fails discovery");

        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(error.to_string().contains("ordinal-1-failure"), "{error}");
        assert!(error.to_string().contains("exit code=19"), "{error}");
        assert!(!error.to_string().contains("cancelled"), "{error}");
        assert!(!process_is_alive(fixture_pid(&pid)));
        let output = String::from_utf8(output).expect("discovery progress is UTF-8");
        let ordinal_0 = output.find("ordinal-0-sleeper").expect("ordinal 0 reports");
        let ordinal_1 = output.find("ordinal-1-failure").expect("ordinal 1 reports");
        assert!(ordinal_0 < ordinal_1, "{output}");
    }

    #[cfg(unix)]
    #[test]
    fn real_discovery_timeout_reaps_child() {
        let fixture = DiscoveryFixture::new();
        let pid = fixture.root.join("timeout-pid");
        let sleeper = fixture.script(
            "timeout",
            &format!(
                "printf '%s\\n' \"$$\" > '{}'\nexec /bin/sleep 30\n",
                pid.display(),
            ),
        );
        let test = fixture.test("timeout", sleeper);
        let started = Instant::now();

        let error = prebuilt_test_cases_with_deadline(
            &test,
            &empty_environment(),
            CancellationToken::new(),
            Duration::from_secs(2),
        )
        .expect_err("metadata deadline terminates the child");

        assert!(started.elapsed() < Duration::from_secs(6));
        assert!(error.to_string().contains("timeout"));
        assert!(!process_is_alive(fixture_pid(&pid)));
    }

    #[cfg(unix)]
    #[test]
    fn real_discovery_preserves_order_and_concurrency_bound() {
        let fixture = DiscoveryFixture::new();
        let release = fixture.root.join("release");
        let mut started_paths = Vec::new();
        let mut tests = Vec::new();
        for ordinal in 0..6 {
            let started = fixture.root.join(format!("started-{ordinal}"));
            let delay = match ordinal {
                0 => "0.09",
                1 => "0.06",
                2 => "0.03",
                _ => "0",
            };
            let executable = fixture.script(
                &format!("ordered-{ordinal}"),
                &format!(
                    "printf 'started\\n' > '{}'\nwhile [ ! -f '{}' ]; do /bin/sleep 0.01; done\n/bin/sleep {delay}\nprintf 'case_{ordinal}: test\\n'\n",
                    started.display(),
                    release.display(),
                ),
            );
            started_paths.push(started);
            tests.push(fixture.test(&format!("ordered-{ordinal}"), executable));
        }
        let environment = empty_environment();

        let discoveries = thread::scope(|scope| {
            let worker = scope.spawn(|| {
                run_bounded_discovery(&tests, 3, |test, cancellation| {
                    prebuilt_test_cases(test, &environment, cancellation)
                })
            });
            wait_for_paths(&started_paths[..3]);
            assert!(started_paths[3..].iter().all(|path| !path.exists()));
            fs::write(&release, b"release\n").expect("release discovery children");
            worker
                .join()
                .expect("discovery worker thread does not panic")
        })
        .expect("real bounded discovery succeeds");

        assert_eq!(
            discoveries
                .completed
                .iter()
                .map(|discovery| discovery.ordinal)
                .collect::<Vec<_>>(),
            (0..tests.len()).collect::<Vec<_>>(),
        );
        assert_eq!(
            discoveries
                .completed
                .into_iter()
                .map(|discovery| discovery.cases.expect("discovery succeeds"))
                .collect::<Vec<_>>(),
            (0..tests.len())
                .map(|ordinal| vec![format!("case_{ordinal}")])
                .collect::<Vec<_>>(),
        );
    }
}
