//! Retired public vocabulary and its current successors.

/// One retired concept, every spelling the gate refuses, and its successor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetiredName {
    pub retired: &'static str,
    pub successor: &'static str,
    pub spellings: &'static [&'static str],
}

impl RetiredName {
    const fn new(
        retired: &'static str,
        successor: &'static str,
        spellings: &'static [&'static str],
    ) -> Self {
        Self {
            retired,
            successor,
            spellings,
        }
    }
}

/// The complete port-owned vocabulary retirement ledger.
///
/// This table deliberately lives in the Rust tree. It must not be generated
/// from or compared with preliminary design material at build or test time.
pub const RETIRED_NAMES: &[RetiredName] = &[
    RetiredName::new("serve", "up, canvas", &["serve"]),
    RetiredName::new("study, studies", "gate", &["study", "studies"]),
    RetiredName::new(
        "flex, sticky, jitter",
        "ratchet",
        &["flex", "sticky", "jitter"],
    ),
    RetiredName::new("ingest", "doc", &["ingest"]),
    RetiredName::new("taskdoc", "doc, dialect", &["taskdoc", "task doc"]),
    RetiredName::new("incepted", "born", &["incepted"]),
    RetiredName::new("inbox item", "directive", &["inbox item"]),
    RetiredName::new("conscience", "taste", &["conscience"]),
    RetiredName::new("maxim", "taste", &["maxim"]),
    RetiredName::new("projection", "fold", &["projection"]),
    RetiredName::new(
        "watch UI, observer mode",
        "watch",
        &["watch UI", "observer mode"],
    ),
    RetiredName::new("unclaim", "free", &["unclaim"]),
    RetiredName::new("lifetime", "life", &["lifetime"]),
    RetiredName::new("driver", "vendor", &["driver"]),
    RetiredName::new("team", "lineage", &["team"]),
    RetiredName::new(
        "supervisor process",
        "occupant, lane",
        &["supervisor process"],
    ),
    RetiredName::new("constitution", "gate", &["constitution"]),
    RetiredName::new(
        "bypassPermissions",
        "consent::floored",
        &["bypassPermissions"],
    ),
    RetiredName::new("replica", "head", &["replica"]),
    RetiredName::new("briefing", "recall", &["briefing"]),
    RetiredName::new(
        "source (attribution lens)",
        "actor",
        &["source attribution", "attribution source"],
    ),
    RetiredName::new(
        "taskchampion",
        "deleted",
        &["taskchampion", "task champion"],
    ),
    RetiredName::new("ceiling", "floor, baseline", &["ceiling"]),
    RetiredName::new("dev", "gate, install", &["dev"]),
    RetiredName::new("deinit", "spice install --unapply", &["deinit"]),
];

/// One retired row matched in candidate source text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetiredMatch {
    pub retired: &'static str,
    pub successor: &'static str,
}

/// Find every retired concept named by an identifier, key, or spelling.
#[must_use]
pub fn retired_matches(candidate: &str) -> Vec<RetiredMatch> {
    let candidate_words = tokenize(candidate);
    RETIRED_NAMES
        .iter()
        .filter(|entry| {
            entry.spellings.iter().any(|spelling| {
                let retired_words = tokenize(spelling);
                contains_words(&candidate_words, &retired_words)
            })
        })
        .map(|entry| RetiredMatch {
            retired: entry.retired,
            successor: entry.successor,
        })
        .collect()
}

fn tokenize(value: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut previous_was_lowercase = false;

    for character in value.chars() {
        if !character.is_alphanumeric() {
            finish_word(&mut words, &mut current);
            previous_was_lowercase = false;
            continue;
        }
        if character.is_uppercase() && previous_was_lowercase {
            finish_word(&mut words, &mut current);
        }
        current.extend(character.to_lowercase());
        previous_was_lowercase = character.is_lowercase();
    }
    finish_word(&mut words, &mut current);
    words
}

fn finish_word(words: &mut Vec<String>, current: &mut String) {
    if !current.is_empty() {
        words.push(std::mem::take(current));
    }
}

fn contains_words(candidate: &[String], retired: &[String]) -> bool {
    !retired.is_empty()
        && candidate
            .windows(retired.len())
            .any(|window| window == retired)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    const SOURCE_ROW_COUNT: usize = 25;

    #[test]
    fn every_source_row_names_a_successor_and_unique_spellings() {
        assert_eq!(RETIRED_NAMES.len(), SOURCE_ROW_COUNT);
        let mut spellings = BTreeSet::new();

        for entry in RETIRED_NAMES {
            assert!(
                !entry.successor.is_empty(),
                "{} has no successor",
                entry.retired
            );
            assert!(
                !entry.spellings.is_empty(),
                "{} has no spelling",
                entry.retired
            );
            for spelling in entry.spellings {
                assert!(
                    spellings.insert(tokenize(spelling)),
                    "duplicate retired spelling {spelling}"
                );
            }
        }
    }

    #[test]
    fn identifiers_are_split_across_case_and_punctuation() {
        assert_eq!(tokenize("bypassPermissions"), ["bypass", "permissions"]);
        assert_eq!(tokenize("Task_Champion"), ["task", "champion"]);
    }
}
