//! The key grammar and the closed set of dispositions.
//!
//! A directive's key is the one word both sides quote: the lane hears it in
//! rendered direction and writes it back in prose. Every later judgment keys
//! off that word, so it is parsed once, here, into a type that cannot hold
//! anything else.

use std::error::Error;
use std::fmt;
use std::str::FromStr;

use wire::{CanonicalUuid, Disposition};

const ALPHABET: &[u8; 52] = b"0123456789BCDFGHJKLMNPQRSTVWXYZbcdfghjklmnpqrstvwxyz";
const SPACE: u64 = 52_u64.pow(Key::WIDTH as u32);

/// A steering key such as `1kJK6N3R`.
///
/// The width is fixed at [`Key::WIDTH`], which is what makes prose recognition
/// exact: an acknowledgment clause is told apart from a sentence that merely
/// uses the word by whether the tokens it names are key-shaped, and a grammar
/// admitting any length would make ordinary words indistinguishable from keys.
/// A key survives being copied through prose, a shell, and a rendered file
/// because nothing in it needs escaping.
///
/// The character set is asked two different ways, and the two do not agree on
/// purpose. [`Key::shaped`] is the mint's own vowel-free base-52 alphabet and
/// answers recognition; [`Key::admitted`] is every alphanumeric, and answers
/// parsing, because a retired base-62 mint left durable keys that still have to
/// replay. Read either doc before narrowing one to match the other.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key(String);

/// Text that does not spell a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyError(String);

/// An epoch millisecond that cannot be represented by a steering key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyMintError(i64);

impl Key {
    /// Characters every key carries.
    pub const WIDTH: usize = 8;

    /// The key as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this text is shaped like a key the current mint would produce.
    ///
    /// Recognition asks this of a token before it decides a clause is an
    /// acknowledgment at all, which is a question about shape rather than a
    /// value it needs. It is deliberately the narrow question: no English word
    /// of this width survives an alphabet with the vowels removed, so prose
    /// that merely discusses acknowledging never names a key by accident.
    #[must_use]
    pub fn shaped(text: &str) -> bool {
        text.len() == Self::WIDTH && text.bytes().all(|byte| ALPHABET.contains(&byte))
    }

    /// Whether this text spells a key any mint of this system ever wrote.
    ///
    /// Parsing is the wider question, and it has to be. Before the moment codec
    /// there was a base-62 mint over the ASCII alphanumerics, and the keys it
    /// wrote are durable: they sit in the journal as `steer::sent` bodies that
    /// [`crate::Ledger::from_snapshot`] reads on every replay. Refusing them
    /// here does not reject one row, it aborts the replay at that fact and
    /// loses every later one, which is how a narrowed grammar takes a working
    /// resident down.
    ///
    /// The cost of the split is narrow, and worth naming exactly rather than
    /// waving at. A placed acknowledgment clause judges its tokens through
    /// this grammar, so a pre-codec key still retires where acknowledgments
    /// are actually written. What [`Key::shaped`] governs is the diagnostic
    /// for a clause that was *misplaced*: a pre-codec key quoted or buried
    /// mid-line goes unreported rather than unretired. That is the direction
    /// worth failing in — a key nothing can parse is unusable everywhere,
    /// while a key whose misplacement goes unmentioned still lands the moment
    /// it is written in a clause of its own.
    #[must_use]
    pub fn admitted(text: &str) -> bool {
        text.len() == Self::WIDTH && text.bytes().all(|byte| byte.is_ascii_alphanumeric())
    }

    /// Mint the time-ordered key for an epoch millisecond.
    ///
    /// This is the predecessor's authoritative codec: eight fixed-width
    /// base-52 digits, with vowels omitted and the remaining characters in
    /// ASCII order. Fixed width and that monotonic alphabet make ordinary
    /// string comparison agree with mint time.
    ///
    /// # Errors
    ///
    /// Returns [`KeyMintError`] when `epoch_millis` is negative or will not
    /// fit in eight base-52 digits.
    pub fn for_millis(epoch_millis: i64) -> Result<Self, KeyMintError> {
        let value = u64::try_from(epoch_millis).map_err(|_| KeyMintError(epoch_millis))?;
        if value >= SPACE {
            return Err(KeyMintError(epoch_millis));
        }
        Ok(Self::encode(value))
    }

    /// Derive one stable human-sized seal from a durable identity.
    ///
    /// A lane inbox seal is a capability-derived recognition token, not an
    /// operator directive key. It must remain stable when the inbox is
    /// republished, so it deliberately does not use the moment codec above.
    #[must_use]
    pub fn for_identity(identity: CanonicalUuid) -> Self {
        let value = identity.get().as_u128() % u128::from(SPACE);
        Self::encode(value as u64)
    }

    fn encode(mut value: u64) -> Self {
        let mut bytes = [b'0'; Key::WIDTH];
        for byte in bytes.iter_mut().rev() {
            *byte = ALPHABET[(value % 52) as usize];
            value /= 52;
        }
        Self(bytes.into_iter().map(char::from).collect())
    }
}

