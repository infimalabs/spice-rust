//! Task-overview home for every task only the operator can move, across the
//! whole board, with a control that steps to the next one.

use super::*;
use crate::composer_markdown::inline::LineControls;
use crate::runtime::BlockerActionState;
use crate::work::{
    DependencyWall, OperatorQueue, OperatorWait, TaskOverviewSelection, WorkProjection,
};

pub(super) struct BlockerPresentation<'view, 'board> {
    work: &'view WorkProjection<'board>,
    queue: OperatorQueue<'board>,
    /// The waiting task the next control opens.
    next: Option<&'board TaskStamp>,
    selection: &'view TaskOverviewSelection,
}

impl<'view, 'board> BlockerPresentation<'view, 'board> {
    pub(super) fn new(
        work: &'view WorkProjection<'board>,
        selection: &'view TaskOverviewSelection,
    ) -> Self {
        let queue = OperatorQueue::new(work);
        // The next control opens one task, so a one-task overview is the
        // queue position it steps from.
        let current = match selection.tasks() {
            [only] => work
                .tasks()
                .iter()
                .position(|task| task.row().statement.body.born == *only),
            _ => None,
        };
        let next = queue
            .next_after(current)
            .map(|index| &work.tasks()[index].row().statement.body.born);
        Self {
            work,
            queue,
            next,
            selection,
        }
    }

    pub(super) fn render_rail(
        &self,
        review: &UseStateHandle<bool>,
        launcher: &NodeRef,
        navigate: &Callback<navigation::Intent>,
    ) -> Html {
        let count = self.queue.len();
        let active = count > 0;
        let summary = if active {
            format!(
                "{count} {} on you.",
                if count == 1 {
                    "task waits"
                } else {
                    "tasks wait"
                }
            )
        } else {
            "Nothing waits on you.".to_owned()
        };
        let open = {
            let review = review.clone();
            Callback::from(move |_| review.set(true))
        };
        let next = self.next.cloned().map(|task| {
            let navigate = navigate.clone();
            Callback::from(move |_| navigate.emit(navigation::Intent::Task(task.clone())))
        });
        html! {
            <section
                class={classes!("work-blocker-rail", active.then_some("active"))}
                data-blocker-count={count.to_string()}
                aria-label="Tasks waiting on you"
            >
                <span class="work-blocker-rail-mark" aria-hidden="true">
                    if active {
                        <svg width="17" height="17" viewBox="0 0 24 24" focusable="false">
                            <path
                                d="M12 5.5v10"
                                fill="none"
                                stroke="currentColor"
                                stroke-width="2.5"
                                stroke-linecap="round"
                            />
                            <circle cx="12" cy="19" r="1.4" fill="currentColor" />
                        </svg>
                    } else {
                        <svg width="17" height="17" viewBox="0 0 24 24" focusable="false">
                            <path
                                d="m4.5 12.5 4.5 4.5L19.5 6.5"
                                fill="none"
                                stroke="currentColor"
                                stroke-width="2.8"
                                stroke-linecap="round"
                                stroke-linejoin="round"
                            />
                        </svg>
                    }
                </span>
                <span class="work-blocker-rail-copy">
                    <strong>{if active { "Needs attention" } else { "Ready to proceed" }}</strong>
                    <span>{summary}</span>
                </span>
                <span class="work-blocker-rail-actions">
                    <button
                        ref={launcher.clone()}
                        type="button"
                        class="app-compact-control work-blocker-review-open"
                        disabled={!active}
                        aria-haspopup="dialog"
                        onclick={open}
                    >
                        {if active { "Review" } else { "Nothing to review" }}
                    </button>
                    <button
                        type="button"
                        class="app-compact-control work-blocker-next"
                        disabled={next.is_none()}
                        title="Open the next task waiting on you"
                        onclick={next.unwrap_or_default()}
                    >{"Next"}</button>
                </span>
            </section>
        }
    }

