//! The key grammar and the closed set of dispositions.
//!
//! A directive's key is the one word both sides quote: the lane hears it in
//! rendered direction and writes it back in prose. Every later judgment keys
//! off that word, so it is parsed once, here, into a type that cannot hold
//! anything else.

use std::error::Error;
use std::fmt;
use std::str::FromStr;

use wire::Disposition;

/// A steering key such as `1kJK6N3R`.
///
/// The grammar is a fixed run of ASCII alphanumeric characters. The width is
/// what makes prose recognition exact: an acknowledgment clause is told apart
/// from a sentence that merely uses the word by whether the tokens it names are
/// key-shaped, and a grammar admitting any length would make ordinary words
/// indistinguishable from keys. A key survives being copied through prose, a
/// shell, and a rendered file because nothing in it needs escaping.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key(String);

/// Text that does not spell a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyError(String);

impl Key {
    /// Characters every key carries.
    pub const WIDTH: usize = 8;

    /// The key as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this text is shaped like a key, without minting one.
    ///
    /// Recognition asks this of a token before it decides a clause is an
    /// acknowledgment at all, which is a question about shape rather than a
    /// value it needs.
    #[must_use]
    pub fn shaped(text: &str) -> bool {
        text.len() == Self::WIDTH && text.bytes().all(|byte| byte.is_ascii_alphanumeric())
    }
}

impl FromStr for Key {
    type Err = KeyError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if Self::shaped(text) {
            Ok(Self(text.to_owned()))
        } else {
            Err(KeyError(text.to_owned()))
        }
    }
}

impl fmt::Display for Key {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl fmt::Display for KeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "steering key must be {} alphanumeric characters: {:?}",
            Key::WIDTH,
            self.0
        )
    }
}

impl Error for KeyError {}

/// Every disposition a directive may sit in, and the one word it is spelled
/// with.
///
/// The set is closed by [`wire::Disposition`]; this fixes the spelling so a
/// word arriving as text is read into the vocabulary or refused, never
/// silently treated as no disposition at all.
///
/// `missed` is deliberately absent. A missed key names an acknowledgment that
/// did not land and alters no directive, so it is an observation rather than a
/// state anything retires into.
pub const DISPOSITIONS: [(&str, Disposition); 5] = [
    ("live", Disposition::Live),
    ("acked", Disposition::Acked),
    ("declined", Disposition::Declined),
    ("lapsed", Disposition::Lapsed),
    ("canceled", Disposition::Canceled),
];

/// Text that does not spell a disposition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispositionError(String);

impl fmt::Display for DispositionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let admitted: Vec<&str> = DISPOSITIONS.iter().map(|(word, _)| *word).collect();
        write!(
            formatter,
            "disposition must be one of {}: {:?}",
            admitted.join(", "),
            self.0
        )
    }
}

impl Error for DispositionError {}

/// Reads one disposition word.
///
/// # Errors
///
/// Returns [`DispositionError`] naming the admitted words when the text is
/// outside the closed set.
pub fn disposition(word: &str) -> Result<Disposition, DispositionError> {
    DISPOSITIONS
        .iter()
        .find(|(spelling, _)| *spelling == word)
        .map(|(_, disposition)| *disposition)
        .ok_or_else(|| DispositionError(word.to_owned()))
}

/// The two words assistant prose leads an acknowledgment clause with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Marker {
    /// The directive was heard and acted on.
    Ack,
    /// The directive was refused with a reason.
    Nack,
}

/// Every marker, in the order the protocol states them.
pub const MARKERS: [Marker; 2] = [Marker::Nack, Marker::Ack];

impl Marker {
    /// The marker this word spells, if it spells one.
    #[must_use]
    pub fn read(word: &str) -> Option<Self> {
        MARKERS.into_iter().find(|marker| marker.as_str() == word)
    }

    /// The word this marker is spelled with.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ack => "ACK",
            Self::Nack => "NACK",
        }
    }

    /// The disposition a valid clause under this marker retires into.
    #[must_use]
    pub const fn disposition(self) -> Disposition {
        match self {
            Self::Ack => Disposition::Acked,
            Self::Nack => Disposition::Declined,
        }
    }
}

impl fmt::Display for Marker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::{DISPOSITIONS, Key, Marker, disposition};
    use wire::Disposition;

    #[test]
    fn a_key_is_a_fixed_run_of_alphanumeric_characters() {
        assert_eq!("1kJK6N3R".parse::<Key>().expect("key").as_str(), "1kJK6N3R");
        for refused in [
            "",
            "1kJK6N3",
            "1kJK6N3Rx",
            "1kJK-N3R",
            "1kJK 6N3",
            "1kJK6N3\u{e9}",
        ] {
            assert!(
                refused.parse::<Key>().is_err(),
                "{refused:?} was admitted as a key"
            );
        }
    }

    /// The spellings are the ones the wire already serializes, not a second
    /// vocabulary that agrees today.
    #[test]
    fn every_disposition_word_matches_the_wire_spelling() {
        for (word, admitted) in DISPOSITIONS {
            let encoded = wire::encode(&admitted).expect("disposition encodes");
            assert_eq!(
                String::from_utf8(encoded).expect("encoding is text"),
                format!("\"{word}\"")
            );
            assert_eq!(disposition(word), Ok(admitted));
        }
    }

    /// Reading a disposition refuses rather than reporting nothing, and
    /// `missed` is outside the set on purpose.
    #[test]
    fn an_unknown_disposition_is_refused() {
        for refused in ["missed", "Acked", "acked ", "", "sent"] {
            let error = disposition(refused).expect_err("word is outside the set");
            assert!(
                error.to_string().contains("disposition must be one of"),
                "{refused:?} refused without naming the admitted words"
            );
        }
    }

    /// Exhaustive matching means a sixth wire disposition cannot be added
    /// without this test failing to compile against the missing spelling.
    #[test]
    fn the_closed_set_covers_every_wire_disposition() {
        for admitted in [
            Disposition::Live,
            Disposition::Acked,
            Disposition::Declined,
            Disposition::Lapsed,
            Disposition::Canceled,
        ] {
            let named = match admitted {
                Disposition::Live => "live",
                Disposition::Acked => "acked",
                Disposition::Declined => "declined",
                Disposition::Lapsed => "lapsed",
                Disposition::Canceled => "canceled",
            };
            assert_eq!(disposition(named), Ok(admitted));
        }
        assert_eq!(DISPOSITIONS.len(), 5);
    }

    #[test]
    fn each_marker_names_the_disposition_it_retires_into() {
        assert_eq!(Marker::read("ACK"), Some(Marker::Ack));
        assert_eq!(Marker::read("NACK"), Some(Marker::Nack));
        assert_eq!(Marker::read("ack"), None);
        assert_eq!(Marker::Ack.disposition(), Disposition::Acked);
        assert_eq!(Marker::Nack.disposition(), Disposition::Declined);
    }
}
