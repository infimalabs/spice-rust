//! Deterministic recent-window TF-IDF matching between completed work and
//! passive Designs.
//!
//! Documents use lowercase alphabetic terms from authored task fields. Digit
//! runs, short words, and function words are inert; one trailing `s` is folded.
//! A corpus fitted from the most recently revised statements supplies smooth
//! inverse document frequency. Logarithmic term frequency and L2-normalized
//! sparse vectors then produce one cosine score.

use std::cmp::{Ordering, Reverse};
use std::collections::{BTreeMap, BinaryHeap};

pub use wire::rooms::COMPLETION_VALIDATION_BYTES;
use wire::rooms::DESIGN_NOTICE_SHOWN_TERMS;
use wire::{TaskHead, TaskStamp};

/// Fewest letters a word needs to count as a term.
pub const MIN_TERM_CHARS: usize = 4;
/// Most characters of a Design's one-line pitch.
pub const PITCH_CHARS: usize = 160;
/// Most rendered bytes of whole open questions one Design request carries.
pub const DESIGN_REQUEST_QUESTIONS_BYTES: usize = 2 * 1024;
/// Most recently revised task statements, open or completed, in the frequency
/// corpus.
pub const FREQUENCY_WINDOW: usize = 256;
/// Minimum cosine similarity that can create a Design request.
pub const MATCH_SCORE_FLOOR: f64 = 0.15;

/// Common words that say nothing about what a statement concerns.
const STOP: &[&str] = &[
    "about", "after", "also", "been", "before", "being", "both", "cannot", "could", "does", "each",
    "every", "from", "have", "into", "just", "like", "many", "more", "most", "much", "must",
    "never", "none", "only", "other", "over", "same", "should", "some", "such", "than", "that",
    "their", "them", "then", "there", "these", "they", "this", "those", "through", "under",
    "until", "very", "what", "when", "where", "whether", "which", "while", "will", "with",
    "within", "without", "would", "your",
];

/// Normalized term occurrence counts for one document.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TermCounts(BTreeMap<String, u32>);

impl TermCounts {
    /// Count terms across arbitrary document fields.
    #[must_use]
    pub fn from_texts<'a>(texts: impl IntoIterator<Item = &'a str>) -> Self {
        let mut counts = BTreeMap::<String, u32>::new();
        for text in texts {
            for run in text.split(|c: char| !c.is_alphanumeric()) {
                if let Some(term) = normalize(run) {
                    let count = counts.entry(term).or_default();
                    *count = count.saturating_add(1);
                }
            }
        }
        Self(counts)
    }

    /// Count a task statement's title, description, and itemized acceptance.
    #[must_use]
    pub fn statement(title: &str, description: Option<&str>, acceptance: &[String]) -> Self {
        Self::from_texts(
            std::iter::once(title)
                .chain(description)
                .chain(acceptance.iter().map(String::as_str)),
        )
    }

    /// Count a completed statement plus its bounded successful validation.
    #[must_use]
    pub fn completion(
        title: &str,
        description: Option<&str>,
        acceptance: &[String],
        validation: &str,
    ) -> Self {
        let mut validation_end = validation.len().min(COMPLETION_VALIDATION_BYTES);
        while !validation.is_char_boundary(validation_end) {
            validation_end -= 1;
        }
        Self::from_texts(
            std::iter::once(title)
                .chain(description)
                .chain(acceptance.iter().map(String::as_str))
                .chain(std::iter::once(&validation[..validation_end])),
        )
    }

    fn terms(&self) -> impl Iterator<Item = &String> {
        self.0.keys()
    }
}

/// Bounded statement document frequencies shared by both compared texts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FrequencyProfile {
    statements: usize,
    document_frequency: BTreeMap<String, usize>,
}

impl FrequencyProfile {
    /// Fit one profile from document-presence counts.
    #[must_use]
    pub fn of(documents: impl IntoIterator<Item = TermCounts>) -> Self {
        let mut profile = Self::default();
        for document in documents {
            profile.statements = profile.statements.saturating_add(1);
            for term in document.terms() {
                let count = profile.document_frequency.entry(term.clone()).or_default();
                *count = count.saturating_add(1);
            }
        }
        profile
    }

