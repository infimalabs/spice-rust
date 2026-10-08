//! Pure word-bag matching over typed assistant prose.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use gate::{Applicability, PolicyContext, PolicyResolveError, RuleId, ScopeSelector};
use listen::{Event, Locus};
use wire::{Digest, Domain, Name};

/// One admitted candidate selector.
///
/// Matching is case-insensitive over alphabetic words. Whitespace in a phrase
/// is normalized before matching, and every selector word must match one
/// complete prose word exactly.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Trigger {
    source: String,
    words: Vec<String>,
}

impl Trigger {
    /// Parse one exact alphabetic word or phrase.
    ///
    /// # Errors
    ///
    /// Refuses empty selectors and every non-alphabetic selector term.
    pub fn parse(source: impl AsRef<str>) -> Result<Self, TriggerError> {
        let source = source.as_ref().trim();
        if source.is_empty() {
            return Err(TriggerError::Empty);
        }

        let words = source
            .split_whitespace()
            .map(normalize_selector)
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            source: source.to_owned(),
            words,
        })
    }

    /// Selector spelling supplied by tracked configuration.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.source
    }

    fn spans(&self, tokens: &[Option<String>]) -> Vec<Span> {
        tokens
            .windows(self.words.len())
            .enumerate()
            .filter(|(_, window)| {
                window
                    .iter()
                    .zip(&self.words)
                    .all(|(token, word)| token.as_deref() == Some(word))
            })
            .map(|(first, _)| Span {
                first,
                end: first + self.words.len(),
            })
            .collect()
    }
}

fn normalize_selector(word: &str) -> Result<String, TriggerError> {
    if word.is_empty() || !word.chars().all(char::is_alphabetic) {
        return Err(TriggerError::NotWord(word.to_owned()));
    }
    Ok(lowercase(word))
}

fn lowercase(text: &str) -> String {
    text.chars().flat_map(char::to_lowercase).collect()
}

/// Canonical authored prose used by durable reminder identity.
///
/// Matching is case-insensitive and whitespace-insensitive, so identity uses
/// the same two equivalences while retaining every non-whitespace character.
/// Punctuation remains significant: this is an identity for the whole prose,
/// not for the smaller token stream selectors happen to inspect.
#[must_use]
pub fn normalize_prose(text: &str) -> String {
    lowercase(&text.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Domain-separated identity of one taste firing on one authored prose body.
///
/// The two components are length framed rather than delimiter joined because
/// prose may contain any delimiter. The compaction epoch deliberately is not
/// part of this value: it is carried beside the identity on the reminder fact,
/// allowing the same content to become eligible once in each later epoch.
#[must_use]
pub fn reminder_identity(taste: &Name, prose: &str) -> Digest {
    let normalized = normalize_prose(prose);
    let mut framed = Vec::with_capacity(16 + taste.as_str().len() + normalized.len());
    framed.extend_from_slice(&(taste.as_str().len() as u64).to_be_bytes());
    framed.extend_from_slice(taste.as_str().as_bytes());
    framed.extend_from_slice(&(normalized.len() as u64).to_be_bytes());
    framed.extend_from_slice(normalized.as_bytes());
    Digest::compute(Domain::TasteProse, &framed)
}

/// Canonical identity of one complete ordered configured taste bag.
///
/// Names, selectors, triggers, and vendors are length-framed in their admitted
/// order, and enablement is one fixed byte, so any policy movement produces a
/// new identity without a second serializer.
#[must_use]
pub fn bag_identity(bag: &Bag) -> Digest {
    let mut framed = Vec::new();
    frame_count(&mut framed, bag.tastes.len());
    for taste in &bag.tastes {
        frame_text(&mut framed, taste.name.as_str());
        frame_count(&mut framed, taste.triggers.len());
        for trigger in &taste.triggers {
            frame_text(&mut framed, trigger.as_str());
        }
        frame_text(&mut framed, &taste.message);
        framed.push(u8::from(taste.enabled));
        for values in [
            taste.applicability.selector().paths(),
            taste.applicability.selector().extensions(),
            taste.applicability.selector().vendors(),
            taste.applicability.selector().models(),
            taste.applicability.selector().phases(),
        ] {
            frame_count(&mut framed, values.len());
            for value in values {
                frame_text(&mut framed, value);
            }
        }
    }
    Digest::compute(Domain::TasteBag, &framed)
}

fn frame_count(framed: &mut Vec<u8>, count: usize) {
    framed.extend_from_slice(&(count as u64).to_be_bytes());
}

fn frame_text(framed: &mut Vec<u8>, text: &str) {
    frame_count(framed, text.len());
    framed.extend_from_slice(text.as_bytes());
}

fn prose_tokens(text: &str) -> (Vec<Option<String>>, Vec<(usize, usize)>) {
    // A digit attached to letters makes the whole alphanumeric run inert. It
    // remains as a barrier so `fall 2 back` cannot collapse into the phrase
    // `fall back`, while punctuation and whitespace still delimit words.
    let mut tokens = Vec::new();
    let mut loci = Vec::new();
    let mut current = String::new();
    let mut alphabetic = true;
    let mut line = 1;
    let mut line_start = 0;

    for (offset, character) in text.char_indices() {
        if character.is_alphanumeric() {
            if current.is_empty() {
                loci.push((line, offset - line_start + 1));
                alphabetic = true;
            }
            current.extend(character.to_lowercase());
            alphabetic &= character.is_alphabetic();
        } else {
            if !current.is_empty() {
                tokens.push(alphabetic.then(|| std::mem::take(&mut current)));
                current.clear();
            }
            if character == '\n' {
                line += 1;
                line_start = offset + 1;
            }
        }
    }
    if !current.is_empty() {
        tokens.push(alphabetic.then_some(current));
    }

    (tokens, loci)
}

/// Why a trigger spelling was refused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TriggerError {
    Empty,
    NotWord(String),
}

impl fmt::Display for TriggerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("a taste trigger cannot be empty"),
            Self::NotWord(word) => write!(
                formatter,
                "taste trigger term {word:?} is not an alphabetic word"
            ),
        }
    }
}

