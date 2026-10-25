//! Deterministic recent-window TF-IDF similarity between task statements.
//!
//! Documents use lowercase alphabetic terms from authored task fields. Digit
//! runs, short words, and function words are inert; one trailing `s` is folded.
//! A corpus fitted from the most recently revised statements supplies smooth
//! inverse document frequency. Logarithmic term frequency and L2-normalized
//! sparse vectors then produce one cosine score.
//!
//! Term counts are canonical and fit-independent. A [`Prepared`] vector is a
//! derived cache weighted under one fitted [`Generation`]; two vectors compare
//! only when they share it. Every matcher (Design awareness and nudges, the
//! wake and `task next` match, and claim overlap) takes its fit and statement
//! vectors from the one shared [`MatchCache`].

use std::cmp::{Ordering, Reverse};
use std::collections::{BTreeMap, BinaryHeap};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

use wire::TaskHead;
use wire::rooms::{COMPLETION_VALIDATION_BYTES, DESIGN_NOTICE_SHOWN_TERMS};

/// Fewest letters a word needs to count as a term.
pub const MIN_TERM_CHARS: usize = 4;
/// Most recently revised task statements, open or completed, in the frequency
/// corpus.
pub const FREQUENCY_WINDOW: usize = 256;
/// Balanced cosine-similarity base for stance-weighted Design request floors.
pub const MATCH_SCORE_BASE: f64 = 0.15;
/// Least cosine at which two concurrently claimed statements warn of overlap.
pub const CLAIM_OVERLAP_FLOOR: f64 = 0.25;

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

    /// Number of distinct terms counted.
    #[must_use]
    pub fn distinct(&self) -> usize {
        self.0.len()
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

    /// Count one task head's authored statement.
    #[must_use]
    pub fn task(statement: &TaskHead) -> Self {
        Self::statement(
            &statement.title,
            statement.description.as_deref(),
            &statement.acceptance,
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
        let validation = utf8_prefix(validation, COMPLETION_VALIDATION_BYTES);
        Self::from_texts(
            std::iter::once(title)
                .chain(description)
                .chain(acceptance.iter().map(String::as_str))
                .chain(std::iter::once(validation)),
        )
    }

    fn terms(&self) -> impl Iterator<Item = &String> {
        self.0.keys()
    }
}

/// Identity of one fitted document-frequency table.
///
/// Two fits over the same statements share a generation, and any change to the
/// population or a term's document frequency names a different one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Generation([u8; 32]);

impl Generation {
    fn of(statements: usize, document_frequency: &BTreeMap<String, usize>) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&(statements as u64).to_le_bytes());
        for (term, frequency) in document_frequency {
            hasher.update(&(term.len() as u64).to_le_bytes());
            hasher.update(term.as_bytes());
            hasher.update(&(*frequency as u64).to_le_bytes());
        }
        Self(*hasher.finalize().as_bytes())
    }
}

/// A cosine was asked between vectors weighted under different fits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GenerationMismatch {
    /// Generation of the vector the comparison was asked of.
    pub left: Generation,
    /// Generation of the vector it was compared with.
    pub right: Generation,
}

impl fmt::Display for GenerationMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("vectors weighted under different frequency generations do not compare")
    }
}

impl std::error::Error for GenerationMismatch {}

/// One L2-normalized TF-IDF vector and the fit that weighted it.
#[derive(Clone, Debug, PartialEq)]
pub struct Prepared {
    generation: Generation,
    weights: BTreeMap<String, f64>,
}

impl Prepared {
    /// The fit this vector was weighted under.
    #[must_use]
    pub fn generation(&self) -> Generation {
        self.generation
    }

    /// Cosine similarity with another vector of the same generation.
    ///
    /// # Errors
    ///
    /// Refuses vectors weighted under different generations rather than
    /// comparing incommensurable weights.
    pub fn cosine(&self, other: &Self) -> Result<f64, GenerationMismatch> {
        if self.generation != other.generation {
            return Err(GenerationMismatch {
                left: self.generation,
                right: other.generation,
            });
        }
        Ok(dot(&self.weights, &other.weights))
    }

