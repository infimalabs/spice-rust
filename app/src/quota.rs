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
    let rows = snapshot
        .capacity
        .iter()
        .flat_map(|provider| {
            provider.windows.iter().map(|window| {
                format!(
                    "{} {}: {} remaining",
                    provider.provider, window.window, window.remaining_accounts
                )
            })
        })
        .collect::<Vec<_>>();
    if rows.is_empty() {
        "No provider capacity reported".to_owned()
    } else {
        rows.join(" · ")
    }
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
    fn mixed_provider_capacity_names_every_provider_window() {
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
        assert_eq!(
            capacity_summary(&generation),
            "anthropic five-hour: 1.5 remaining · openai-codex seven-day: 0.25 remaining"
        );
        assert_eq!(present(&generation).condition, Condition::StaleLastGood);
    }

    fn snapshot() -> wire::QuotaSnapshot {
        wire::QuotaSnapshot {
            generated_at_ms: 10,
            reports: Vec::new(),
            accounts_without_usage: Vec::new(),
            disabled_credentials: Vec::new(),
            capacity: vec![
                wire::QuotaProviderCapacity {
                    provider: "anthropic".to_owned(),
                    windows: vec![capacity("five-hour", "1.5")],
                },
                wire::QuotaProviderCapacity {
                    provider: "openai-codex".to_owned(),
                    windows: vec![capacity("seven-day", "0.25")],
                },
            ],
        }
    }

    fn capacity(window: &str, remaining: &str) -> wire::QuotaCapacity {
        wire::QuotaCapacity {
            window: window.to_owned(),
            duration_ms: None,
            meter: None,
            accounts: 2,
            used_accounts: "0.5".to_owned(),
            remaining_accounts: remaining.to_owned(),
        }
    }
}
