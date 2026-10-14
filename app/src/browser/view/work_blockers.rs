//! Stable task-overview home for walls only the operator can move.

use super::*;
use crate::composer_markdown::inline::LineControls;
use crate::runtime::BlockerActionState;
use crate::work::{
    DependencyWall, DependencyWalls, TaskOverviewSelection, WorkProjection, WorkTask,
};

/// One selected task held by operator-only walls. Dependencies on work an
/// agent can claim or is working are ordinary waits and never appear.
struct WalledTask<'view, 'board> {
    task: &'view WorkTask<'board>,
    requirements: Vec<&'board wire::EnvironmentRequirement>,
    dependencies: Vec<(&'board TaskStamp, DependencyWall<'board>)>,
}

impl WalledTask<'_, '_> {
    fn count(&self) -> usize {
        self.requirements.len() + self.dependencies.len()
    }
}

pub(super) struct BlockerPresentation<'view, 'board> {
    tasks: Vec<WalledTask<'view, 'board>>,
    count: usize,
    selection: &'view TaskOverviewSelection,
}

impl<'view, 'board> BlockerPresentation<'view, 'board> {
    pub(super) fn new(
        work: &'view WorkProjection<'board>,
        selection: &'view TaskOverviewSelection,
        runtime: Option<&'view AppRuntime>,
    ) -> Self {
        let board = runtime.and_then(AppRuntime::board);
        let mut walls = DependencyWalls::new(work);
        let tasks = work
            .tasks()
            .iter()
            .enumerate()
            .filter(|(_, task)| selection.tasks().contains(&task.row().statement.body.born))
            .map(|(index, task)| WalledTask {
                task,
                requirements: task
                    .row()
                    .statement
                    .body
                    .requirements
                    .as_slice()
                    .iter()
                    .filter(|requirement| requirement_blocked(board, requirement))
                    .collect(),
                dependencies: task
                    .dependencies()
                    .iter()
                    .filter_map(|dependency| {
                        walls
                            .wall(index, dependency)
                            .map(|wall| (dependency.born(), wall))
                    })
                    .collect(),
            })
            .filter(|task| task.count() > 0)
            .collect::<Vec<_>>();
        let count = tasks.iter().map(WalledTask::count).sum();
        Self {
            tasks,
            count,
            selection,
        }
    }

    pub(super) fn render_rail(&self, review: &UseStateHandle<bool>, launcher: &NodeRef) -> Html {
        let active = self.count > 0;
        let summary = if active {
            format!(
                "{} {} across {} selected {} need attention.",
                self.count,
                if self.count == 1 {
                    "blocker"
                } else {
                    "blockers"
                },
                self.tasks.len(),
                if self.tasks.len() == 1 {
                    "task"
                } else {
                    "tasks"
                },
            )
        } else {
            "Nothing in this selection waits on you.".to_owned()
        };
        let open = {
            let review = review.clone();
            Callback::from(move |_| review.set(true))
        };
        html! {
            <section
                class={classes!("work-blocker-rail", active.then_some("active"))}
                data-blocker-count={self.count.to_string()}
                aria-label="Selection blockers"
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
                <button
                    ref={launcher.clone()}
                    type="button"
                    class="app-compact-control work-blocker-review-open"
                    disabled={!active}
                    aria-haspopup="dialog"
                    onclick={open}
                >
                    {if active { "Review blockers" } else { "No blockers" }}
                </button>
            </section>
        }
    }

