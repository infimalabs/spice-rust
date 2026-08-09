use wire::Instant;
use wire::body::{self, TranscriptPosition};
use wire::intent::{EffortPhaseAnswer, EffortUnavailable, EffortUnavailableCode, EffortUsage};

pub(super) fn sample_locus(
    sample: &body::TranscriptUsage,
    batch: &body::JournalIngested,
    reasons: &mut Vec<EffortUnavailable>,
) -> Option<(TranscriptPosition, TranscriptPosition)> {
    let Some(first) = sample.spans.first() else {
        push_reason(
            reasons,
            EffortUnavailableCode::MalformedUsage,
            "usage sample has no source span",
        );
        return None;
    };
    let last = sample.spans.last()?;
    let Some(end) = last.offset.checked_add(last.length) else {
        push_reason(
            reasons,
            EffortUnavailableCode::ArithmeticOverflow,
            "usage span end overflowed u64",
        );
        return None;
    };
    let from = TranscriptPosition {
        member: first.member,
        offset: first.offset,
    };
    let to = TranscriptPosition {
        member: last.member,
        offset: end,
    };
    let ordered = sample.spans.windows(2).all(|pair| {
        let left_end = pair[0].offset.checked_add(pair[0].length);
        left_end
            .is_some_and(|left_end| (pair[0].member, left_end) <= (pair[1].member, pair[1].offset))
    });
    if !ordered || position(from) < position(batch.from) || position(to) > position(batch.to) {
        push_reason(
            reasons,
            EffortUnavailableCode::MalformedUsage,
            "usage spans are unordered or cross their accounted batch",
        );
        return None;
    }
    Some((from, to))
}

pub(super) fn usage(sample: &body::TranscriptUsage) -> EffortUsage {
    EffortUsage {
        turns: sample.turns,
        input_tokens: sample.input_tokens,
        cached_input_tokens: sample.cached_input_tokens,
        cache_write_input_tokens: sample.cache_write_input_tokens,
        output_tokens: sample.output_tokens,
        reasoning_output_tokens: sample.reasoning_output_tokens,
    }
}

pub(super) fn zero_usage() -> EffortUsage {
    EffortUsage {
        reasoning_output_tokens: Some(0),
        ..EffortUsage::default()
    }
}

pub(super) fn add_usage(left: &EffortUsage, right: &EffortUsage) -> Option<EffortUsage> {
    Some(EffortUsage {
        turns: left.turns.checked_add(right.turns)?,
        input_tokens: left.input_tokens.checked_add(right.input_tokens)?,
        cached_input_tokens: left
            .cached_input_tokens
            .checked_add(right.cached_input_tokens)?,
        cache_write_input_tokens: left
            .cache_write_input_tokens
            .checked_add(right.cache_write_input_tokens)?,
        output_tokens: left.output_tokens.checked_add(right.output_tokens)?,
        reasoning_output_tokens: match (left.reasoning_output_tokens, right.reasoning_output_tokens)
        {
            (Some(left), Some(right)) => Some(left.checked_add(right)?),
            _ => None,
        },
    })
}

pub(super) fn divide_usage(total: &EffortUsage, count: u64) -> EffortUsage {
    EffortUsage {
        turns: total.turns / count,
        input_tokens: total.input_tokens / count,
        cached_input_tokens: total.cached_input_tokens / count,
        cache_write_input_tokens: total.cache_write_input_tokens / count,
        output_tokens: total.output_tokens / count,
        reasoning_output_tokens: total.reasoning_output_tokens.map(|value| value / count),
    }
}

pub(super) fn first_locus(phase: &EffortPhaseAnswer) -> (u64, u64) {
    phase
        .segments
        .first()
        .map_or((0, 0), |segment| position(segment.from))
}

pub(super) const fn position(value: TranscriptPosition) -> (u64, u64) {
    (value.member, value.offset)
}

pub(super) fn read<F: wire::Body>(envelope: &wire::Envelope<wire::AnyFact>) -> F {
    envelope
        .fact
        .read()
        .expect("fact kind selected its registered body")
}

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

pub(super) fn epoch() -> Instant {
    "1970-01-01T00:00:00.000Z"
        .parse()
        .expect("epoch spelling is canonical")
}