    pub(super) fn render_review(
        &self,
        runtime: Option<&AppRuntime>,
        review: &UseStateHandle<bool>,
        close_ref: &NodeRef,
        navigate: &Callback<navigation::Intent>,
        can_grant: bool,
        grant: &Callback<task_detail::TrustGrantRequest>,
    ) -> Html {
        if !**review {
            return Html::default();
        }
        let close = {
            let review = review.clone();
            Callback::from(move |_| review.set(false))
        };
        // A title chip leaves the review for that task's overview article.
        // Closing returns focus to the review's launcher, so the article takes
        // focus after that render settles.
        let focus = {
            let review = review.clone();
            let focus = super::work_detail::focus_task_callback();
            Callback::from(move |task: TaskStamp| {
                review.set(false);
                let focus = focus.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    gloo_timers::future::TimeoutFuture::new(0).await;
                    focus.emit(task);
                });
            })
        };
        // Opening a waiting task leaves the review for that task's overview.
        let open = {
            let review = review.clone();
            let navigate = navigate.clone();
            Callback::from(move |task: TaskStamp| {
                review.set(false);
                navigate.emit(navigation::Intent::Task(task));
            })
        };
        let resolves = |task: &wire::TaskHandle| {
            task.born()
                .is_ok_and(|stamp| self.selection.tasks().contains(&stamp))
        };
        let controls = LineControls::Live {
            resolves: &resolves,
        };
        let count = self.queue.len();
        html! {
            <section
                class="work-blocker-review app-sheet-overlay"
                role="dialog"
                aria-modal="true"
                aria-labelledby="work-blocker-review-title"
                data-blocker-count={count.to_string()}
            >
                <header class="app-sheet-header">
                    <div>
                        <p class="app-sheet-kicker">{"Needs attention"}</p>
                        <h2 id="work-blocker-review-title">
                            {format!("{count} {} on you", if count == 1 { "task waits" } else { "tasks wait" })}
                        </h2>
                    </div>
                    <button
                        ref={close_ref.clone()}
                        type="button"
                        class="app-compact-control"
                        onclick={close}
                    >{"Back to overview"}</button>
                </header>
                <div
                    class="app-sheet-body work-blocker-review-body"
                    onclick={task_detail::task_text_chip_click(&focus)}
                    onkeydown={task_detail::task_text_chip_keydown(&focus)}
                >
                    <p class="work-blocker-review-intro">
                        {"Every task only you can move appears here once, across the whole board. Work waiting on tasks an agent can claim or is working proceeds on its own."}
                    </p>
                    {for self.queue.waits().iter().map(|wait| {
                        self.render_wait(wait, &controls, runtime, &open, can_grant, grant)
                    })}
                </div>
            </section>
        }
    }

    fn render_wait(
        &self,
        wait: &OperatorWait<'board>,
        controls: &LineControls<'_>,
        runtime: Option<&AppRuntime>,
        open: &Callback<TaskStamp>,
        can_grant: bool,
        grant: &Callback<task_detail::TrustGrantRequest>,
    ) -> Html {
        let board = runtime.and_then(AppRuntime::board);
        let tasks = self.work.tasks();
        let row = tasks[wait.task()].row();
        let statement = &row.statement.body;
        let requirements = wait.environment().then(|| {
            statement
                .requirements
                .as_slice()
                .iter()
                .filter(|requirement| requirement_blocked(board, requirement))
        });
        let missing_resources = wait.environment().then(|| {
            statement.resources.iter().filter(|(name, _)| {
                board.is_none_or(|board| board.resource_pool_available(name).is_none())
            })
        });
        let holds = wait
            .holds()
            .iter()
            .map(|&index| tasks[index].row().statement.body.handle.to_string())
            .collect::<Vec<_>>();
        let open = {
            let open = open.clone();
            let task = statement.born.clone();
            Callback::from(move |_: MouseEvent| open.emit(task.clone()))
        };
        html! {
            <article class="work-blocker-task" data-task-stamp={statement.born.to_string()}>
                <header>
                    <p class="app-sheet-kicker">{"Waiting on you"}</p>
                    <h3>{task_detail::render_task_title(&row.presentation_title, controls)}</h3>
                    <p class="work-task-pitch">{statement.pitch.clone()}</p>
                    <code>{statement.handle.to_string()}</code>
                    <button
                        type="button"
                        class="app-compact-control work-blocker-task-open"
                        onclick={open}
                    >{"Open task"}</button>
                </header>
                <div class="work-blocker-list">
                    {for requirements.into_iter().flatten()
                        .map(|requirement| render_requirement(statement, requirement, runtime, can_grant, grant))}
                    {for missing_resources.into_iter().flatten()
                        .map(|(name, shards)| render_resource_requirement(name, shards))}
                    if let Some(wall) = wait.wall() {
                        {render_wall(wall)}
                    }
                </div>
                if !holds.is_empty() {
                    <p class="work-blocker-holds">
                        {format!("Holds back {} {}: {}", holds.len(), if holds.len() == 1 { "task" } else { "tasks" }, holds.join(", "))}
                    </p>
                }
            </article>
        }
    }
}