impl Error for TriggerError {}

/// One tracked taste definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Taste {
    name: Name,
    triggers: Vec<Trigger>,
    message: String,
    enabled: bool,
    applicability: Applicability,
}

impl Taste {
    /// Build one taste from its tracked word bag and imperative message.
    ///
    /// # Errors
    ///
    /// Refuses an empty bag, an empty message, or any invalid trigger.
    pub fn new<I, S>(name: Name, words: I, message: impl Into<String>) -> Result<Self, BagError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self::configured(name, words, message, true, &ScopeSelector::default())
    }

    pub(crate) fn configured<I, S>(
        name: Name,
        words: I,
        message: impl Into<String>,
        enabled: bool,
        scopes: &ScopeSelector,
    ) -> Result<Self, BagError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let triggers = words
            .into_iter()
            .map(Trigger::parse)
            .collect::<Result<Vec<_>, _>>()?;
        if triggers.is_empty() {
            return Err(BagError::EmptyWords(name.to_string()));
        }
        let message = message.into();
        if message.trim().is_empty() {
            return Err(BagError::EmptyMessage(name.to_string()));
        }
        let subject = name
            .as_str()
            .parse::<RuleId>()
            .expect("every admitted taste name is a valid selector subject");
        let applicability = Applicability::compile(subject, scopes)?;
        Ok(Self {
            name,
            triggers,
            message,
            enabled,
            applicability,
        })
    }

    /// Stable one-word taste identity.
    #[must_use]
    pub const fn name(&self) -> &Name {
        &self.name
    }

    /// Candidate selectors in tracked order.
    #[must_use]
    pub fn triggers(&self) -> &[Trigger] {
        &self.triggers
    }

    /// Imperative presented when a candidate becomes a live reminder.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Whether this named tracked row participates in matching.
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    /// Canonical shared selector constraining this taste.
    #[must_use]
    pub const fn scopes(&self) -> &ScopeSelector {
        self.applicability.selector()
    }
}

/// A closed configured set of tracked tastes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Bag {
    tastes: Vec<Taste>,
}

