use std::collections::BTreeSet;
use std::fmt;

/// One fact or head family an answer says it contains.
#[derive(Clone, Copy, Debug)]
pub enum AnswerContent {
    Fact(wire::FactKind),
    Facts {
        plane: &'static str,
        pattern: &'static str,
    },
    Head {
        variant: &'static str,
        name: &'static str,
    },
}

/// One generated fold input.
#[derive(Clone, Copy, Debug)]
pub enum Source {
    Fact(wire::FactKind),
    Facts {
        plane: &'static str,
        pattern: &'static str,
    },
    Fold {
        variant: &'static str,
        name: &'static str,
    },
    Head {
        variant: &'static str,
        name: &'static str,
    },
    Ring {
        variant: &'static str,
        name: &'static str,
    },
    Config {
        variant: &'static str,
        name: &'static str,
    },
    Tape {
        variant: &'static str,
        name: &'static str,
    },
    CommandResults(&'static str),
}

/// One fold row supplied by the declarative registry.
#[derive(Clone, Copy, Debug)]
pub struct FoldRegistration {
    pub variant: &'static str,
    pub name: &'static str,
    pub answer: &'static str,
    pub content: &'static [AnswerContent],
    pub sources: &'static [Source],
    pub beyond: &'static str,
}

/// One bounded ring row supplied by the declarative registry.
#[derive(Clone, Copy, Debug)]
pub struct RingRegistration {
    pub variant: &'static str,
    pub name: &'static str,
    pub recovery: &'static str,
}

/// A build-time refusal of an incomplete generated registry.
#[derive(Debug, Eq, PartialEq)]
pub struct Refusal(String);

impl Refusal {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The rows pinned from the fold registry contract.
///
/// This independent list is deliberate: generating the inventory and its
/// completeness oracle from the same rows would let a deleted row disappear
/// from both sides without a build failure.
const EXPECTED_ROWS: &[(&str, &str)] = &[
    ("SteerInbox", "steer::inbox"),
    ("ConsentQueue", "consent::queue"),
    ("LaneCanary", "lane::canary"),
    ("TasteActive", "taste::active"),
    ("CanvasVerdicts", "canvas::verdicts"),
    ("CanvasMagnitude", "canvas::magnitude"),
    ("CanvasFlow", "canvas::flow"),
    ("CanvasTopology", "canvas::topology"),
    ("CanvasChronology", "canvas::chronology"),
    ("ReleaseHistory", "release::history"),
    ("BusRefusals", "bus::refusals"),
];

/// Each declared plane and the wire spelling it admits facts under.
///
/// Public so the plane-membership law in `fold/tests/admission.rs` can hold
/// this table against `FactPlane::contains`, which is the same membership
/// written a second time by hand.
pub const FACT_PLANES: &[(&str, &str)] = &[
    ("Task", "task::*"),
    ("Claim", "claim::*"),
    ("Document", "doc::*"),
    ("Steering", "steer::*"),
    ("Consent", "consent::*"),
    ("Lane", "lane::*"),
    ("Taste", "taste::*"),
    ("Canary", "canary::*"),
    ("Vendor", "vendor::*"),
    ("Trust", "trust::*"),
    ("Commit", "commit::*"),
    ("Gate", "gate::*"),
    ("Journal", "journal::*"),
    ("Resident", "up::*"),
    ("Proof", "proof::*"),
    ("Release", "release::*"),
];

const ADMITTED_HEADS: &[(&str, &str)] = &[
    ("Task", "task"),
    ("Workflow", "workflow"),
    ("Claim", "claim"),
    ("Lease", "lease"),
    ("Witness", "witness"),
    ("Finding", "finding"),
    ("Residency", "residency"),
];

const ADMITTED_CONFIGS: &[(&str, &str)] = &[("Consent", "consent"), ("Taste", "taste")];
const ADMITTED_TAPES: &[(&str, &str)] = &[("DeclaredExtents", "declared-extents")];

/// Validate the generated wire/fold join before the fold crate compiles.
pub fn validate(
    facts: &[wire::FactKind],
    folds: &[FoldRegistration],
    rings: &[RingRegistration],
) -> Result<(), Refusal> {
    validate_rows(folds, rings)?;
    validate_sources(facts, folds, rings)?;
    validate_consumers(facts, folds)?;
    validate_answers(facts, folds)?;
    Ok(())
}

fn validate_rows(folds: &[FoldRegistration], rings: &[RingRegistration]) -> Result<(), Refusal> {
    let mut rows = BTreeSet::new();
    for row in folds
        .iter()
        .map(|fold| (fold.variant, fold.name))
        .chain(rings.iter().map(|ring| (ring.variant, ring.name)))
    {
        if !rows.insert(row) {
            return Err(Refusal::new(format!(
                "fold registry row {} is declared more than once",
                row.1
            )));
        }
    }

    for expected in EXPECTED_ROWS {
        if !rows.contains(expected) {
            return Err(Refusal::new(format!(
                "fold registry is missing contract row {}",
                expected.1
            )));
        }
    }
    for row in rows {
        if !EXPECTED_ROWS.contains(&row) {
            return Err(Refusal::new(format!(
                "fold registry contains unknown contract row {}",
                row.1
            )));
        }
    }

    for fold in folds {
        if fold.answer.is_empty() {
            return Err(Refusal::new(format!(
                "fold {} has no declared answer",
                fold.name
            )));
        }
        if fold.content.is_empty() {
            return Err(Refusal::new(format!(
                "fold {} answer declares no content",
                fold.name
            )));
        }
        if !matches!(fold.beyond, "Wait" | "Answer") {
            return Err(Refusal::new(format!(
                "fold {} has unknown beyond policy {}",
                fold.name, fold.beyond
            )));
        }
    }
    for ring in rings {
        if ring.recovery != "None" {
            return Err(Refusal::new(format!(
                "bounded ring {} must declare recovery None",
                ring.name
            )));
        }
    }
    Ok(())
}

fn validate_sources(
    facts: &[wire::FactKind],
    folds: &[FoldRegistration],
    rings: &[RingRegistration],
) -> Result<(), Refusal> {
    for (position, fold) in folds.iter().enumerate() {
        for source in fold.sources {
            match source {
                Source::Fact(_) => {}
                Source::Facts { plane, pattern } => {
                    require_fact_pattern(facts, fold.name, plane, pattern)?;
                }
                Source::Head { variant, name } => {
                    require_pair(fold.name, "head", (*variant, *name), ADMITTED_HEADS)?;
                }
                Source::Fold { variant, name } => {
                    let source_position = folds
                        .iter()
                        .position(|candidate| {
                            candidate.variant == *variant && candidate.name == *name
                        })
                        .ok_or_else(|| {
                            Refusal::new(format!(
                                "fold {} names unknown source fold {name}",
                                fold.name
                            ))
                        })?;
                    if source_position >= position {
                        return Err(Refusal::new(format!(
                            "fold {} must follow source fold {name}",
                            fold.name
                        )));
                    }
                }
                Source::Ring { variant, name } => {
                    if !rings
                        .iter()
                        .any(|ring| ring.variant == *variant && ring.name == *name)
                    {
                        return Err(Refusal::new(format!(
                            "fold {} names unknown source ring {name}",
                            fold.name
                        )));
                    }
                }
                Source::Config { variant, name } => {
                    require_pair(fold.name, "config", (*variant, *name), ADMITTED_CONFIGS)?;
                }
                Source::Tape { variant, name } => {
                    require_pair(fold.name, "tape", (*variant, *name), ADMITTED_TAPES)?;
                }
                Source::CommandResults(name) => {
                    if *name != "stored-command-results" {
                        return Err(Refusal::new(format!(
                            "fold {} names unknown command-result source {name}",
                            fold.name
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}

fn validate_consumers(facts: &[wire::FactKind], folds: &[FoldRegistration]) -> Result<(), Refusal> {
    for &fact in facts {
        let consumed = folds.iter().any(|fold| {
            fold.sources
                .iter()
                .any(|source| source_supplies_fact(*source, fact))
        });
        if !consumed {
            return Err(Refusal::new(format!(
                "fact kind {} from producer {} has no consuming fold",
                fact.as_str(),
                fact.producer(),
            )));
        }
    }
    Ok(())
}

fn validate_answers(facts: &[wire::FactKind], folds: &[FoldRegistration]) -> Result<(), Refusal> {
    for fold in folds {
        for content in fold.content {
            match content {
                AnswerContent::Fact(_) => {
                    require_answer_source(fold, *content)?;
                }
                AnswerContent::Facts { plane, pattern } => {
                    let declared = FACT_PLANES.contains(&(*plane, *pattern));
                    let supplied = facts
                        .iter()
                        .any(|fact| pattern_matches(pattern, fact.as_str()));
                    if !declared || !supplied {
                        return Err(Refusal::new(format!(
                            "fold {} answer names fact content {pattern}, but no admitted kind can supply it",
                            fold.name
                        )));
                    }
                    require_answer_source(fold, *content)?;
                }
                AnswerContent::Head { variant, name } => {
                    if !ADMITTED_HEADS.contains(&(*variant, *name)) {
                        return Err(Refusal::new(format!(
                            "fold {} answer names head content {name}, but no admitted head can supply it",
                            fold.name
                        )));
                    }
                    require_answer_source(fold, *content)?;
                }
            }
        }
    }
    Ok(())
}

fn require_fact_pattern(
    facts: &[wire::FactKind],
    fold: &str,
    plane: &str,
    pattern: &str,
) -> Result<(), Refusal> {
    let declared = FACT_PLANES.contains(&(plane, pattern));
    let supplied = facts
        .iter()
        .any(|fact| pattern_matches(pattern, fact.as_str()));
    if declared && supplied {
        Ok(())
    } else {
        Err(Refusal::new(format!(
            "fold {fold} names unknown fact family {plane} => {pattern}"
        )))
    }
}

fn require_pair(
    fold: &str,
    class: &str,
    pair: (&str, &str),
    admitted: &[(&str, &str)],
) -> Result<(), Refusal> {
    if admitted.contains(&pair) {
        Ok(())
    } else {
        Err(Refusal::new(format!(
            "fold {fold} names unknown {class} source {}",
            pair.1
        )))
    }
}

fn require_answer_source(fold: &FoldRegistration, content: AnswerContent) -> Result<(), Refusal> {
    let supplied = match content {
        AnswerContent::Fact(kind) => fold
            .sources
            .iter()
            .any(|source| source_supplies_fact(*source, kind)),
        AnswerContent::Facts { pattern, .. } => fold.sources.iter().any(|source| match source {
            Source::Fact(kind) => pattern_matches(pattern, kind.as_str()),
            Source::Facts {
                pattern: source_pattern,
                ..
            } => patterns_overlap(pattern, source_pattern),
            _ => false,
        }),
        AnswerContent::Head { name, .. } => fold
            .sources
            .iter()
            .any(|source| matches!(source, Source::Head { name: source, .. } if source == &name)),
    };

    if supplied {
        Ok(())
    } else {
        Err(Refusal::new(format!(
            "fold {} answer content {} is not supplied by its admitted sources",
            fold.name,
            content_name(content)
        )))
    }
}

fn source_supplies_fact(source: Source, kind: wire::FactKind) -> bool {
    match source {
        Source::Fact(source) => source == kind,
        Source::Facts { pattern, .. } => pattern_matches(pattern, kind.as_str()),
        Source::Fold { .. }
        | Source::Head { .. }
        | Source::Ring { .. }
        | Source::Config { .. }
        | Source::Tape { .. }
        | Source::CommandResults(_) => false,
    }
}

fn patterns_overlap(left: &str, right: &str) -> bool {
    left.strip_suffix('*') == right.strip_suffix('*')
}

/// True when a wire spelling falls under a declared family glob.
pub fn pattern_matches(pattern: &str, kind: &str) -> bool {
    pattern
        .strip_suffix('*')
        .is_some_and(|prefix| kind.starts_with(prefix))
}

fn content_name(content: AnswerContent) -> &'static str {
    match content {
        AnswerContent::Fact(kind) => kind.as_str(),
        AnswerContent::Facts { pattern, .. } => pattern,
        AnswerContent::Head { name, .. } => name,
    }
}