fn requirement_blocked(
    board: Option<&crate::runtime::BoardView>,
    requirement: &wire::EnvironmentRequirement,
) -> bool {
    match requirement {
        wire::EnvironmentRequirement::ResourcePool { name, .. } => {
            board.is_none_or(|board| board.resource_pool_available(name).is_none())
        }
        _ => board.is_none_or(|board| !board.requirement_satisfied(requirement)),
    }
}

/// Say why a waiting task stands still and why only the operator can move it.
/// Its environment is explained by its requirement cards instead.
fn render_wall(wall: DependencyWall<'_>) -> Html {
    let (kind, title, reason, state) = match wall {
        DependencyWall::Environment(_) => return Html::default(),
        DependencyWall::Gate(_) => (
            "blocked",
            "Blocked",
            "Its prerequisite or controller cannot advance. Change the task or restore its missing authority.",
            "Operator required",
        ),
        DependencyWall::Deferred(_) => (
            "deferred",
            "Deferred",
            "No agent is offered it until it is woken.",
            "Deferred",
        ),
        DependencyWall::Parked(_) => (
            "parked",
            "Parked",
            "It is an unfiled Draft or filed off the queue; no agent is offered it until it is filed.",
            "Parked",
        ),
        DependencyWall::Cycle(_) => (
            "cycle",
            "Dependency cycle",
            "Its dependencies lead back to itself; no agent can finish the chain until they change.",
            "Cycle",
        ),
        DependencyWall::Unreachable(_) => (
            "unreachable",
            "Closed without completing",
            "It can never complete, so each dependency on it must change.",
            "Unreachable",
        ),
        DependencyWall::Selection(_) => (
            "selection",
            "Awaiting your decision",
            "Its candidate waits on your verdict for each output; no lane can make that decision for you.",
            "Operator required",
        ),
    };
    html! {
        <section class="work-blocker-card" data-blocker-kind={kind}>
            <div>
                <strong>{title}</strong>
                <p>{reason}</p>
            </div>
            <span class="work-blocker-state">{state}</span>
        </section>
    }
}

fn render_requirement(
    statement: &wire::intent::BoardStatement,
    requirement: &wire::EnvironmentRequirement,
    runtime: Option<&AppRuntime>,
    can_grant: bool,
    grant: &Callback<task_detail::TrustGrantRequest>,
) -> Html {
    match requirement {
        wire::EnvironmentRequirement::TrustGrant { value } => {
            render_trust_requirement(statement, value, runtime, can_grant, grant)
        }
        wire::EnvironmentRequirement::JournalSchema { value } => html! {
            <section class="work-blocker-card" data-blocker-kind="journal-schema">
                <div>
                    <strong>{"Journal schema upgrade required"}</strong>
                    <p>{format!("Requires schema {value} or newer. Only the operator can upgrade the journal; upgrade actions are not available in the app yet.")}</p>
                </div>
                <span class="work-blocker-state">{"Upgrade required"}</span>
            </section>
        },
        wire::EnvironmentRequirement::ApplianceVersion { value } => html! {
            <section class="work-blocker-card" data-blocker-kind="appliance-version">
                <div>
                    <strong>{"Newer appliance required"}</strong>
                    <p>{format!("Requires a version later than {value}. Only the operator can install it; reload and upgrade actions are not available in the app yet.")}</p>
                </div>
                <span class="work-blocker-state">{"Upgrade required"}</span>
            </section>
        },
        wire::EnvironmentRequirement::ResourcePool { name, shards } => {
            render_resource_requirement(name, shards.get())
        }
    }
}