    /// The strongest weighted overlaps with `other`, sorted, as a notice
    /// records them.
    #[must_use]
    pub fn explanation(&self, other: &Self) -> Vec<String> {
        let mut products = self
            .weights
            .iter()
            .filter_map(|(term, weight)| {
                other
                    .weights
                    .get(term)
                    .map(|other_weight| (term, weight * other_weight))
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
}

/// Bounded statement document frequencies shared by both compared texts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrequencyProfile {
    statements: usize,
    document_frequency: BTreeMap<String, usize>,
    generation: Generation,
}

impl FrequencyProfile {
    /// Fit one profile from document-presence counts.
    #[must_use]
    pub fn of(documents: impl IntoIterator<Item = TermCounts>) -> Self {
        let mut statements = 0_usize;
        let mut document_frequency = BTreeMap::<String, usize>::new();
        for document in documents {
            statements = statements.saturating_add(1);
            for term in document.terms() {
                let count = document_frequency.entry(term.clone()).or_default();
                *count = count.saturating_add(1);
            }
        }
        Self {
            generation: Generation::of(statements, &document_frequency),
            statements,
            document_frequency,
        }
    }

    /// The identity of this fit.
    #[must_use]
    pub fn generation(&self) -> Generation {
        self.generation
    }
}

/// The fitted recent window and the task vectors weighted under it, so every
/// matcher over an unchanged board reuses one fit.
///
/// Keys are content, never journal positions: the fit is keyed by the
/// window's statements and each vector by its statement's counted text, so
/// equal keys always mean equal inputs and one cache may serve any journal.
/// Vectors are dropped whole when the window moves; a vector asked under any
/// other fit is weighted afresh and not kept. The lock is a leaf, held only
/// inside these methods.
#[derive(Debug, Default)]
pub struct MatchCache(Mutex<FittedWindow>);

/// Content identity of the authored text [`TermCounts::task`] counts.
type StatementKey = [u8; 32];

/// The content key of exactly the texts [`TermCounts::task`] counts, in its
/// order, so equal keys always count equally.
fn statement_key(statement: &TaskHead) -> StatementKey {
    let mut hasher = blake3::Hasher::new();
    for text in std::iter::once(statement.title.as_str())
        .chain(statement.description.as_deref())
        .chain(statement.acceptance.iter().map(String::as_str))
    {
        hasher.update(&(text.len() as u64).to_le_bytes());
        hasher.update(text.as_bytes());
    }
    *hasher.finalize().as_bytes()
}

#[derive(Debug, Default)]
struct FittedWindow {
    fit: Option<(StatementKey, Arc<FrequencyProfile>)>,
    prepared: BTreeMap<StatementKey, Option<Arc<Prepared>>>,
}

impl MatchCache {
    /// An empty cache.
    #[must_use]
    pub const fn new() -> Self {
        Self(Mutex::new(FittedWindow {
            fit: None,
            prepared: BTreeMap::new(),
        }))
    }

    /// The process's one cache, which every production matcher shares.
    #[must_use]
    pub fn shared() -> &'static Self {
        static SHARED: MatchCache = MatchCache::new();
        &SHARED
    }

    /// The fit over the [`FREQUENCY_WINDOW`] most recently revised task
    /// statements, refitted only when that window's statements changed.
    #[must_use]
    pub fn fit<'a>(&self, statements: impl Iterator<Item = &'a TaskHead>) -> Arc<FrequencyProfile> {
        let mut recent = BinaryHeap::with_capacity(FREQUENCY_WINDOW + 1);
        for statement in statements {
            recent.push(Reverse(RecentStatement(statement)));
            if recent.len() > FREQUENCY_WINDOW {
                recent.pop();
            }
        }
        let recent = recent.into_sorted_vec();
        let mut hasher = blake3::Hasher::new();
        for Reverse(RecentStatement(statement)) in &recent {
            hasher.update(&statement_key(statement));
        }
        let key = *hasher.finalize().as_bytes();
        let mut window = self.window();
        if let Some((current, frequency)) = &window.fit
            && *current == key
        {
            return Arc::clone(frequency);
        }
        let frequency =
            Arc::new(FrequencyProfile::of(recent.into_iter().map(
                |Reverse(RecentStatement(statement))| TermCounts::task(statement),
            )));
        *window = FittedWindow {
            fit: Some((key, Arc::clone(&frequency))),
            prepared: BTreeMap::new(),
        };
        frequency
    }

    /// `statement` weighted under `frequency`, reused while `frequency` is
    /// this cache's current fit. `None` when it has no weightable terms.
    #[must_use]
    pub fn prepare(
        &self,
        frequency: &FrequencyProfile,
        statement: &TaskHead,
    ) -> Option<Arc<Prepared>> {
        let weigh = || {
            frequency
                .prepare(&TermCounts::task(statement))
                .map(Arc::new)
        };
        let mut window = self.window();
        if window
            .fit
            .as_ref()
            .is_none_or(|(_, current)| current.generation != frequency.generation)
        {
            return weigh();
        }
        window
            .prepared
            .entry(statement_key(statement))
            .or_insert_with(weigh)
            .clone()
    }

    /// The cached window; a panic mid-update leaves only an empty cache.
    fn window(&self) -> MutexGuard<'_, FittedWindow> {
        self.0.lock().unwrap_or_else(|poisoned| {
            self.0.clear_poison();
            let mut window = poisoned.into_inner();
            *window = FittedWindow::default();
            window
        })
    }
}