impl Bag {
    /// Admit a configured set with one row per taste name.
    ///
    /// # Errors
    ///
    /// Refuses a repeated taste identity.
    pub fn new(tastes: Vec<Taste>) -> Result<Self, BagError> {
        let mut names = BTreeSet::new();
        for taste in &tastes {
            if !names.insert(taste.name().as_str()) {
                return Err(BagError::Duplicate(taste.name().to_string()));
            }
        }
        Ok(Self { tastes })
    }

    /// Named tracked tastes in tracked order, including disabled rows.
    #[must_use]
    pub fn tastes(&self) -> &[Taste] {
        &self.tastes
    }

    /// Select applicable configured candidates from one typed listening event.
    ///
    /// Only [`Event::Prose`] is readable here. Reasoning, tools, usage,
    /// compaction, and failure events produce no candidates, and this API has
    /// no vendor-byte input to parse.
    /// # Errors
    ///
    /// Refuses malformed context or any selector axis the caller did not
    /// supply.
    pub fn hits(
        &self,
        event: &Event,
        context: &PolicyContext,
    ) -> Result<Vec<Hit>, PolicyResolveError> {
        let Event::Prose(prose) = event else {
            return Ok(Vec::new());
        };
        let (tokens, loci) = prose_tokens(&prose.text);
        let mut hits = Vec::new();
        for taste in self.tastes.iter().filter(|taste| taste.enabled) {
            if !taste.applicability.matches(context)? {
                continue;
            }
            for trigger in &taste.triggers {
                for span in trigger.spans(&tokens) {
                    let (line, column) = loci[span.first];
                    hits.push(Hit {
                        taste: taste.name.clone(),
                        trigger: trigger.source.clone(),
                        message: taste.message.clone(),
                        span,
                        context: prose.text.clone(),
                        locus: prose.locus,
                        line,
                        column,
                    });
                }
            }
        }
        Ok(hits)
    }
}

/// Why a taste bag was refused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BagError {
    Trigger(TriggerError),
    Applicability(gate::PolicyCompileError),
    EmptyWords(String),
    EmptyMessage(String),
    Duplicate(String),
}

impl fmt::Display for BagError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Trigger(error) => error.fmt(formatter),
            Self::Applicability(error) => error.fmt(formatter),
            Self::EmptyWords(name) => write!(formatter, "taste {name} has an empty word bag"),
            Self::EmptyMessage(name) => write!(formatter, "taste {name} has an empty message"),
            Self::Duplicate(name) => write!(formatter, "taste {name} is declared more than once"),
        }
    }
}

impl Error for BagError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Trigger(error) => Some(error),
            Self::Applicability(error) => Some(error),
            Self::EmptyWords(_) | Self::EmptyMessage(_) | Self::Duplicate(_) => None,
        }
    }
}

impl From<TriggerError> for BagError {
    fn from(error: TriggerError) -> Self {
        Self::Trigger(error)
    }
}

impl From<gate::PolicyCompileError> for BagError {
    fn from(error: gate::PolicyCompileError) -> Self {
        Self::Applicability(error)
    }
}

/// Half-open token positions selected by one trigger.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Span {
    pub first: usize,
    pub end: usize,
}

/// One selected prose candidate.
///
/// A hit is inert evidence and cannot mint a fact itself. Resident recognition
/// combines it with current lane, epoch, steering, and transaction authority
/// before minting the one reminder the runtime law admits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Hit {
    taste: Name,
    trigger: String,
    message: String,
    span: Span,
    context: String,
    locus: Locus,
    line: usize,
    column: usize,
}

impl Hit {
    #[must_use]
    pub const fn taste(&self) -> &Name {
        &self.taste
    }

    #[must_use]
    pub fn trigger(&self) -> &str {
        &self.trigger
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    #[must_use]
    pub const fn span(&self) -> Span {
        self.span
    }

    #[must_use]
    pub fn context(&self) -> &str {
        &self.context
    }

    #[must_use]
    pub const fn locus(&self) -> Locus {
        self.locus
    }

    /// One-based prose line at which the selected trigger begins.
    #[must_use]
    pub const fn line(&self) -> usize {
        self.line
    }

    /// One-based byte column at which the selected trigger begins.
    #[must_use]
    pub const fn column(&self) -> usize {
        self.column
    }
}
