//! Pure word-bag matching over typed assistant prose.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use listen::{Event, Locus};
use wire::Name;

/// One admitted candidate selector.
///
/// Matching is case-insensitive over tokens split at non-alphanumeric
/// characters. Whitespace in a phrase is normalized before matching. A bare
/// word matches one complete token, and one trailing `*` matches suffixes of a
/// single token. No other glob position is admitted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Trigger {
    source: String,
    matcher: Matcher,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Matcher {
    Exact(Vec<String>),
    Prefix(String),
}

impl Trigger {
    /// Parse one exact word, exact phrase, or trailing-suffix glob.
    ///
    /// # Errors
    ///
    /// Refuses empty selectors, punctuation, and a `*` anywhere except the
    /// end of a single word.
    pub fn parse(source: impl AsRef<str>) -> Result<Self, TriggerError> {
        let source = source.as_ref().trim();
        if source.is_empty() {
            return Err(TriggerError::Empty);
        }

        let pieces = source.split_whitespace().collect::<Vec<_>>();
        let stars = source.chars().filter(|character| *character == '*').count();
        let glob = stars == 1 && source.ends_with('*');
        if stars > 0 && (!glob || pieces.len() != 1) {
            return Err(TriggerError::GlobPosition(source.to_owned()));
        }

        let words = pieces
            .iter()
            .map(|piece| piece.trim_end_matches('*'))
            .map(normalize_selector)
            .collect::<Result<Vec<_>, _>>()?;
        let matcher = if glob {
            Matcher::Prefix(words[0].clone())
        } else {
            Matcher::Exact(words)
        };

        Ok(Self {
            source: source.to_owned(),
            matcher,
        })
    }

    /// Selector spelling supplied by tracked configuration.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.source
    }

    fn spans(&self, tokens: &[String]) -> Vec<Span> {
        match &self.matcher {
            Matcher::Exact(words) => tokens
                .windows(words.len())
                .enumerate()
                .filter(|(_, window)| *window == words.as_slice())
                .map(|(first, _)| Span {
                    first,
                    end: first + words.len(),
                })
                .collect(),
            Matcher::Prefix(prefix) => tokens
                .iter()
                .enumerate()
                .filter(|(_, token)| token.starts_with(prefix))
                .map(|(first, _)| Span {
                    first,
                    end: first + 1,
                })
                .collect(),
        }
    }
}

fn normalize_selector(word: &str) -> Result<String, TriggerError> {
    if word.is_empty() || !word.chars().all(char::is_alphanumeric) {
        return Err(TriggerError::NotWord(word.to_owned()));
    }
    Ok(lowercase(word))
}

fn lowercase(text: &str) -> String {
    text.chars().flat_map(char::to_lowercase).collect()
}

fn prose_tokens(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(lowercase)
        .collect()
}

/// Why a trigger spelling was refused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TriggerError {
    Empty,
    GlobPosition(String),
    NotWord(String),
}

impl fmt::Display for TriggerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("a taste trigger cannot be empty"),
            Self::GlobPosition(trigger) => write!(
                formatter,
                "taste trigger {trigger:?} places `*` outside one trailing word"
            ),
            Self::NotWord(word) => write!(formatter, "taste trigger term {word:?} is not a word"),
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
        Ok(Self {
            name,
            triggers,
            message,
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

    /// Imperative presented when a candidate is confirmed.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// A closed active set of tracked tastes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Bag {
    tastes: Vec<Taste>,
}

impl Bag {
    /// Admit an active set with one row per taste name.
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

    /// Active tastes in tracked order.
    #[must_use]
    pub fn tastes(&self) -> &[Taste] {
        &self.tastes
    }

    /// Select judgment candidates from one typed listening event.
    ///
    /// Only [`Event::Prose`] is readable here. Reasoning, tools, usage,
    /// compaction, and failure events produce no candidates, and this API has
    /// no vendor-byte input to parse.
    #[must_use]
    pub fn hits(&self, event: &Event) -> Vec<Hit> {
        let Event::Prose(prose) = event else {
            return Vec::new();
        };
        let tokens = prose_tokens(&prose.text);
        self.tastes
            .iter()
            .flat_map(|taste| {
                taste.triggers.iter().flat_map(|trigger| {
                    trigger.spans(&tokens).into_iter().map(|span| Hit {
                        taste: taste.name.clone(),
                        trigger: trigger.source.clone(),
                        message: taste.message.clone(),
                        span,
                        context: prose.text.clone(),
                        locus: prose.locus,
                    })
                })
            })
            .collect()
    }
}

/// Why a taste bag was refused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BagError {
    Trigger(TriggerError),
    EmptyWords(String),
    EmptyMessage(String),
    Duplicate(String),
}

impl fmt::Display for BagError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Trigger(error) => error.fmt(formatter),
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
            Self::EmptyWords(_) | Self::EmptyMessage(_) | Self::Duplicate(_) => None,
        }
    }
}

impl From<TriggerError> for BagError {
    fn from(error: TriggerError) -> Self {
        Self::Trigger(error)
    }
}

/// Half-open token positions selected by one trigger.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Span {
    pub first: usize,
    pub end: usize,
}

/// One prose candidate requiring judgment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Hit {
    taste: Name,
    trigger: String,
    message: String,
    span: Span,
    context: String,
    locus: Locus,
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
}
