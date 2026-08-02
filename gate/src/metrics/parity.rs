use std::error::Error;
use std::fmt;

use super::{EvidenceState, MetricEvidence, MetricId, MetricKey};
use crate::manifest::PREDECESSOR_GATE_RELEASE;

/// One predecessor decision captured before one-shot migration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PredecessorMetricObservation {
    metric: MetricId,
    key: MetricKey,
    result: PredecessorMetricResult,
}

impl PredecessorMetricObservation {
    #[must_use]
    pub const fn new(metric: MetricId, key: MetricKey, result: PredecessorMetricResult) -> Self {
        Self {
            metric,
            key,
            result,
        }
    }
}

/// Predecessor outcomes that have deterministic current meaning.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PredecessorMetricResult {
    Measured(u64),
    NotApplicable { reason: String },
}

/// Supported equivalence or an exact release-owned boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MetricParity {
    Equivalent {
        metric: MetricId,
        key: MetricKey,
        value: Option<u64>,
    },
    Refused {
        metric: MetricId,
        key: MetricKey,
        source_field: String,
        owning_release: String,
        recovery: String,
    },
}

/// Compare one captured predecessor result with one admitted current row.
///
/// # Errors
///
/// Moved identities, values, applicability, incomplete evidence, and refusals
/// without the exact owning release are rejected rather than softened.
pub fn compare_predecessor_metric(
    predecessor: &PredecessorMetricObservation,
    current: &MetricEvidence,
) -> Result<MetricParity, ParityError> {
    if predecessor.metric != current.metric() || &predecessor.key != current.key() {
        return Err(ParityError::Identity {
            expected_metric: predecessor.metric,
            expected_key: predecessor.key.clone(),
            observed_metric: current.metric(),
            observed_key: current.key().clone(),
        });
    }
    if let EvidenceState::Unsupported {
        source_field,
        owning_release,
        recovery,
        ..
    } = current.state()
    {
        if owning_release != PREDECESSOR_GATE_RELEASE {
            return Err(ParityError::RefusalOwner {
                observed: owning_release.clone(),
            });
        }
        return Ok(MetricParity::Refused {
            metric: current.metric(),
            key: current.key().clone(),
            source_field: source_field.clone(),
            owning_release: owning_release.clone(),
            recovery: recovery.clone(),
        });
    }
    match (&predecessor.result, current.state()) {
        (PredecessorMetricResult::Measured(expected), EvidenceState::Measured { measurement })
            if *expected == measurement.value() =>
        {
            Ok(MetricParity::Equivalent {
                metric: current.metric(),
                key: current.key().clone(),
                value: Some(*expected),
            })
        }
        (PredecessorMetricResult::Measured(expected), EvidenceState::Measured { measurement }) => {
            Err(ParityError::Value {
                expected: *expected,
                observed: measurement.value(),
            })
        }
        (PredecessorMetricResult::NotApplicable { .. }, EvidenceState::NotApplicable { .. }) => {
            Ok(MetricParity::Equivalent {
                metric: current.metric(),
                key: current.key().clone(),
                value: None,
            })
        }
        (_, state) => Err(ParityError::Decision {
            predecessor: format!("{:?}", predecessor.result),
            current: format!("{state:?}"),
        }),
    }
}

/// Why a predecessor/current comparison did not prove equivalence or refusal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParityError {
    Identity {
        expected_metric: MetricId,
        expected_key: MetricKey,
        observed_metric: MetricId,
        observed_key: MetricKey,
    },
    Value {
        expected: u64,
        observed: u64,
    },
    Decision {
        predecessor: String,
        current: String,
    },
    RefusalOwner {
        observed: String,
    },
}

impl fmt::Display for ParityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Identity {
                expected_metric,
                expected_key,
                observed_metric,
                observed_key,
            } => write!(
                formatter,
                "expected {} {expected_key:?}, observed {} {observed_key:?}",
                expected_metric.as_str(),
                observed_metric.as_str()
            ),
            Self::Value { expected, observed } => {
                write!(
                    formatter,
                    "expected measurement {expected}, observed {observed}"
                )
            }
            Self::Decision {
                predecessor,
                current,
            } => write!(
                formatter,
                "predecessor decision {predecessor} does not equal current state {current}"
            ),
            Self::RefusalOwner { observed } => write!(
                formatter,
                "refusal owner {observed:?} is not predecessor release {PREDECESSOR_GATE_RELEASE}"
            ),
        }
    }
}

impl Error for ParityError {}
