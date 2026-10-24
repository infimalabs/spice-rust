use std::error::Error;
use std::fmt;

use super::declaration::metric_parser_declaration;
use super::{
    EvidenceError, EvidenceState, LatchBinding, LatchLimits, LatchOutcome, MetricEvidence,
    MetricId, MetricKey, MetricMeasurement, ParserEvidence, ResolvedMetricRule,
};
use crate::state::{
    ActorInputs, ActorSource, BoundDecision, BoundError, BoundRule, KeyError, LatchEntry,
    LatchKind, LatchObservation, LatchTransition, StateDecision, StateKey,
};

/// One bounded structural measurement compiled for current state reconciliation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetricStateBinding {
    metric: MetricId,
    key: MetricKey,
    value: u64,
    parser: ParserEvidence,
    rule: ResolvedMetricRule,
    entry: LatchEntry,
    bound: BoundRule,
    variation_percent: u64,
}

impl MetricStateBinding {
    /// Bind one parsed value and resolved policy rule to the sole current
    /// durable-state key and bound vocabulary.
    pub fn new(
        metric: MetricId,
        key: MetricKey,
        value: u64,
        parser: ParserEvidence,
        rule: ResolvedMetricRule,
    ) -> Result<Self, StateBindingError> {
        if rule.metric() != metric {
            return Err(StateBindingError::RuleMetric {
                expected: metric,
                observed: rule.metric(),
            });
        }
        let expected_parser = metric_parser_declaration(metric);
        if !parser.matches_declaration(expected_parser) {
            return Err(StateBindingError::Parser(format!(
                "{} parser evidence does not match {}",
                parser.parser().as_str(),
                expected_parser.id().as_str()
            )));
        }
        let kind = latch_kind(metric).ok_or(StateBindingError::UnboundedMetric(metric))?;
        let state_key = state_key(&key)?;
        let entry = LatchEntry::new(kind, state_key).map_err(StateBindingError::Key)?;
        let variation_percent = rule.variation_percent();
        let bound = BoundRule::new(
            rule_identity(&rule),
            metric.as_str(),
            rule.base_limit(),
            rule.ratchet_limit(),
            rule.unlimited(),
            false,
        )
        .map_err(StateBindingError::Bound)?;
        Ok(Self {
            metric,
            key,
            value,
            parser,
            rule,
            entry,
            bound,
            variation_percent,
        })
    }

    #[must_use]
    pub const fn metric(&self) -> MetricId {
        self.metric
    }

    #[must_use]
    pub const fn key(&self) -> &MetricKey {
        &self.key
    }

    #[must_use]
    pub const fn entry(&self) -> &LatchEntry {
        &self.entry
    }

    /// Produce the exact observation consumed by [`crate::state::CurrentState`].
    pub fn observation(
        &self,
        actor_inputs: ActorInputs,
    ) -> Result<LatchObservation, StateBindingError> {
        LatchObservation::new(
            self.entry.clone(),
            self.bound.clone(),
            self.value,
            actor_inputs,
            self.variation_percent,
        )
        .map_err(StateBindingError::Bound)
    }

    /// Decide this bound directly against one known latch state.
    ///
    /// [`Self::observation`] hands the same rule to
    /// [`crate::state::CurrentState`], which reads `latched` from the ledger it
    /// is about to rewrite and returns the decision inside a transaction. An
    /// acquisition that only measures needs the decision without the rewrite,
    /// so it reads the latch itself and decides here. Both routes call the one
    /// [`BoundRule`] compiled above, so neither can compute a limit the other
    /// would not.
    ///
    /// # Errors
    ///
    /// Returns the bound's own refusal, which for a rule requiring an actor is
    /// provenance that names none.
    pub fn decide(
        &self,
        latched: bool,
        actor_inputs: &ActorInputs,
    ) -> Result<BoundDecision, StateBindingError> {
        self.bound
            .decide(self.value, latched, actor_inputs, self.variation_percent)
            .map_err(StateBindingError::Bound)
    }

