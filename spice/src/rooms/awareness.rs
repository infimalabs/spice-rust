//! Deterministic word matching between an open Design and a lane's claimed
//! task.
//!
//! A statement's terms are its lowercase alphabetic words: text splits on
//! every character that is not a letter or digit, a run that holds any digit
//! is inert (as in taste bags), words shorter than [`MIN_TERM_CHARS`] letters,
//! words whose lowercase form is not wholly lowercase letters, and common
//! function words are dropped, and one trailing `s` after a longer
//! stem is folded away so `designs` and `design` agree. A Design's terms come
//! from its title and description; a task's from its title, description, and
//! acceptance. The task matches the Design when they share at least
//! [`MIN_SHARED_TERMS`] distinct terms that are not [common](CommonTerms)
//! across the board's open statements, and at least one of them is a term of
//! the Design's title.

use std::collections::{BTreeMap, BTreeSet};

pub use wire::rooms::COMPLETION_VALIDATION_BYTES;
use wire::rooms::DESIGN_NOTICE_TERMS;

/// Fewest letters a word needs to count as a term.
pub const MIN_TERM_CHARS: usize = 4;
/// Fewest distinct shared terms that match a task to a Design.
pub const MIN_SHARED_TERMS: usize = 3;
/// Most characters of a Design's one-line pitch.
pub const PITCH_CHARS: usize = 160;
/// Fewest open statements a board needs before any term can be common.
pub const COMMON_TERM_MIN_STATEMENTS: usize = 8;
/// A term in more than this fraction, numerator over denominator, of the
/// board's open statements is common.
pub const COMMON_TERM_SHARE: (usize, usize) = (1, 4);
/// Most distinct terms retained in one completion matching profile.
pub const COMPLETION_PROFILE_TERMS: usize = 256;

/// Common words that say nothing about what a statement concerns.
const STOP: &[&str] = &[
    "about", "after", "also", "been", "before", "being", "both", "cannot", "could", "does", "each",
    "every", "from", "have", "into", "just", "like", "many", "more", "most", "much", "must",
    "never", "none", "only", "other", "over", "same", "should", "some", "such", "than", "that",
    "their", "them", "then", "there", "these", "they", "this", "those", "through", "under",
    "until", "very", "what", "when", "where", "whether", "which", "while", "will", "with",
    "within", "without", "would", "your",
];

/// The terms of one Design's statement, its title's terms kept apart.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DesignTerms {
    title: BTreeSet<String>,
    all: BTreeSet<String>,
}

impl DesignTerms {
    /// Terms of a Design titled `title` described by `description`.
    #[must_use]
    pub fn new(title: &str, description: Option<&str>) -> Self {
        let title = terms([title]);
        let mut all = title.clone();
        all.extend(terms(description));
        Self { title, all }
    }

    /// The shared terms that are not `common`, ascending and at most
    /// [`DESIGN_NOTICE_TERMS`], when `task` matches this Design; `None`
    /// otherwise.
    #[must_use]
    pub fn matched(&self, task: &BTreeSet<String>, common: &CommonTerms) -> Option<Vec<String>> {
        let shared = self
            .all
            .intersection(task)
            .filter(|term| !common.0.contains(*term))
            .collect::<Vec<_>>();
        let titled = shared.iter().any(|term| self.title.contains(*term));
        (shared.len() >= MIN_SHARED_TERMS && titled).then(|| {
            shared
                .into_iter()
                .take(DESIGN_NOTICE_TERMS)
                .cloned()
                .collect()
        })
    }
}

/// Terms so widespread across the board's open statements that sharing them
/// says nothing about what a task and a Design have in common.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommonTerms(BTreeSet<String>);

impl CommonTerms {
    /// The terms found in more than [`COMMON_TERM_SHARE`] of `statements`,
    /// each one open statement's terms. A board with fewer than
    /// [`COMMON_TERM_MIN_STATEMENTS`] open statements has none. Memory is
    /// bounded by the statements' distinct terms.
    #[must_use]
    pub fn of(statements: impl IntoIterator<Item = BTreeSet<String>>) -> Self {
        let mut counts = BTreeMap::<String, usize>::new();
        let mut total = 0_usize;
        for statement in statements {
            total += 1;
            for term in statement {
                *counts.entry(term).or_default() += 1;
            }
        }
        if total < COMMON_TERM_MIN_STATEMENTS {
            return Self::default();
        }
        let (numerator, denominator) = COMMON_TERM_SHARE;
        Self(
            counts
                .into_iter()
                .filter(|(_, count)| count * denominator > total * numerator)
                .map(|(term, _)| term)
                .collect(),
        )
    }
}

