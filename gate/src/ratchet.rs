//! Ceilings that fall on their own and rise only by an operator's hand.
//!
//! A ratchet is a promise about direction, not about any particular number.
//! Everything here is arranged so the promise cannot be broken quietly: a
//! measurement above its ceiling is a regression and never a new ceiling,
//! tightening lowers or holds but has no path that raises, and a ceiling whose
//! metric has stopped being declared stays in the file rather than evaporating,
//! because a ceiling that disappears has effectively risen to infinity.

use std::collections::BTreeMap;
use std::fmt;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::measure::Measurement;

/// Where one metric stands against its ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "movement")]
pub enum Movement {
    /// The measurement sits exactly at its ceiling.
    Held {
        /// The ceiling, which is also the measurement.
        ceiling: u64,
    },
    /// The measurement is below its ceiling, so the ceiling may fall to it.
    Shrank {
        /// The ceiling before this run.
        ceiling: u64,
        /// What was measured.
        measured: u64,
    },
    /// The measurement is above its ceiling.
    Regressed {
        /// The ceiling this run passed.
        ceiling: u64,
        /// What was measured.
        measured: u64,
    },
    /// The metric is declared and measured but has no ceiling yet.
    Unbaselined {
        /// What was measured.
        measured: u64,
    },
    /// A ceiling survives for a metric the policy no longer declares.
    Retired {
        /// The ceiling nothing measures any more.
        ceiling: u64,
    },
}

impl Movement {
    /// Whether this movement lets a run pass.
    ///
    /// A retired ceiling fails: it is the one shape in which a ratchet can rise
    /// without anyone writing a larger number, so it has to be settled by the
    /// same operator commit any other rise would need.
    #[must_use]
    pub const fn passes(self) -> bool {
        match self {
            Self::Held { .. } | Self::Shrank { .. } | Self::Unbaselined { .. } => true,
            Self::Regressed { .. } | Self::Retired { .. } => false,
        }
    }
}

impl fmt::Display for Movement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Held { ceiling } => write!(formatter, "held at {ceiling}"),
            Self::Shrank { ceiling, measured } => {
                write!(formatter, "shrank from {ceiling} to {measured}")
            }
            Self::Regressed { ceiling, measured } => {
                write!(formatter, "regressed past {ceiling} to {measured}")
            }
            Self::Unbaselined { measured } => write!(formatter, "unbaselined at {measured}"),
            Self::Retired { ceiling } => {
                write!(formatter, "ceiling {ceiling} has no declared metric")
            }
        }
    }
}

/// One metric's standing after a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Finding {
    /// The metric this is about.
    pub metric: String,
    /// Where it stands.
    pub movement: Movement,
}

/// The committed ceilings a run is judged against.
///
/// The file is one `metric value` line per ceiling, sorted and unique, because
/// a ratchet file is read by operators in diffs. Two spellings of the same
/// ceiling set would make a diff say something happened when nothing did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Baseline {
    ceilings: BTreeMap<String, u64>,
}

impl Baseline {
    /// Read a committed baseline.
    ///
    /// # Errors
    ///
    /// Returns the first line that is not an admitted spelling, carrying that
    /// line as evidence. A malformed baseline is never partially accepted: a
    /// baseline read as half its ceilings would let the missing half regress
    /// unnoticed.
    pub fn parse(text: &str) -> Result<Self, BaselineError> {
        let mut ceilings: BTreeMap<String, u64> = BTreeMap::new();
        let mut previous: Option<&str> = None;

        for (offset, line) in text.lines().enumerate() {
            let number = offset + 1;
            let refuse = |malady| Err(BaselineError::new(number, line, malady));

            let Some((metric, ceiling)) = line.split_once(' ') else {
                return refuse(Malady::NotTwoFields);
            };
            if metric.is_empty() || ceiling.is_empty() {
                return refuse(Malady::NotTwoFields);
            }
            let Ok(ceiling) = ceiling.parse::<u64>() else {
                return refuse(Malady::NotACount);
            };
            if previous.is_some_and(|earlier| earlier == metric) {
                return refuse(Malady::Duplicate);
            }
            if previous.is_some_and(|earlier| earlier > metric) {
                return refuse(Malady::Unsorted);
            }

            previous = Some(metric);
            ceilings.insert(metric.to_owned(), ceiling);
        }

        Ok(Self { ceilings })
    }

