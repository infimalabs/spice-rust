//! Exact command-rim capture assembly for complete and focused gate runs.

use super::*;

pub(super) fn capture_inner_with_reach<'trace>(
    selector: Selector,
    input_mode: InputMode,
    selected: Option<GateId>,
    trace: Option<&'trace mut (dyn FnMut(RepositoryExecutionEvent) + Send)>,
    state_run: Option<::gate::BatteryStateRun>,
    selection: Option<&ChangeSelectionReport>,
) -> Result<BatteryCapture<'trace>, RunError> {
    capture_inner_with_reach_and_claim(
        selector,
        input_mode,
        selected,
        trace,
        state_run,
        selection,
        ClaimCapture::Idle,
    )
}

pub(super) fn capture_inner_with_reach_and_claim<'trace>(
    selector: Selector,
    input_mode: InputMode,
    selected: Option<GateId>,
    trace: Option<&'trace mut (dyn FnMut(RepositoryExecutionEvent) + Send)>,
    state_run: Option<::gate::BatteryStateRun>,
    selection: Option<&ChangeSelectionReport>,
    claim: ClaimCapture,
) -> Result<BatteryCapture<'trace>, RunError> {
    let directory = env::current_dir()
        .map_err(|error| RunError::fault("gate::working-directory-unavailable", error))?;
    capture_at_inner_with_options(
        &directory,
        CaptureOptions {
            selector,
            input_mode,
            selected,
            trace,
            state_run,
            selection,
            claim,
        },
    )
}

pub(super) fn capture_at_inner<'trace>(
    directory: &Path,
    selector: Selector,
    input_mode: InputMode,
    selected: Option<GateId>,
    trace: Option<&'trace mut (dyn FnMut(RepositoryExecutionEvent) + Send)>,
    state_run: Option<::gate::BatteryStateRun>,
    selection: Option<&ChangeSelectionReport>,
) -> Result<BatteryCapture<'trace>, RunError> {
    capture_at_inner_with_options(
        directory,
        CaptureOptions {
            selector,
            input_mode,
            selected,
            trace,
            state_run,
            selection,
            claim: ClaimCapture::Idle,
        },
    )
}

struct CaptureOptions<'trace, 'selection> {
    selector: Selector,
    input_mode: InputMode,
    selected: Option<GateId>,
    trace: Option<&'trace mut (dyn FnMut(RepositoryExecutionEvent) + Send)>,
    state_run: Option<::gate::BatteryStateRun>,
    selection: Option<&'selection ChangeSelectionReport>,
    claim: ClaimCapture,
}

fn capture_at_inner_with_options<'trace>(
    directory: &Path,
    options: CaptureOptions<'trace, '_>,
) -> Result<BatteryCapture<'trace>, RunError> {
    let CaptureOptions {
        selector,
        input_mode,
        selected,
        trace,
        state_run,
        selection,
        claim,
    } = options;
    let repository = Repository::discover(directory)
        .map_err(|error| RunError::fault("gate::repository-unresolved", error))?;
    repository.admit_state().map_err(|error| match error {
        StateError::Filesystem { .. } => RunError::fault("gate::state-unavailable", error),
        _ => RunError::refusal("gate::state-refused", error),
    })?;
    let actor = acting_actor(repository.worktree_root())?;
    let changes = selected_changes(repository.worktree_root(), &selector)?;
    let rim = RimCapture::new(directory, search_path(), changes)
        .map_err(|error| RunError::refusal("gate::rim-refused", error))?;
    let request = CaptureRequest {
        root: repository.worktree_root(),
        selector: &selector,
        input_mode,
        selected,
        trace,
        state_run,
        claim,
        actor: ActorCapture::Acting {
            actor,
            // The shared state run owns the exact clock and active claims.
            bound_claims: Vec::new(),
        },
        rim,
        hygiene: HygieneConfiguration::current(),
    };
    if let Some(selection) = selection {
        capture_reached(request, selection)
    } else {
        capture_without_reach(request)
    }
}

struct CaptureRequest<'root, 'trace> {
    root: &'root Path,
    selector: &'root Selector,
    input_mode: InputMode,
    selected: Option<GateId>,
    trace: Option<&'trace mut (dyn FnMut(RepositoryExecutionEvent) + Send)>,
    state_run: Option<::gate::BatteryStateRun>,
    claim: ClaimCapture,
    actor: ActorCapture,
    rim: RimCapture,
    hygiene: HygieneConfiguration,
}

