//! Pure presentation rules for redacted provider quota generations.

use wire::{QuotaAttempt, QuotaFailureKind, QuotaGeneration};

/// Operator-visible sampler condition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Condition {
    NeverSampled,
    Refreshing,
    Current,
    StaleLastGood,
    Unsupported,
    Failed,
}

impl Condition {
    pub(crate) const fn key(self) -> &'static str {
        match self {
            Self::NeverSampled => "never-sampled",
            Self::Refreshing => "refreshing",
            Self::Current => "current",
            Self::StaleLastGood => "stale-last-good",
            Self::Unsupported => "unsupported",
            Self::Failed => "failed-attempt",
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::NeverSampled => "Not sampled yet",
            Self::Refreshing => "Refreshing",
            Self::Current => "Current",
            Self::StaleLastGood => "Last good sample; refresh failed",
            Self::Unsupported => "Quota sampling unsupported",
            Self::Failed => "Quota sample failed",
        }
    }
}

/// Distinguishable words and chronology for one generation.
pub(crate) struct Presentation {
    pub(crate) condition: Condition,
    pub(crate) detail: &'static str,
    pub(crate) sampled_at_ms: Option<i64>,
}

pub(crate) fn present(generation: &QuotaGeneration) -> Presentation {
    let sampled_at_ms = generation
        .last_good
        .as_ref()
        .map(|snapshot| snapshot.generated_at_ms);
    let failed = matches!(generation.latest_attempt, Some(QuotaAttempt::Failed { .. }));
    let condition = if generation.refreshing {
        Condition::Refreshing
    } else if sampled_at_ms.is_some() && failed {
        Condition::StaleLastGood
    } else if sampled_at_ms.is_some() {
        Condition::Current
    } else if matches!(
        generation.latest_attempt,
        Some(QuotaAttempt::Failed {
            kind: QuotaFailureKind::ExecutableUnavailable,
            ..
        })
    ) {
        Condition::Unsupported
    } else if failed {
        Condition::Failed
    } else {
        Condition::NeverSampled
    };
    let detail = match (condition, sampled_at_ms.is_some()) {
        (Condition::Refreshing, true) => {
            "Refreshing; retained data remains visible and continues to age."
        }
        (Condition::Refreshing, false) => "The first provider quota sample is running.",
        (Condition::Current, _) => "The most recent sample completed successfully.",
        (Condition::StaleLastGood, _) => {
            "The latest refresh failed; retained successful data remains visible and continues to age."
        }
        (Condition::Unsupported, _) => "The configured OMP executable cannot provide quota usage.",
        (Condition::Failed, _) => "No successful provider quota sample is available.",
        (Condition::NeverSampled, _) => "The resident has not completed a provider quota sample.",
    };
    Presentation {
        condition,
        detail,
        sampled_at_ms,
    }
}

pub(crate) fn failure_label(kind: QuotaFailureKind) -> &'static str {
    match kind {
        QuotaFailureKind::ExecutableUnavailable => "executable unavailable",
        QuotaFailureKind::Spawn => "process launch failed",
        QuotaFailureKind::Timeout => "sample timed out",
        QuotaFailureKind::StdoutOverflow => "usage output exceeded its bound",
        QuotaFailureKind::StderrOverflow => "diagnostic output exceeded its bound",
        QuotaFailureKind::NonzeroExit => "OMP returned a nonzero exit",
        QuotaFailureKind::MalformedOutput => "OMP returned malformed usage data",
        QuotaFailureKind::OutputIncomplete => "OMP output did not drain completely",
        QuotaFailureKind::Supervisor => "process supervision failed",
        QuotaFailureKind::WorkerStart => "sampler worker could not start",
        QuotaFailureKind::WorkerLost => "sampler worker ended without a result",
    }
}

pub(crate) fn account_label(account: &wire::QuotaAccount) -> String {
    account
        .email
        .as_ref()
        .or(account.account_id.as_ref())
        .or(account.org_name.as_ref())
        .or(account.org_id.as_ref())
        .or(account.project_id.as_ref())
        .cloned()
        .unwrap_or_else(|| "Redacted account".to_owned())
}

pub(crate) fn capacity_summary(generation: &QuotaGeneration) -> String {
    let Some(snapshot) = generation.last_good.as_ref() else {
        return present(generation).condition.label().to_owned();
    };
    weekly_remaining(snapshot).map_or_else(
        || "No provider capacity reported".to_owned(),
        |remaining| format!("{:.0}% remaining", remaining * 100.0),
    )
}