    /// The canonical rendering, ready to commit.
    #[must_use]
    pub fn render(&self) -> String {
        let mut rendered = String::new();
        for (metric, ceiling) in &self.ceilings {
            let _ = writeln!(rendered, "{metric} {ceiling}");
        }
        rendered
    }

    /// One metric's ceiling, absent when it has none.
    #[must_use]
    pub fn ceiling(&self, metric: &str) -> Option<u64> {
        self.ceilings.get(metric).copied()
    }

    /// Every ceiling, in metric order.
    pub fn ceilings(&self) -> impl Iterator<Item = (&str, u64)> {
        self.ceilings
            .iter()
            .map(|(metric, ceiling)| (metric.as_str(), *ceiling))
    }

    /// Where every metric stands, in metric order.
    ///
    /// Ceilings without a measurement are reported too, so a metric that stops
    /// being declared cannot take its ceiling with it in silence.
    #[must_use]
    pub fn compare(&self, measurements: &[Measurement]) -> Vec<Finding> {
        let mut findings: BTreeMap<&str, Movement> = self
            .ceilings
            .iter()
            .map(|(metric, ceiling)| (metric.as_str(), Movement::Retired { ceiling: *ceiling }))
            .collect();

        for measurement in measurements {
            let measured = measurement.value();
            let movement = match self.ceiling(measurement.metric()) {
                None => Movement::Unbaselined { measured },
                Some(ceiling) if measured == ceiling => Movement::Held { ceiling },
                Some(ceiling) if measured < ceiling => Movement::Shrank { ceiling, measured },
                Some(ceiling) => Movement::Regressed { ceiling, measured },
            };
            findings.insert(measurement.metric(), movement);
        }

        findings
            .into_iter()
            .map(|(metric, movement)| Finding {
                metric: metric.to_owned(),
                movement,
            })
            .collect()
    }

    /// The baseline this run earns.
    ///
    /// Every path through this function either lowers a ceiling or leaves it
    /// alone. There is deliberately no branch that writes a larger number than
    /// the one already committed, which is what makes "shrink-only" a property
    /// of the code rather than of the caller's discipline.
    #[must_use]
    pub fn tighten(&self, measurements: &[Measurement]) -> Self {
        let mut ceilings = self.ceilings.clone();
        for measurement in measurements {
            let earned = match ceilings.get(measurement.metric()) {
                Some(ceiling) => (*ceiling).min(measurement.value()),
                None => measurement.value(),
            };
            ceilings.insert(measurement.metric().to_owned(), earned);
        }
        Self { ceilings }
    }
}

/// What is wrong with one baseline line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Malady {
    /// The line is not exactly a metric and a count separated by one space.
    NotTwoFields,
    /// The second field is not a non-negative count.
    NotACount,
    /// The metric repeats the line before it.
    Duplicate,
    /// The metric sorts before the line above it.
    Unsorted,
}

impl fmt::Display for Malady {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotTwoFields => {
                formatter.write_str("is not one metric and one count separated by a single space")
            }
            Self::NotACount => formatter.write_str("does not name a non-negative count"),
            Self::Duplicate => formatter.write_str("repeats the metric above it"),
            Self::Unsorted => formatter.write_str("sorts before the metric above it"),
        }
    }
}

/// A baseline line that is not one of the admitted spellings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselineError {
    number: usize,
    line: String,
    malady: Malady,
}

impl BaselineError {
    fn new(number: usize, line: &str, malady: Malady) -> Self {
        Self {
            number,
            line: line.to_owned(),
            malady,
        }
    }

    /// Which line was refused, counting from one.
    #[must_use]
    pub const fn number(&self) -> usize {
        self.number
    }

    /// The exact line that was refused.
    #[must_use]
    pub fn line(&self) -> &str {
        &self.line
    }

    /// Why it was refused.
    #[must_use]
    pub const fn malady(&self) -> Malady {
        self.malady
    }
}

impl fmt::Display for BaselineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "baseline line {} {:?} {}",
            self.number, self.line, self.malady
        )
    }
}

impl std::error::Error for BaselineError {}