fn capture_reached<'trace>(
    request: CaptureRequest<'_, 'trace>,
    selection: &ChangeSelectionReport,
) -> Result<BatteryCapture<'trace>, RunError> {
    let Some(gate_id) = request.selected else {
        return Err(RunError::refusal(
            "gate::reach-refused",
            "authored reach requires one selected arm",
        ));
    };
    if request.trace.is_some() {
        return Err(RunError::refusal(
            "gate::reach-refused",
            "authored reach requires an untraced selected arm",
        ));
    }
    let state_run = request.state_run.ok_or_else(|| {
        RunError::refusal(
            "gate::reach-refused",
            "authored reach requires an enclosing state transaction",
        )
    })?;
    let capture = match request.input_mode {
        InputMode::Generic => BatteryCapture::capture_generic_arm_in_state_reached(
            request.root,
            request.selector,
            request.claim,
            request.actor,
            request.rim,
            request.hygiene,
            selection,
            gate_id,
            state_run,
        ),
        InputMode::Strict => BatteryCapture::capture_arm_in_state_reached(
            request.root,
            request.selector,
            request.claim,
            request.actor,
            request.rim,
            request.hygiene,
            selection,
            gate_id,
            state_run,
        ),
    };
    capture.map_err(|error| RunError::refusal("gate::capture-refused", error))
}

fn capture_without_reach<'trace>(
    request: CaptureRequest<'_, 'trace>,
) -> Result<BatteryCapture<'trace>, RunError> {
    let CaptureRequest {
        root,
        selector,
        input_mode,
        selected,
        trace,
        state_run,
        claim,
        actor,
        rim,
        hygiene,
    } = request;
    match (input_mode, selected, trace, state_run) {
        (InputMode::Generic, Some(gate_id), None, Some(state_run)) => {
            BatteryCapture::capture_generic_arm_in_state(
                root, selector, claim, actor, rim, hygiene, gate_id, state_run,
            )
        }
        (InputMode::Strict, Some(gate_id), None, Some(state_run)) => {
            BatteryCapture::capture_arm_in_state(
                root, selector, claim, actor, rim, hygiene, gate_id, state_run,
            )
        }
        (InputMode::Generic, Some(gate_id), Some(trace), None) => {
            BatteryCapture::capture_generic_arm_traced(
                root, selector, claim, actor, rim, hygiene, gate_id, trace,
            )
        }
        (InputMode::Strict, Some(gate_id), Some(trace), None) => {
            BatteryCapture::capture_arm_traced(
                root, selector, claim, actor, rim, hygiene, gate_id, trace,
            )
        }
        (InputMode::Generic, Some(gate_id), None, None) => {
            BatteryCapture::capture_generic_arm(root, selector, claim, actor, rim, hygiene, gate_id)
        }
        (InputMode::Strict, Some(gate_id), None, None) => {
            BatteryCapture::capture_arm(root, selector, claim, actor, rim, hygiene, gate_id)
        }
        (InputMode::Generic, None, Some(trace), None) => BatteryCapture::capture_generic_traced(
            root, selector, claim, actor, rim, hygiene, trace,
        ),
        (InputMode::Strict, None, Some(trace), None) => {
            BatteryCapture::capture_traced(root, selector, claim, actor, rim, hygiene, trace)
        }
        (InputMode::Generic, None, None, None) => {
            BatteryCapture::capture_generic(root, selector, claim, actor, rim, hygiene)
        }
        (InputMode::Strict, None, None, None) => {
            BatteryCapture::capture(root, selector, claim, actor, rim, hygiene)
        }
        (InputMode::Generic, None, Some(trace), Some(state_run)) => {
            BatteryCapture::capture_generic_in_state_traced(
                root, selector, claim, actor, rim, hygiene, state_run, trace,
            )
        }
        (InputMode::Strict, None, Some(trace), Some(state_run)) => {
            BatteryCapture::capture_in_state_traced(
                root, selector, claim, actor, rim, hygiene, state_run, trace,
            )
        }
        (_, None, None, Some(_)) | (_, Some(_), Some(_), Some(_)) => {
            unreachable!("only complete shared-state captures are traced")
        }
    }
    .map_err(|error| {
        let detail = error.detail();
        let reason = if detail.starts_with("cannot load captured battery contract:") {
            "gate::contract-refused"
        } else if detail.starts_with("cannot capture battery source:") {
            "gate::selection-refused"
        } else {
            "gate::capture-refused"
        };
        RunError::refusal(reason, detail)
    })
}
