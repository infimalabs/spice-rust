//! Typed unavailable reasons accumulated while task effort is derived.

use wire::intent::{EffortUnavailable, EffortUnavailableCode};

pub(super) fn push_reason(
    reasons: &mut Vec<EffortUnavailable>,
    code: EffortUnavailableCode,
    detail: impl Into<String>,
) {
    reasons.push(reason(code, detail));
}

pub(super) fn reason(code: EffortUnavailableCode, detail: impl Into<String>) -> EffortUnavailable {
    EffortUnavailable {
        code,
        detail: detail.into(),
    }
}