    /// Retain the exact transaction observation beside the decision used by
    /// acquisition evidence.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn observe_decision(
        &self,
        latched: bool,
        actor_inputs: &ActorInputs,
    ) -> Result<(BoundDecision, LatchObservation), StateBindingError> {
        let observation = self
            .observation(actor_inputs.clone())?
            .with_prior_latched(latched);
        self.decide(latched, actor_inputs)
            .map(|decision| (decision, observation))
    }

    /// Convert a reconciled current-state decision into complete metric evidence.
    pub fn evidence(&self, decision: &StateDecision) -> Result<MetricEvidence, StateBindingError> {
        if decision.entry() != &self.entry {
            return Err(StateBindingError::Decision(
                "state decision entry does not match the compiled metric key".to_owned(),
            ));
        }
        self.measured(decision.bound())
    }

    /// Convert a bound decided by [`Self::decide`] into complete metric evidence.
    ///
    /// [`Self::evidence`] is this same conversion for the reconciling route,
    /// which owns a [`StateDecision`] because it rewrote the ledger. An arm that
    /// only measures holds the [`BoundDecision`] [`Self::decide`] handed back
    /// and has no state decision to present, so it converts from that instead.
    /// Both arrive at the one private `measurement` step and attach the same
    /// rule and latch bindings to the same measured value, so the row an arm
    /// reports and the row the state transaction would report cannot disagree
    /// about the limit that value was judged against.
    ///
    /// # Errors
    ///
    /// Refuses a decision that disagrees with this binding's metric, value, or
    /// resolved rule, and refuses evidence its own declaration rejects.
    pub fn measured(&self, decision: &BoundDecision) -> Result<MetricEvidence, StateBindingError> {
        let measurement = self.measurement(decision)?;
        MetricEvidence::new(
            self.metric,
            self.key.clone(),
            EvidenceState::measured(measurement),
        )
        .map_err(StateBindingError::Evidence)
    }

    fn measurement(
        &self,
        decision: &BoundDecision,
    ) -> Result<MetricMeasurement, StateBindingError> {
        self.validate_decision(decision)?;
        let actor = decision.actor();
        let latch = LatchBinding::new(
            latch_kind_name(self.entry.kind()),
            LatchLimits {
                base: decision.base_limit(),
                configured_ratchet: decision.configured_elastic_limit(),
                variation_percent: decision.variation_percent(),
                ratchet: decision.elastic_limit(),
                active: decision.active_limit(),
                unlimited: decision.unlimited(),
            },
            LatchOutcome {
                over_limit: decision.over_limit(),
                over_base: decision.over_base(),
                ratchet_breach: decision.elastic_breach(),
            },
            transition_name(decision.latch_transition()),
            actor.map(|actor| {
                (
                    actor_source_name(actor.source()).to_owned(),
                    actor.value().to_owned(),
                )
            }),
        )
        .map_err(StateBindingError::Evidence)?;
        Ok(MetricMeasurement::new(self.value, self.parser.clone())
            .with_rule(self.rule.binding().clone())
            .with_latch(latch))
    }

    fn validate_decision(&self, decision: &BoundDecision) -> Result<(), StateBindingError> {
        let matches = decision.rule() == rule_identity(&self.rule)
            && decision.metric() == self.metric.as_str()
            && decision.measured() == self.value
            && decision.base_limit() == self.rule.base_limit()
            && decision.configured_elastic_limit() == self.rule.ratchet_limit()
            && decision.variation_percent() == self.variation_percent
            && decision.unlimited() == self.rule.unlimited();
        if matches {
            Ok(())
        } else {
            Err(StateBindingError::Decision(
                "bound decision disagrees with its metric, value, or resolved rule".to_owned(),
            ))
        }
    }
}

fn state_key(key: &MetricKey) -> Result<StateKey, StateBindingError> {
    match key {
        MetricKey::Path(path) => StateKey::path(path).map_err(StateBindingError::Key),
        MetricKey::Function(function) => {
            StateKey::function(function.path(), function.function()).map_err(StateBindingError::Key)
        }
        MetricKey::Arm => Err(StateBindingError::KeyShape(
            "a bounded measurement requires a concrete path or function key".to_owned(),
        )),
    }
}

fn rule_identity(rule: &ResolvedMetricRule) -> String {
    rule.binding().rule_id().map_or_else(
        || format!("global:{}", rule.binding().field()),
        str::to_owned,
    )
}

const fn latch_kind(metric: MetricId) -> Option<LatchKind> {
    match metric {
        MetricId::FileLines => Some(LatchKind::FileLoc),
        MetricId::FileBytes => Some(LatchKind::FileBytes),
        MetricId::RoutineCcn => Some(LatchKind::ComplexityCcn),
        MetricId::RoutineLength => Some(LatchKind::ComplexityLength),
        MetricId::RepositoryDocumentCharacters => Some(LatchKind::RepoDocChars),
        MetricId::MagicNumberRegressions
        | MetricId::MarkdownLinkCaseMismatches
        | MetricId::RustDocMarkdownTargetAbsences => None,
    }
}

const fn latch_kind_name(kind: LatchKind) -> &'static str {
    match kind {
        LatchKind::FileLoc => "file_loc",
        LatchKind::FileBytes => "file_bytes",
        LatchKind::ComplexityCcn => "complexity_ccn",
        LatchKind::ComplexityLength => "complexity_length",
        LatchKind::RepoDocChars => "repo_doc_chars",
    }
}

const fn transition_name(transition: LatchTransition) -> &'static str {
    match transition {
        LatchTransition::Clear => "clear",
        LatchTransition::Latched => "latched",
        LatchTransition::Held => "held",
        LatchTransition::Healed => "healed",
    }
}

const fn actor_source_name(source: ActorSource) -> &'static str {
    match source {
        ActorSource::PublishedTaskSession => "published-task-session",
        ActorSource::PublishedCommit => "published-commit",
        ActorSource::ActiveActor => "active-actor",
        ActorSource::CandidateBlob => "candidate-blob",
        ActorSource::CandidatePath => "candidate-path",
    }
}

/// Why a parsed structural value could not cross current bounded state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StateBindingError {
    UnboundedMetric(MetricId),
    RuleMetric {
        expected: MetricId,
        observed: MetricId,
    },
    KeyShape(String),
    Parser(String),
    Key(KeyError),
    Bound(BoundError),
    Decision(String),
    Evidence(EvidenceError),
}

impl fmt::Display for StateBindingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnboundedMetric(metric) => {
                write!(formatter, "{} has no durable bound", metric.as_str())
            }
            Self::RuleMetric { expected, observed } => write!(
                formatter,
                "{} cannot consume the resolved {} rule",
                expected.as_str(),
                observed.as_str()
            ),
            Self::KeyShape(reason) | Self::Parser(reason) | Self::Decision(reason) => {
                formatter.write_str(reason)
            }
            Self::Key(error) => write!(formatter, "state key: {error}"),
            Self::Bound(error) => write!(formatter, "bounded state: {error}"),
            Self::Evidence(error) => write!(formatter, "metric evidence: {error}"),
        }
    }
}

impl Error for StateBindingError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Key(error) => Some(error),
            Self::Bound(error) => Some(error),
            Self::Evidence(error) => Some(error),
            _ => None,
        }
    }
}
