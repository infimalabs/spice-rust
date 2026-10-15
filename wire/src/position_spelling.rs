//! The shortest vowel-free base-52 spelling of one nonzero journal position:
//! the text every identity derived from the fact recorded there shares.

use std::fmt;

use crate::envelope::Seq;
use crate::witness::TASK_STAMP_ALPHABET;

/// Base-52 digits needed to spell `u64::MAX`.
const MAX_DIGITS: usize = 12;

/// Why text spells no canonical nonzero journal position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Misspelling {
    /// No digits were supplied.
    Empty,
    /// A character lies outside the vowel-free base-52 alphabet.
    Alphabet,
    /// The zero digit names no journal position.
    Zero,
    /// A leading zero digit makes the spelling noncanonical.
    LeadingZero,
    /// The spelled value exceeds every journal position.
    Overflow,
}

/// The position `text` spells canonically.
pub(crate) fn parse(text: &str) -> Result<Seq, Misspelling> {
    let mut value = 0_u64;
    let mut overflow = false;
    for byte in text.bytes() {
        let Some(digit) = TASK_STAMP_ALPHABET
            .iter()
            .position(|candidate| *candidate == byte)
        else {
            return Err(Misspelling::Alphabet);
        };
        match value
            .checked_mul(52)
            .and_then(|value| value.checked_add(digit as u64))
        {
            Some(next) => value = next,
            None => overflow = true,
        }
    }
    match text.as_bytes() {
        [] => Err(Misspelling::Empty),
        [b'0'] => Err(Misspelling::Zero),
        [b'0', ..] => Err(Misspelling::LeadingZero),
        _ if overflow => Err(Misspelling::Overflow),
        _ => Ok(Seq::new(value).expect("a canonical nonempty spelling names a nonzero position")),
    }
}

/// Write `position` in its one canonical spelling.
pub(crate) fn write(position: Seq, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
    let mut digits = [0; MAX_DIGITS];
    let mut start = MAX_DIGITS;
    let mut value = position.get();
    while value > 0 {
        start -= 1;
        digits[start] = TASK_STAMP_ALPHABET[(value % 52) as usize];
        value /= 52;
    }
    formatter
        .write_str(std::str::from_utf8(&digits[start..]).expect("the base-52 alphabet is ASCII"))
}
