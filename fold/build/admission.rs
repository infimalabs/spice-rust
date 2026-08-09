use std::collections::BTreeSet;
use std::fmt;

/// One generated operational fold input.
#[derive(Clone, Copy, Debug)]
pub enum Source {
    Facts {
        plane: &'static str,
        pattern: &'static str,
    },
}

/// One fold row supplied by the declarative registry.
#[derive(Clone, Copy, Debug)]
pub struct FoldRegistration {
    pub variant: &'static str,
    pub name: &'static str,
    pub answer: &'static str,
    pub sources: &'static [Source],
    pub beyond: &'static str,
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

/// The rows pinned independently from the generated registry.
///
/// A new row cannot silently join both the enum and its completeness oracle.
/// It also has to join the exhaustive typed replay match in `journal_command`.
const EXPECTED_ROWS: &[(&str, &str)] = &[("TaskEffort", "task::effort")];

/// Every source family the operational task-effort reducer consumes.
pub const FACT_PLANES: &[(&str, &str)] = &[
    ("Task", "task::*"),
    ("Claim", "claim::*"),
    ("Lane", "lane::*"),
    ("Lineage", "lineage::*"),
    ("Journal", "journal::*"),
];

/// Validate the generated wire/fold join before the fold crate compiles.
pub fn validate(facts: &[wire::FactKind], folds: &[FoldRegistration]) -> Result<(), Refusal> {
    validate_rows(folds)?;
    validate_sources(facts, folds)
}

fn validate_rows(folds: &[FoldRegistration]) -> Result<(), Refusal> {
    let mut rows = BTreeSet::new();
    for fold in folds {
        let row = (fold.variant, fold.name);
        if !rows.insert(row) {
            return Err(Refusal::new(format!(
                "fold registry row {} is declared more than once",
                fold.name
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
        if fold.sources.is_empty() {
            return Err(Refusal::new(format!(
                "fold {} declares no sources",
                fold.name
            )));
        }
        if fold.beyond != "Answer" {
            return Err(Refusal::new(format!(
                "fold {} has unknown beyond policy {}",
                fold.name, fold.beyond
            )));
        }
    }
    Ok(())
}

fn validate_sources(facts: &[wire::FactKind], folds: &[FoldRegistration]) -> Result<(), Refusal> {
    for fold in folds {
        for source in fold.sources {
            let Source::Facts { plane, pattern } = source;
            let declared = FACT_PLANES.contains(&(*plane, *pattern));
            let supplied = facts
                .iter()
                .any(|fact| pattern_matches(pattern, fact.as_str()));
            if !declared || !supplied {
                return Err(Refusal::new(format!(
                    "fold {} names unknown fact family {plane} => {pattern}",
                    fold.name
                )));
            }
        }
    }
    Ok(())
}

/// True when a wire spelling falls under a declared family glob.
pub fn pattern_matches(pattern: &str, kind: &str) -> bool {
    pattern
        .strip_suffix('*')
        .is_some_and(|prefix| kind.starts_with(prefix))
}