fn render_resource_requirement(name: &wire::ResourcePoolName, shards: u32) -> Html {
    html! {
        <section class="work-blocker-card" data-blocker-kind="resource-pool">
            <div>
                <strong>{"Resource pool not configured"}</strong>
                <p>{format!("Requires {shards} shard(s) from `{name}`. Configure that named lock pool before this task can be allocated.")}</p>
            </div>
            <span class="work-blocker-state">{"Configuration required"}</span>
        </section>
    }
}

fn render_trust_requirement(
    statement: &wire::intent::BoardStatement,
    digest: &wire::Digest,
    runtime: Option<&AppRuntime>,
    can_grant: bool,
    grant: &Callback<task_detail::TrustGrantRequest>,
) -> Html {
    let action_state = runtime.and_then(|runtime| runtime.blocker_action(digest));
    let trust = runtime
        .and_then(AppRuntime::board)
        .and_then(|board| board.trust_head(digest));
    let (ready, explanation, unavailable_label) = match trust {
        None => (
            false,
            "The exact configuration has not been captured. Run `spice trust pend` from any worktree carrying this exact configuration before approval.".to_owned(),
            "Capture required",
        ),
        Some(head)
            if !head
                .worktree
                .as_deref()
                .is_some_and(|worktree| !worktree.is_empty()) =>
        {
            (
                false,
                "The recorded configuration has no worktree evidence. Run `spice trust pend` from any worktree carrying this exact configuration before approval.".to_owned(),
                "Capture required",
            )
        }
        Some(head) if head.state == wire::TrustState::Pended => (
            true,
            "Only the operator can trust executable configuration. Approve only if this exact digest is expected; its pending record carries verified worktree provenance.".to_owned(),
            "",
        ),
        Some(head) if head.state == wire::TrustState::Revoked => (
            false,
            "This exact configuration was revoked. A worktree carrying the next exact configuration must run `spice trust pend` before approval.".to_owned(),
            "Revoked",
        ),
        Some(_) => (
            false,
            "This configuration is already granted; refresh the blocker snapshot before acting.".to_owned(),
            "Refresh required",
        ),
    };
    let pending = matches!(
        action_state,
        Some(BlockerActionState::Pending | BlockerActionState::Approved)
    );
    let action_label = match action_state {
        Some(BlockerActionState::Pending) => "Approving…",
        Some(BlockerActionState::Approved) => "Approved",
        Some(BlockerActionState::Failed(_)) => "Retry approval",
        None if can_grant => "Approve trust grant",
        None => "Approval unavailable",
    };
    let approve = {
        let grant = grant.clone();
        let request = task_detail::TrustGrantRequest {
            task: statement.born.clone(),
            revision: statement.revision,
            digest: digest.clone(),
        };
        Callback::from(move |_| grant.emit(request.clone()))
    };
    html! {
        <section class="work-blocker-card work-blocker-card--action" data-blocker-kind="trust-grant">
            <div>
                <strong>{"Executable trust approval required"}</strong>
                <p>{explanation}</p>
                <code title={digest.to_string()}>{digest.to_string()}</code>
                if let Some(BlockerActionState::Failed(issue)) = action_state {
                    <p class="work-blocker-issue" role="alert">{issue.clone()}</p>
                }
                // Progress changes only the button's own label, so the card
                // never grows a line while approval settles.
                <span class="visually-hidden" role="status">{pending.then_some(action_label)}</span>
            </div>
            if ready {
                <button
                    type="button"
                    class="app-compact-control"
                    disabled={!can_grant || pending}
                    onclick={approve}
                >{action_label}</button>
            } else {
                <span class="work-blocker-state">{unavailable_label}</span>
            }
        </section>
    }
}