impl FromStr for Key {
    type Err = KeyError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if Self::admitted(text) {
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

impl fmt::Display for KeyMintError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "epoch millisecond must fit in {} base-52 characters: {}",
            Key::WIDTH,
            self.0
        )
    }
}

impl Error for KeyMintError {}

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
pub const DISPOSITIONS: [(&str, Disposition); 6] = [
    ("live", Disposition::Live),
    ("acked", Disposition::Acked),
    ("declined", Disposition::Declined),
    ("lapsed", Disposition::Lapsed),
    ("deadlettered", Disposition::DeadLettered),
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

    #[test]
    fn recognition_holds_the_mint_alphabet_while_parsing_holds_every_mint() {
        // The two questions are asked of the same characters and must answer
        // differently. A vowel can only have come from the retired base-62
        // mint, so prose recognition should not volunteer it while parsing
        // still has to accept it: those keys are durable and replay reads them.
        for retired in ["1kJK6A3R", "1kJK6e3R", "YTVtQvOX", "QmTKOamJ", "SEobxPxT"] {
            assert!(
                Key::admitted(retired),
                "{retired:?} is a durable key parsing must accept"
            );
            assert!(
                retired.parse::<Key>().is_ok(),
                "{retired:?} did not parse through the public path"
            );
            assert!(
                !Key::shaped(retired),
                "{retired:?} is not shaped like anything the mint writes"
            );
        }

        // A key the current mint writes answers yes to both.
        let minted = Key::for_millis(1_700_000_000_000).expect("minted key");
        assert!(Key::shaped(minted.as_str()));
        assert!(Key::admitted(minted.as_str()));
    }

    #[test]
    fn an_identity_has_one_stable_seal() {
        let identity = "018f82d4-6f59-7d06-92e3-bce9131163c4"
            .parse()
            .expect("identity UUID");
        let key = Key::for_identity(identity);

        assert_eq!(key, Key::for_identity(identity));
        assert!(Key::shaped(key.as_str()));
    }

    #[test]
    fn keys_minted_one_second_apart_sort_in_mint_order() {
        let first = Key::for_millis(1_700_000_000_000).expect("first key");
        let second = Key::for_millis(1_700_000_001_000).expect("second key");

        assert_eq!(first.as_str(), "1dzGkSrJ");
        assert_eq!(second.as_str(), "1dzGkTCX");
        assert!(first.as_str() < second.as_str(), "{first} !< {second}");
    }

    #[test]
    fn the_moment_codec_is_fixed_width_vowel_free_and_bounded() {
        let first = Key::for_millis(0).expect("epoch key");
        let last = Key::for_millis(52_i64.pow(Key::WIDTH as u32) - 1).expect("last key");

        assert_eq!(first.as_str(), "00000000");
        assert_eq!(last.as_str(), "zzzzzzzz");
        assert!(Key::for_millis(-1).is_err());
        assert!(Key::for_millis(52_i64.pow(Key::WIDTH as u32)).is_err());

        // Two sample encodings only ever exercise the digits those two values
        // happen to land on, so every digit goes in every position below. What
        // the sweep compares against matters just as much: asking `Key::shaped`
        // about a minted key only asks the mint alphabet about itself, and a
        // vowel added to that constant would satisfy both sides at once. The
        // digits are written out here instead, so the mapping has to match a
        // literal that editing the constant cannot move.
        const DIGITS: &[u8; 52] = b"0123456789BCDFGHJKLMNPQRSTVWXYZbcdfghjklmnpqrstvwxyz";
        for position in 0..Key::WIDTH as u32 {
            for digit in 0..52_i64 {
                let key = Key::for_millis(digit * 52_i64.pow(position)).expect("swept key");
                let written = key.as_str().as_bytes()[Key::WIDTH - 1 - position as usize];
                assert_eq!(
                    written,
                    DIGITS[digit as usize],
                    "{key} spells digit {digit} at position {position} as {:?}",
                    char::from(written)
                );
            }
        }
    }

    #[test]
    fn the_mint_alphabet_ascends_in_ascii_so_string_order_is_mint_order() {
        // This is the property the whole scheme rests on: fixed width plus an
        // alphabet whose byte order matches its digit order is exactly what
        // makes plain string comparison agree with mint time. Sample keys imply
        // it; nothing else states it, so a reordering that kept the same
        // characters would slip past every other test here.
        assert!(
            super::ALPHABET.windows(2).all(|pair| pair[0] < pair[1]),
            "the mint alphabet is not ascending"
        );
        assert!(
            !super::ALPHABET
                .iter()
                .any(|byte| b"AEIOUaeiou".contains(byte)),
            "the mint alphabet carries a vowel"
        );
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
            Disposition::DeadLettered,
            Disposition::Canceled,
        ] {
            let named = match admitted {
                Disposition::Live => "live",
                Disposition::Acked => "acked",
                Disposition::Declined => "declined",
                Disposition::Lapsed => "lapsed",
                Disposition::DeadLettered => "deadlettered",
                Disposition::Canceled => "canceled",
            };
            assert_eq!(disposition(named), Ok(admitted));
        }
        assert_eq!(DISPOSITIONS.len(), 6);
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