/// Weekly remaining capacity across every account as one fraction in
/// `0..=1`, each account weighted by its plan capacity; `None` when no
/// account reports a weekly window.
pub(crate) fn weekly_remaining(snapshot: &wire::QuotaSnapshot) -> Option<f64> {
    let (remaining, capacity) = snapshot
        .reports
        .iter()
        .filter_map(|report| {
            let remaining = binding_remaining_fraction(report)?;
            let capacity = plan_capacity_units(report);
            Some((remaining * capacity, capacity))
        })
        .fold((0.0, 0.0), |(remaining, capacity), account| {
            (remaining + account.0, capacity + account.1)
        });
    (capacity > 0.0).then(|| (remaining / capacity).clamp(0.0, 1.0))
}

fn binding_remaining_fraction(report: &wire::QuotaReport) -> Option<f64> {
    report
        .limits
        .iter()
        .filter(|limit| limit.scope.shared == Some(true) && is_weekly_limit(limit))
        .filter_map(limit_remaining_fraction)
        .reduce(f64::min)
}

fn is_weekly_limit(limit: &wire::QuotaLimit) -> bool {
    const WEEK_MS: u64 = 7 * 24 * 60 * 60 * 1_000;

    limit.window.as_ref().is_some_and(|window| {
        window.duration_ms == Some(WEEK_MS) || is_weekly_id(window.id.as_str())
    }) || limit.scope.window_id.as_deref().is_some_and(is_weekly_id)
}

fn is_weekly_id(id: &str) -> bool {
    ["7d", "7-day", "7-days", "weekly"]
        .iter()
        .any(|candidate| id.eq_ignore_ascii_case(candidate))
}

pub(crate) fn limit_remaining_fraction(limit: &wire::QuotaLimit) -> Option<f64> {
    let amount = &limit.amount;
    parse_quantity(amount.remaining_fraction.as_ref())
        .or_else(|| parse_quantity(amount.used_fraction.as_ref()).map(|used| 1.0 - used))
        .or_else(|| {
            let remaining = parse_quantity(amount.remaining.as_ref())?;
            let limit = parse_quantity(amount.limit.as_ref())?;
            (limit > 0.0).then_some(remaining / limit)
        })
        .filter(|fraction| fraction.is_finite())
        .map(|fraction| fraction.clamp(0.0, 1.0))
}

fn parse_quantity(value: Option<&String>) -> Option<f64> {
    value?.parse::<f64>().ok().filter(|value| value.is_finite())
}

fn plan_capacity_units(report: &wire::QuotaReport) -> f64 {
    const FREE: f64 = 0.2;
    const BASELINE: f64 = 1.0;
    const LOWER_PRO: f64 = 5.0;
    const HIGH_PRO: f64 = 20.0;

    let Some(plan) = report.account.plan_type.as_deref().map(str::trim) else {
        return BASELINE;
    };
    if plan_matches(
        plan,
        &[
            "20x",
            "max20",
            "max20x",
            "max_20x",
            "max-20x",
            "max 20x",
            "pro20",
            "pro20x",
            "pro_20x",
            "pro-20x",
            "pro 20x",
            "chatgpt_pro_20x",
        ],
    ) {
        return HIGH_PRO;
    }
    if plan_matches(plan, &["free", "chatgpt_free", "go", "chatgpt_go"]) {
        return FREE;
    }
    if plan_matches(
        plan,
        &[
            "5x",
            "max",
            "max5",
            "max5x",
            "max_5x",
            "max-5x",
            "max 5x",
            "pro5",
            "pro5x",
            "pro_5x",
            "pro-5x",
            "pro 5x",
            "chatgpt_pro_5x",
        ],
    ) {
        return LOWER_PRO;
    }
    if report.provider.eq_ignore_ascii_case("openai-codex")
        && plan_matches(
            plan,
            &[
                "pro",
                "chatgpt_pro",
                "prolite",
                "pro_lite",
                "pro-lite",
                "pro lite",
            ],
        )
    {
        return LOWER_PRO;
    }
    BASELINE
}