    /// Fit from the [`FREQUENCY_WINDOW`] most recently revised task statements.
    #[must_use]
    pub fn recent<'a>(statements: impl Iterator<Item = &'a TaskHead>) -> Self {
        let mut recent = BinaryHeap::with_capacity(FREQUENCY_WINDOW + 1);
        for statement in statements {
            recent.push(Reverse(RecentStatement(statement)));
            if recent.len() > FREQUENCY_WINDOW {
                recent.pop();
            }
        }
        Self::of(
            recent
                .into_iter()
                .map(|Reverse(RecentStatement(statement))| {
                    TermCounts::statement(
                        &statement.title,
                        statement.description.as_deref(),
                        &statement.acceptance,
                    )
                }),
        )
    }

    /// Cosine similarity for two documents fitted against this same profile.
    #[must_use]
    pub fn score(&self, left: &TermCounts, right: &TermCounts) -> Option<f64> {
        let left = self.vector(left)?;
        let right = self.vector(right)?;
        let score = dot(&left, &right);
        score.is_finite().then_some(score)
    }

    /// Select the highest qualifying candidate; lower durable identity wins an
    /// exact finite-score tie. Explanations retain the strongest weighted
    /// overlaps, then sort those terms for the wire contract.
    #[must_use]
    pub fn select<'a, T>(
        &self,
        completion: &TermCounts,
        candidates: impl IntoIterator<Item = MatchCandidate<'a, T>>,
    ) -> Option<(&'a T, Vec<String>)> {
        let completion = self.vector(completion)?;
        let mut best: Option<(&TaskStamp, &T, f64, Vec<String>)> = None;
        for candidate in candidates {
            let Some(design) = self.vector(&candidate.document) else {
                continue;
            };
            let score = dot(&design, &completion);
            if !score.is_finite() || score < MATCH_SCORE_FLOOR {
                continue;
            }
            let replace =
                best.as_ref()
                    .is_none_or(|(identity, _, known, _)| match score.total_cmp(known) {
                        Ordering::Greater => true,
                        Ordering::Equal => candidate.identity < *identity,
                        Ordering::Less => false,
                    });
            if replace {
                best = Some((
                    candidate.identity,
                    candidate.value,
                    score,
                    explanation(&design, &completion),
                ));
            }
        }
        best.map(|(_, value, _, terms)| (value, terms))
    }

    fn vector(&self, document: &TermCounts) -> Option<BTreeMap<String, f64>> {
        let population = self.statements as f64;
        let mut weights = BTreeMap::new();
        let mut squared_norm = 0.0;
        for (term, count) in &document.0 {
            let frequency = *self.document_frequency.get(term).unwrap_or(&0) as f64;
            let tf = 1.0 + f64::from(*count).ln();
            let idf = 1.0 + ((population + 1.0) / (frequency + 1.0)).ln();
            let weight = tf * idf;
            if !weight.is_finite() {
                return None;
            }
            squared_norm += weight * weight;
            weights.insert(term.clone(), weight);
        }
        let norm = squared_norm.sqrt();
        if !norm.is_finite() || norm == 0.0 {
            return None;
        }
        for weight in weights.values_mut() {
            *weight /= norm;
        }
        Some(weights)
    }
}

/// One eligible value and the document scored under its durable identity.
pub struct MatchCandidate<'a, T> {
    identity: &'a TaskStamp,
    value: &'a T,
    document: TermCounts,
}

impl<'a, T> MatchCandidate<'a, T> {
    #[must_use]
    pub fn new(identity: &'a TaskStamp, value: &'a T, document: TermCounts) -> Self {
        Self {
            identity,
            value,
            document,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct RecentStatement<'a>(&'a TaskHead);

impl PartialEq for RecentStatement<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.0.revision == other.0.revision
    }
}

impl Eq for RecentStatement<'_> {}

impl PartialOrd for RecentStatement<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RecentStatement<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.revision.cmp(&other.0.revision)
    }
}

fn normalize(run: &str) -> Option<String> {
    if run.chars().any(char::is_numeric) {
        return None;
    }
    let word = run.to_lowercase();
    let word = match word.strip_suffix('s') {
        Some(stem) if stem.chars().count() >= MIN_TERM_CHARS && !stem.ends_with('s') => {
            stem.to_owned()
        }
        _ => word,
    };
    (word.chars().count() >= MIN_TERM_CHARS
        && word.chars().all(|c| c.is_alphabetic() && !c.is_uppercase())
        && !STOP.contains(&word.as_str()))
    .then_some(word)
}

fn dot(left: &BTreeMap<String, f64>, right: &BTreeMap<String, f64>) -> f64 {
    left.iter()
        .map(|(term, weight)| weight * right.get(term).copied().unwrap_or(0.0))
        .sum()
}

fn explanation(design: &BTreeMap<String, f64>, completion: &BTreeMap<String, f64>) -> Vec<String> {
    let mut products = design
        .iter()
        .filter_map(|(term, design_weight)| {
            completion
                .get(term)
                .map(|completion_weight| (term, design_weight * completion_weight))
        })
        .collect::<Vec<_>>();
    products.sort_by(|(left_term, left), (right_term, right)| {
        right
            .total_cmp(left)
            .then_with(|| left_term.cmp(right_term))
    });
    let mut terms = products
        .into_iter()
        .take(DESIGN_NOTICE_SHOWN_TERMS)
        .map(|(term, _)| term.clone())
        .collect::<Vec<_>>();
    terms.sort();
    terms
}

