//! Stable task-overview home for environmental and dependency blockers.

use super::*;
use crate::runtime::BlockerActionState;
use crate::work::{TaskOverviewSelection, WorkProjection, WorkTask};

pub(super) struct BlockerPresentation<'view, 'board> {
    work: &'view WorkProjection<'board>,
    selection: &'view TaskOverviewSelection,
    board: Option<&'view crate::runtime::BoardView>,
    task_count: usize,
    count: usize,
}

impl<'view, 'board> BlockerPresentation<'view, 'board> {
    pub(super) fn new(
        work: &'view WorkProjection<'board>,
        selection: &'view TaskOverviewSelection,
        runtime: Option<&'view AppRuntime>,
    ) -> Self {
        let board = runtime.and_then(AppRuntime::board);
        let (count, task_count) = work
            .tasks()
            .iter()
            .filter(|task| selection.tasks().contains(&task.row().statement.body.born))
            .map(|task| {
                task.blocked_dependencies().count()
                    + task
                        .row()
                        .statement
                        .body
                        .requirements
                        .as_slice()
                        .iter()
                        .filter(|requirement| requirement_blocked(board, requirement))
                        .count()
            })
            .fold((0, 0), |(count, task_count), blockers| {
                (count + blockers, task_count + usize::from(blockers > 0))
            });
        Self {
            work,
            selection,
            board,
            count,
            task_count,
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
                self.task_count,
                if self.task_count == 1 {
                    "task"
                } else {
                    "tasks"
                },
            )
        } else {
            "No active blockers in this selection.".to_owned()
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
        html! {
            <section
                class="work-blocker-review"
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
                <div class="app-sheet-body work-blocker-review-body">
                    <p class="work-blocker-review-intro">
                        {"Resolve an actionable requirement here. Dependency and runtime requirements remain visible without offering controls that cannot help."}
                    </p>
                    {for self.work.tasks().iter()
                        .filter(|task| self.selection.tasks().contains(&task.row().statement.body.born))
                        .filter(|task| {
                            task.blocked_dependencies().next().is_some()
                                || task.row().statement.body.requirements.as_slice().iter()
                                    .any(|requirement| requirement_blocked(self.board, requirement))
                        })
                        .map(|task| render_task(task, self.board, runtime, can_grant, grant))}
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
    task: &WorkTask<'_>,
    board: Option<&crate::runtime::BoardView>,
    runtime: Option<&AppRuntime>,
    can_grant: bool,
    grant: &Callback<task_detail::TrustGrantRequest>,
) -> Html {
    let row = task.row();
    let statement = &row.statement.body;
    html! {
        <article class="work-blocker-task" data-task-stamp={statement.born.to_string()}>
            <header>
                <p class="app-sheet-kicker">{"Blocked task"}</p>
                <h3>{row.presentation_title.clone()}</h3>
                <code>{statement.handle.to_string()}</code>
            </header>
            <div class="work-blocker-list">
                {for statement.requirements.as_slice().iter()
                    .filter(|requirement| requirement_blocked(board, requirement))
                    .map(|requirement| render_requirement(statement, requirement, runtime, can_grant, grant))}
                {for task.blocked_dependencies().map(|dependency| html! {
                    <section class="work-blocker-card" data-blocker-kind="dependency">
                        <div>
                            <strong>{"Incomplete dependency"}</strong>
                            <p>{format!("Task {} must complete before this task can proceed.", dependency.born())}</p>
                        </div>
                        <span class="work-blocker-state">{"Waiting"}</span>
                    </section>
                })}
            </div>
        </article>
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
                    <p>{format!("Requires schema {} or newer. Upgrade actions are not available in the app yet.", value)}</p>
                </div>
                <span class="work-blocker-state">{"Upgrade required"}</span>
            </section>
        },
        wire::EnvironmentRequirement::ApplianceVersion { value } => html! {
            <section class="work-blocker-card" data-blocker-kind="appliance-version">
                <div>
                    <strong>{"Newer appliance required"}</strong>
                    <p>{format!("Requires a version later than {value}. Reload and upgrade actions are not available in the app yet.")}</p>
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
    let state = runtime.and_then(|runtime| runtime.blocker_action(digest));
    let pending = matches!(
        state,
        Some(BlockerActionState::Pending | BlockerActionState::Approved)
    );
    let action_label = match state {
        Some(BlockerActionState::Pending) => "Approving…",
        Some(BlockerActionState::Approved) => "Approved · refreshing…",
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
                <p>{"Approve only if this exact configuration digest is expected for the producing worktree."}</p>
                <code title={digest.to_string()}>{digest.to_string()}</code>
                if let Some(BlockerActionState::Failed(issue)) = state {
                    <p class="work-blocker-issue" role="alert">{issue.clone()}</p>
                } else if pending {
                    <p class="work-blocker-progress" role="status">{action_label}</p>
                }
            </div>
            <button
                type="button"
                class="app-compact-control"
                disabled={!can_grant || pending}
                onclick={approve}
            >{action_label}</button>
        </section>
    }
}