    pub(super) fn render_review(
        &self,
        runtime: Option<&AppRuntime>,
        review: &UseStateHandle<bool>,
        close_ref: &NodeRef,
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
        let resolves = |task: &wire::TaskHandle| {
            task.born()
                .is_ok_and(|stamp| self.selection.tasks().contains(&stamp))
        };
        let controls = LineControls::Live {
            resolves: &resolves,
        };
        html! {
            <section
                class="work-blocker-review app-sheet-overlay"
                role="dialog"
                aria-modal="true"
                aria-labelledby="work-blocker-review-title"
                data-blocker-count={self.count.to_string()}
            >
                <header class="app-sheet-header">
                    <div>
                        <p class="app-sheet-kicker">{"Needs attention"}</p>
                        <h2 id="work-blocker-review-title">
                            {format!("{} selection {}", self.count, if self.count == 1 { "blocker" } else { "blockers" })}
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
                        {"Only walls you must move appear here. Work waiting on tasks an agent can claim or is working proceeds on its own."}
                    </p>
                    {for self.tasks.iter().map(|task| render_task(task, &controls, runtime, can_grant, grant))}
                </div>
            </section>
        }
    }
}

fn requirement_blocked(
    board: Option<&crate::runtime::BoardView>,
    requirement: &wire::EnvironmentRequirement,
) -> bool {
    board.is_none_or(|board| !board.requirement_satisfied(requirement))
}

fn render_task(
    walled: &WalledTask<'_, '_>,
    controls: &LineControls<'_>,
    runtime: Option<&AppRuntime>,
    can_grant: bool,
    grant: &Callback<task_detail::TrustGrantRequest>,
) -> Html {
    let row = walled.task.row();
    let statement = &row.statement.body;
    html! {
        <article class="work-blocker-task" data-task-stamp={statement.born.to_string()}>
            <header>
                <p class="app-sheet-kicker">{"Blocked task"}</p>
                <h3>{task_detail::render_task_title(&row.presentation_title, controls)}</h3>
                <code>{statement.handle.to_string()}</code>
            </header>
            <div class="work-blocker-list">
                {for walled.requirements.iter()
                    .map(|requirement| render_requirement(statement, requirement, runtime, can_grant, grant))}
                {for walled.dependencies.iter().map(|(dependency, wall)| render_dependency(dependency, *wall))}
            </div>
        </article>
    }
}

/// Name the task where the chain stops and why only the operator can move it.
fn render_dependency(dependency: &TaskStamp, wall: DependencyWall<'_>) -> Html {
    let via = |task: &TaskStamp| {
        if task == dependency {
            format!("Task {task}")
        } else {
            format!("Task {dependency} waits on task {task}, which")
        }
    };
    let (kind, title, reason, state) = match wall {
        DependencyWall::Deferred(task) => (
            "deferred",
            "Deferred prerequisite",
            format!(
                "{} is deferred; no agent is offered it until it is woken.",
                via(task)
            ),
            "Deferred",
        ),
        DependencyWall::Parked(task) => (
            "parked",
            "Parked prerequisite",
            format!(
                "{} is unfiled OOPS or Draft work; no agent is offered it until it is filed.",
                via(task)
            ),
            "Parked",
        ),
        DependencyWall::Cycle(task) => (
            "cycle",
            "Dependency cycle",
            format!(
                "Task {dependency} leads back to task {task}; no agent can finish the chain until its dependencies change."
            ),
            "Cycle",
        ),
        DependencyWall::Unreachable(task) => (
            "unreachable",
            "Prerequisite cannot complete",
            format!(
                "{} was closed without completing, so the dependency must change.",
                via(task)
            ),
            "Unreachable",
        ),
        DependencyWall::Environment(task) => (
            "environment",
            "Prerequisite needs the operator",
            format!(
                "{} needs a trust grant or upgrade only the operator can provide.",
                via(task)
            ),
            "Operator required",
        ),
        DependencyWall::Selection(task) => (
            "selection",
            "Prerequisite awaits selection",
            format!(
                "{} is a Design awaiting the operator's exact selection of its outputs, or the closure of each unselected one.",
                via(task)
            ),
            "Operator required",
        ),
        DependencyWall::Passive(task) => (
            "passive",
            "Prerequisite is a passive Design",
            format!(
                "{} is a Design outside every working group; no lane can acquire it until the operator moves it into one.",
                via(task)
            ),
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