/// A Design's one-line pitch: the first sentence of its description that is
/// more than a one-word label, at most [`PITCH_CHARS`] characters, or its title
/// when the description has no such sentence.
#[must_use]
pub fn pitch(title: &str, description: Option<&str>) -> String {
    let Some(description) = description.map(str::trim).filter(|text| !text.is_empty()) else {
        return title.to_owned();
    };
    let mut start = 0;
    let mut selected = None;
    for (index, punctuation) in description.match_indices(['.', '?', '!']) {
        let end = index + punctuation.len();
        if end < description.len() && !description[end..].starts_with(char::is_whitespace) {
            continue;
        }
        let sentence = description[start..end].trim();
        if sentence.split_whitespace().nth(1).is_some() {
            selected = Some(sentence);
            break;
        }
        start = end;
    }
    let sentence = selected
        .or_else(|| {
            let trailing = description[start..].trim();
            trailing.split_whitespace().nth(1).map(|_| trailing)
        })
        .unwrap_or(title);
    let mut one_line = String::with_capacity(sentence.len().min(PITCH_CHARS));
    for word in sentence.split_whitespace() {
        if !one_line.is_empty() {
            one_line.push(' ');
        }
        one_line.push_str(word);
    }
    if one_line.chars().count() <= PITCH_CHARS {
        return one_line;
    }
    let mut cut = one_line.chars().take(PITCH_CHARS - 1).collect::<String>();
    cut.push('…');
    cut
}