/// Terms of a task statement: its title, description, and acceptance.
#[must_use]
pub fn task_terms(
    title: &str,
    description: Option<&str>,
    acceptance: &[String],
) -> BTreeSet<String> {
    terms(
        std::iter::once(title)
            .chain(description)
            .chain(acceptance.iter().map(String::as_str)),
    )
}

/// Terms of a completed task's statement plus a bounded prefix of its
/// validation, capped again after deduplication.
#[must_use]
pub fn completion_terms(
    title: &str,
    description: Option<&str>,
    acceptance: &[String],
    validation: &str,
) -> BTreeSet<String> {
    let mut validation_end = validation.len().min(COMPLETION_VALIDATION_BYTES);
    while !validation.is_char_boundary(validation_end) {
        validation_end -= 1;
    }
    terms_capped(
        std::iter::once(title)
            .chain(description)
            .chain(acceptance.iter().map(String::as_str))
            .chain(std::iter::once(&validation[..validation_end])),
        COMPLETION_PROFILE_TERMS,
    )
}

/// The distinct terms of `texts`.
fn terms<'a>(texts: impl IntoIterator<Item = &'a str>) -> BTreeSet<String> {
    terms_capped(texts, usize::MAX)
}

fn terms_capped<'a>(texts: impl IntoIterator<Item = &'a str>, maximum: usize) -> BTreeSet<String> {
    let mut terms = BTreeSet::new();
    for text in texts {
        for run in text.split(|c: char| !c.is_alphanumeric()) {
            if terms.len() == maximum {
                return terms;
            }
            if run.chars().any(char::is_numeric) {
                continue;
            }
            let word = run.to_lowercase();
            let word = match word.strip_suffix('s') {
                Some(stem) if stem.chars().count() >= MIN_TERM_CHARS && !stem.ends_with('s') => {
                    stem.to_owned()
                }
                _ => word,
            };
            if word.chars().count() >= MIN_TERM_CHARS
                && word.chars().all(|c| c.is_alphabetic() && !c.is_uppercase())
                && !STOP.contains(&word.as_str())
            {
                terms.insert(word);
            }
        }
    }
    terms
}