impl FrequencyProfile {
    /// Cosine similarity for two documents fitted against this same profile.
    #[must_use]
    pub fn score(&self, left: &TermCounts, right: &TermCounts) -> Option<f64> {
        let score = self.prepare(left)?.cosine(&self.prepare(right)?).ok()?;
        score.is_finite().then_some(score)
    }

    /// Weight one document under this fit. `None` when it has no terms or
    /// cannot be weighted finitely.
    #[must_use]
    pub fn prepare(&self, document: &TermCounts) -> Option<Prepared> {
        Some(Prepared {
            generation: self.generation,
            weights: self.vector(document)?,
        })
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

/// The longest prefix of `text` within `bytes` that ends on a UTF-8 boundary.
#[must_use]
pub fn utf8_prefix(text: &str, bytes: usize) -> &str {
    let mut end = text.len().min(bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn dot(left: &BTreeMap<String, f64>, right: &BTreeMap<String, f64>) -> f64 {
    left.iter()
        .map(|(term, weight)| weight * right.get(term).copied().unwrap_or(0.0))
        .sum()
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

    fn head(revision: u64, title: &str) -> TaskHead {
        let born = wire::TaskStamp::for_micros(revision as i64).expect("fixture stamp");
        let seq = wire::Seq::new(revision).expect("fixture position");
        TaskHead {
            preparation: None,
            intent: None,
            born: born.clone(),
            queue_origin: seq,
            title: title.to_owned(),
            pitch: "Matching reuses one fit.".to_owned(),
            description: None,
            acceptance: Vec::new(),
            requirements: wire::EnvironmentRequirements::default(),
            project: "board.kernel".to_owned(),
            filing: wire::TaskFiling::Filed,
            priority: None,
            taste: None,
            document_slug: None,
            dependencies: Vec::new(),
            provenance: wire::TaskProvenance::Task(born),
            creation: wire::body::TaskCreationContext::unavailable(
                wire::body::TaskCreationUnavailable::SpiceActor,
            ),
            revision: seq,
        }
    }

    #[test]
    fn a_cache_reuses_one_exact_fit_and_vector_until_its_window_moves() {
        let board = [
            head(1, "Snapshot cursor paging"),
            head(2, "Publication boundary retention"),
            head(3, "Lane team design"),
        ];
        let uncached =
            |statements: &[TaskHead]| FrequencyProfile::of(statements.iter().map(TermCounts::task));
        let cache = MatchCache::default();
        let fit = cache.fit(board.iter());
        assert_eq!(*fit, uncached(&board), "the cached fit is the exact fit");
        assert!(
            Arc::ptr_eq(&fit, &cache.fit(board.iter().rev())),
            "an unchanged window, in any order, reuses its fit"
        );
        let vector = cache.prepare(&fit, &board[0]).expect("weighted statement");
        assert_eq!(
            *vector,
            fit.prepare(&TermCounts::task(&board[0])).expect("weighted")
        );
        assert!(
            Arc::ptr_eq(&vector, &cache.prepare(&fit, &board[0]).expect("weighted")),
            "an unchanged statement reuses its vector"
        );

        // The same positions under other text, as another journal would hold.
        let mut edited = board.clone();
        edited[1] = head(2, "Publication boundary retention cursor");
        let refit = cache.fit(edited.iter());
        assert_eq!(
            *refit,
            uncached(&edited),
            "a changed statement refits the window"
        );
        assert_ne!(refit.generation(), fit.generation());
        let reweighted = cache
            .prepare(&refit, &edited[0])
            .expect("weighted statement");
        assert_eq!(reweighted.generation(), refit.generation());
        close(
            reweighted
                .cosine(&cache.prepare(&refit, &edited[1]).expect("weighted"))
                .expect("one fit"),
            refit
                .score(&TermCounts::task(&edited[0]), &TermCounts::task(&edited[1]))
                .expect("scored"),
        );
        let stale = cache.prepare(&fit, &board[0]).expect("weighted statement");
        assert_eq!(
            stale.generation(),
            fit.generation(),
            "a vector asked under a superseded fit stays in that fit's generation"
        );
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
    fn empty_nonoverlapping_and_diluted_documents_score_low() {
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
        assert!(profile.score(&document(&left), &document(&right)).unwrap() < MATCH_SCORE_BASE);
    }

    #[test]
    fn explanations_keep_the_strongest_products_then_store_them_sorted() {
        let profile = FrequencyProfile::of([document(
            "alpha bravo charlie delta foxtrot gamma hotel india",
        )]);
        let text = "alpha alpha alpha bravo bravo charlie delta foxtrot gamma hotel india";
        let left = profile.prepare(&document(text)).unwrap();
        let right = profile.prepare(&document(text)).unwrap();
        let terms = left.explanation(&right);
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
}