/// The longest in-order prefix of whole open questions that fits one request.
#[must_use]
pub fn request_questions(questions: &[String]) -> Vec<String> {
    let mut used: usize = 0;
    questions
        .iter()
        .take_while(|question| {
            let rendered = question.len().saturating_add(3);
            let fits = used.saturating_add(rendered) <= DESIGN_REQUEST_QUESTIONS_BYTES;
            if fits {
                used += rendered;
            }
            fits
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(text: &str) -> TermCounts {
        TermCounts::from_texts([text])
    }

    fn generated_word(index: usize) -> String {
        let first = char::from(b'a' + ((index / (26 * 26)) % 26) as u8);
        let second = char::from(b'a' + ((index / 26) % 26) as u8);
        let third = char::from(b'a' + (index % 26) as u8);
        format!("term{first}{second}{third}")
    }

    fn close(left: f64, right: f64) {
        assert!((left - right).abs() < 1e-12, "{left} != {right}");
    }

    #[test]
    fn term_and_document_frequency_are_logarithmic_presence_counts() {
        let profile = FrequencyProfile::of([document("alpha alpha beta"), document("alpha gamma")]);
        assert_eq!(profile.statements, 2);
        assert_eq!(profile.document_frequency["alpha"], 2);
        assert_eq!(profile.document_frequency["beta"], 1);

        let vector = profile.vector(&document("alpha alpha beta")).unwrap();
        let alpha = (1.0 + 2.0_f64.ln()) * (1.0 + (3.0_f64 / 3.0).ln());
        let beta = (1.0 + 1.0_f64.ln()) * (1.0 + (3.0_f64 / 2.0).ln());
        let norm = (alpha * alpha + beta * beta).sqrt();
        close(vector["alpha"], alpha / norm);
        close(vector["beta"], beta / norm);
        close(vector.values().map(|weight| weight * weight).sum(), 1.0);
    }

    #[test]
    fn empty_nonoverlapping_and_below_floor_documents_never_select() {
        let profile = FrequencyProfile::of([document("alpha beta")]);
        assert!(
            profile
                .score(&TermCounts::default(), &document("alpha"))
                .is_none()
        );
        close(
            profile
                .score(&document("alpha"), &document("bravo"))
                .unwrap(),
            0.0,
        );

        let shared = generated_word(0);
        let left = (0..64).map(generated_word).collect::<Vec<_>>().join(" ");
        let right = std::iter::once(shared)
            .chain((64..127).map(generated_word))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(profile.score(&document(&left), &document(&right)).unwrap() < MATCH_SCORE_FLOOR);
        let identity = TaskStamp::for_micros(1).unwrap();
        let candidate = 1_u8;
        assert!(
            profile
                .select(
                    &document(&right),
                    [MatchCandidate::new(&identity, &candidate, document(&left))]
                )
                .is_none()
        );
        assert_eq!(MATCH_SCORE_FLOOR, 0.15);
    }

    #[test]
    fn selection_ranks_by_cosine_then_durable_identity() {
        let profile = FrequencyProfile::of([document("alpha beta gamma")]);
        let completion = document("alpha beta");
        let first = TaskStamp::for_micros(1).unwrap();
        let second = TaskStamp::for_micros(2).unwrap();
        let best = 3_u8;
        let lesser = 2_u8;
        assert_eq!(
            profile
                .select(
                    &completion,
                    [
                        MatchCandidate::new(&first, &lesser, document("alpha")),
                        MatchCandidate::new(&second, &best, document("alpha beta")),
                    ],
                )
                .map(|(value, _)| *value),
            Some(best)
        );

        let lower_identity = 1_u8;
        let higher_identity = 2_u8;
        assert_eq!(
            profile
                .select(
                    &completion,
                    [
                        MatchCandidate::new(&second, &higher_identity, document("alpha beta")),
                        MatchCandidate::new(&first, &lower_identity, document("alpha beta")),
                    ],
                )
                .map(|(value, _)| *value),
            Some(lower_identity)
        );
    }

    #[test]
    fn explanations_keep_the_strongest_products_then_store_them_sorted() {
        let profile = FrequencyProfile::of([document(
            "alpha bravo charlie delta foxtrot gamma hotel india",
        )]);
        let completion =
            document("alpha alpha alpha bravo bravo charlie delta foxtrot gamma hotel india");
        let identity = TaskStamp::for_micros(1).unwrap();
        let candidate = 1_u8;
        let (_, terms) = profile
            .select(
                &completion,
                [MatchCandidate::new(
                    &identity,
                    &candidate,
                    document(
                        "alpha alpha alpha bravo bravo charlie delta foxtrot gamma hotel india",
                    ),
                )],
            )
            .unwrap();
        assert_eq!(
            terms,
            ["alpha", "bravo", "charlie", "delta", "foxtrot", "gamma"]
        );
        assert_eq!(terms.len(), DESIGN_NOTICE_SHOWN_TERMS);
    }

    #[test]
    fn statement_items_contribute_and_completion_validation_is_only_byte_bounded() {
        let statement = TermCounts::statement(
            "Snapshot proposal",
            None,
            &["Should retained cursors bound each publication page?".to_owned()],
        );
        assert!(statement.0.contains_key("cursor"));
        assert!(statement.0.contains_key("publication"));

        let distinct = (0..300).map(generated_word).collect::<Vec<_>>();
        let completion = TermCounts::completion(
            "Retained statement marker",
            Some("Statement description"),
            &[],
            &distinct.join(" "),
        );
        assert!(
            completion.0.len() > FREQUENCY_WINDOW,
            "the bounded validation has no distinct-term cap"
        );

        let validation = format!(
            "{}é boundarymarker",
            "x".repeat(COMPLETION_VALIDATION_BYTES - 1)
        );
        let clipped = TermCounts::completion("Retained statement marker", None, &[], &validation);
        assert!(
            !clipped.0.contains_key("boundarymarker"),
            "validation after a partial UTF-8 boundary contributes no terms"
        );
    }

    #[test]
    fn function_words_short_words_digit_runs_and_unrecordable_words_are_not_terms() {
        assert_eq!(
            document("The lane's v2 work with each task: designs and rooms")
                .0
                .into_keys()
                .collect::<Vec<_>>(),
            ["design", "lane", "room", "task", "work"]
        );
        assert_eq!(
            document("İstanbul 𝐁𝐎𝐋𝐃 builds")
                .0
                .into_keys()
                .collect::<Vec<_>>(),
            ["build"],
            "a word whose lowercase holds a mark or an uppercase letter is never recorded"
        );
    }

    #[test]
    fn the_pitch_skips_a_label_and_is_bounded_or_the_title() {
        assert_eq!(pitch("Title", None), "Title");
        assert_eq!(
            pitch("Title", Some("\n  First idea. Second idea.\nMore")),
            "First idea."
        );
        assert_eq!(
            pitch(
                "Title",
                Some("Problem.\nThe room request needs a useful summary. More detail.")
            ),
            "The room request needs a useful summary."
        );
        let long = "word ".repeat(80);
        let cut = pitch("Title", Some(&long));
        assert_eq!(cut.chars().count(), PITCH_CHARS);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn request_questions_keep_only_the_whole_in_order_prefix_that_fits() {
        let first = "a".repeat(DESIGN_REQUEST_QUESTIONS_BYTES - 3);
        let questions = [first.clone(), "does not fit".to_owned(), "short".to_owned()];
        assert_eq!(request_questions(&questions), [first]);
    }
}