/// A Design's one-line pitch: the first sentence of its description's first
/// nonblank line, at most [`PITCH_CHARS`] characters, or its title when it has
/// no description.
#[must_use]
pub fn pitch(title: &str, description: Option<&str>) -> String {
    let Some(line) = description
        .into_iter()
        .flat_map(str::lines)
        .map(str::trim)
        .find(|line| !line.is_empty())
    else {
        return title.to_owned();
    };
    let sentence = line
        .match_indices(['.', '?', '!'])
        .find(|(index, _)| line[index + 1..].starts_with(' '))
        .map_or(line, |(index, _)| &line[..=index]);
    if sentence.chars().count() <= PITCH_CHARS {
        return sentence.to_owned();
    }
    let mut cut = sentence.chars().take(PITCH_CHARS - 1).collect::<String>();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(title: &str, description: &str) -> BTreeSet<String> {
        task_terms(title, Some(description), &[])
    }

    /// `count` open statements each holding `generic` plus its own filler.
    fn board(count: usize, generic: &str) -> CommonTerms {
        let filler = [
            "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel",
        ];
        CommonTerms::of(
            filler
                .iter()
                .cycle()
                .take(count)
                .map(|word| task(&format!("{generic} {word}"), "")),
        )
    }

    #[test]
    fn a_task_matches_on_enough_shared_terms_including_a_title_term() {
        let design = DesignTerms::new(
            "Room catch-up pagination",
            Some("Lanes page retained publications through bounded cursors."),
        );
        let matched = design
            .matched(
                &task(
                    "Page room catch-up",
                    "Bound the cursor of retained publications per lane.",
                ),
                &CommonTerms::default(),
            )
            .expect("shared title and statement terms match");
        assert_eq!(
            matched,
            [
                "catch",
                "cursor",
                "lane",
                "page",
                "publication",
                "retained",
                "room"
            ]
        );
    }

    #[test]
    fn shared_terms_outside_the_design_title_or_too_few_do_not_match() {
        let design = DesignTerms::new(
            "Working groups",
            Some("Retained publications page through bounded cursors."),
        );
        assert_eq!(
            design.matched(
                &task(
                    "Cursor paging",
                    "Retained publications and bounded cursors."
                ),
                &CommonTerms::default()
            ),
            None,
            "no shared term names what the Design's title is about"
        );
        assert_eq!(
            design.matched(
                &task("Working groups", "Arrange the board."),
                &CommonTerms::default()
            ),
            None,
            "two shared terms are below the threshold"
        );
    }

    #[test]
    fn board_common_terms_neither_count_nor_satisfy_the_title_requirement() {
        let common = board(8, "lane room work");
        let generic = DesignTerms::new("Lane room work", None);
        let claimed = task("Lane room work", "");
        assert!(generic.matched(&claimed, &CommonTerms::default()).is_some());
        assert_eq!(
            generic.matched(&claimed, &common),
            None,
            "an overlap of only board-common terms invites nothing"
        );

        let titled_by_common = DesignTerms::new(
            "Room lanes",
            Some("Retained publications page through bounded cursors."),
        );
        let paging = task(
            "Paging",
            "Retained publications and bounded cursors in rooms.",
        );
        assert!(
            titled_by_common
                .matched(&paging, &CommonTerms::default())
                .is_some()
        );
        assert_eq!(
            titled_by_common.matched(&paging, &common),
            None,
            "a common title term does not satisfy the title requirement"
        );

        let specific = DesignTerms::new(
            "Room catch-up pagination",
            Some("Lanes page retained publications through bounded cursors."),
        );
        assert_eq!(
            specific
                .matched(
                    &task(
                        "Page room catch-up",
                        "Bound the cursor of retained publications per lane.",
                    ),
                    &common,
                )
                .expect("specific shared terms still match"),
            ["catch", "cursor", "page", "publication", "retained"],
            "matched terms omit the common ones"
        );
    }

    #[test]
    fn a_term_is_common_only_above_its_share_of_a_large_enough_board() {
        assert_eq!(
            board(COMMON_TERM_MIN_STATEMENTS - 1, "lane room work"),
            CommonTerms::default(),
            "a small board discounts nothing"
        );
        let (numerator, denominator) = COMMON_TERM_SHARE;
        let at_share = COMMON_TERM_MIN_STATEMENTS * numerator / denominator;
        let mut statements = (0..COMMON_TERM_MIN_STATEMENTS)
            .map(|_| task("Lane", ""))
            .collect::<Vec<_>>();
        for statement in &mut statements[..at_share] {
            statement.insert("page".to_owned());
        }
        for statement in &mut statements[..=at_share] {
            statement.insert("cursor".to_owned());
        }
        assert_eq!(
            CommonTerms::of(statements),
            CommonTerms(BTreeSet::from(["cursor".to_owned(), "lane".to_owned()])),
            "exactly the share is not common; one statement more is"
        );
    }

    #[test]
    fn a_completion_profile_bounds_validation_bytes_and_distinct_terms() {
        let distinct = (0..COMPLETION_PROFILE_TERMS + 20)
            .map(|index| {
                let first = char::from(b'a' + ((index / (26 * 26)) % 26) as u8);
                let second = char::from(b'a' + ((index / 26) % 26) as u8);
                let third = char::from(b'a' + (index % 26) as u8);
                format!("term{first}{second}{third}")
            })
            .collect::<Vec<_>>();
        let excluded = distinct.last().expect("generated terms").clone();
        let profile = completion_terms(
            "Retained statement marker",
            Some("Statement description"),
            &[],
            &distinct.join(" "),
        );
        assert_eq!(profile.len(), COMPLETION_PROFILE_TERMS);
        assert!(profile.contains("retained"));
        assert!(
            !profile.contains(&excluded),
            "validation terms beyond the profile cap are not retained"
        );

        let validation = format!(
            "{}é boundarymarker",
            "x".repeat(COMPLETION_VALIDATION_BYTES - 1)
        );
        let clipped = completion_terms("Retained statement marker", None, &[], &validation);
        assert!(
            !clipped.contains("boundarymarker"),
            "validation after a partial UTF-8 boundary contributes no terms"
        );
    }

    #[test]
    fn function_words_short_words_digit_runs_and_unrecordable_words_are_not_terms() {
        assert_eq!(
            terms(["The lane's v2 work with each task: designs and rooms"]),
            ["design", "lane", "room", "task", "work"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        );
        assert_eq!(
            terms(["İstanbul 𝐁𝐎𝐋𝐃 builds"]),
            BTreeSet::from(["build".to_owned()]),
            "a word whose lowercase holds a mark or an uppercase letter is never a recorded term"
        );
    }

    #[test]
    fn the_pitch_is_the_first_sentence_bounded_or_the_title() {
        assert_eq!(pitch("Title", None), "Title");
        assert_eq!(
            pitch("Title", Some("\n  First idea. Second idea.\nMore")),
            "First idea."
        );
        let long = "word ".repeat(80);
        let cut = pitch("Title", Some(&long));
        assert_eq!(cut.chars().count(), PITCH_CHARS);
        assert!(cut.ends_with('…'));
    }
}