fn plan_matches(plan: &str, candidates: &[&str]) -> bool {
    candidates
        .iter()
        .any(|candidate| plan.eq_ignore_ascii_case(candidate))
}
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn sampler_conditions_keep_retained_failure_distinct() {
        let mut generation = QuotaGeneration::default();
        assert_eq!(present(&generation).condition, Condition::NeverSampled);

        generation.refreshing = true;
        assert_eq!(present(&generation).condition, Condition::Refreshing);
        generation.refreshing = false;
        generation.latest_attempt = Some(QuotaAttempt::Failed {
            finished_at_ms: 20,
            kind: QuotaFailureKind::ExecutableUnavailable,
            exit_code: None,
        });
        assert_eq!(present(&generation).condition, Condition::Unsupported);
        generation.latest_attempt = Some(QuotaAttempt::Failed {
            finished_at_ms: 21,
            kind: QuotaFailureKind::Timeout,
            exit_code: None,
        });
        assert_eq!(present(&generation).condition, Condition::Failed);

        generation.last_good = Some(Arc::new(snapshot()));
        assert_eq!(present(&generation).condition, Condition::StaleLastGood);
        assert_eq!(present(&generation).sampled_at_ms, Some(10));

        generation.latest_attempt = Some(QuotaAttempt::Succeeded { finished_at_ms: 30 });
        assert_eq!(present(&generation).condition, Condition::Current);
    }

    #[test]
    fn aggregate_uses_plan_capacity_and_only_each_accounts_weekly_window() {
        let generation = QuotaGeneration {
            generation: 4,
            refreshing: false,
            last_good: Some(Arc::new(snapshot())),
            latest_attempt: Some(QuotaAttempt::Failed {
                finished_at_ms: 40,
                kind: QuotaFailureKind::Timeout,
                exit_code: None,
            }),
        };
        assert_eq!(capacity_summary(&generation), "14% remaining");
        assert_eq!(present(&generation).condition, Condition::StaleLastGood);
    }

    fn snapshot() -> wire::QuotaSnapshot {
        const FIVE_HOURS_MS: u64 = 5 * 60 * 60 * 1_000;
        const WEEK_MS: u64 = 7 * 24 * 60 * 60 * 1_000;

        wire::QuotaSnapshot {
            generated_at_ms: 10,
            reports: vec![
                report("openai-codex", Some("free"), &[("weekly", None, "1")]),
                report("anthropic", None, &[("provider-week", Some(WEEK_MS), "1")]),
                report(
                    "openai-codex",
                    Some("pro"),
                    &[
                        ("5h", Some(FIVE_HOURS_MS), "0"),
                        ("7d", Some(WEEK_MS), "0.5"),
                    ],
                ),
                report("openai-codex", Some("pro_20x"), &[("7-days", None, "0")]),
            ],
            accounts_without_usage: Vec::new(),
            disabled_credentials: Vec::new(),
            capacity: Vec::new(),
        }
    }

    fn report(
        provider: &str,
        plan_type: Option<&str>,
        limits: &[(&str, Option<u64>, &str)],
    ) -> wire::QuotaReport {
        let account = wire::QuotaAccount {
            plan_type: plan_type.map(ToOwned::to_owned),
            ..wire::QuotaAccount::default()
        };
        wire::QuotaReport {
            provider: provider.to_owned(),
            fetched_at_ms: 10,
            account,
            limits: limits
                .iter()
                .enumerate()
                .map(
                    |(index, (window_id, duration_ms, remaining))| wire::QuotaLimit {
                        id: format!("{provider}:{index}"),
                        label: format!("Window {index}"),
                        scope: wire::QuotaScope {
                            provider: provider.to_owned(),
                            account_id: None,
                            project_id: None,
                            org_id: None,
                            model_id: None,
                            tier: None,
                            window_id: Some((*window_id).to_owned()),
                            shared: Some(true),
                            shared_group: None,
                        },
                        window: Some(wire::QuotaWindow {
                            id: (*window_id).to_owned(),
                            label: format!("Window {index}"),
                            duration_ms: *duration_ms,
                            resets_at: None,
                            reset_label: None,
                        }),
                        amount: wire::QuotaAmount {
                            used: None,
                            limit: None,
                            remaining: None,
                            used_fraction: None,
                            remaining_fraction: Some((*remaining).to_owned()),
                            unit: "percent".to_owned(),
                        },
                        status: None,
                    },
                )
                .collect(),
            reset_credits: None,
        }
    }
}
