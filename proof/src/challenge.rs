use std::fmt::Write as _;
#[cfg(unix)]
use std::fs;
use std::io;
#[cfg(unix)]
use std::io::Read as _;

const CHALLENGE_BYTES: usize = 32;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Challenge(String);

impl Challenge {
    pub(crate) fn fresh() -> io::Result<Self> {
        let mut bytes = [0_u8; CHALLENGE_BYTES];
        fill(&mut bytes)?;
        let mut challenge = String::with_capacity(CHALLENGE_BYTES * 2);
        for byte in bytes {
            write!(&mut challenge, "{byte:02x}").expect("writing to a String cannot fail");
        }
        Ok(Self(challenge))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    #[cfg(test)]
    pub(crate) fn fixture(challenge: &str) -> Self {
        Self(challenge.to_owned())
    }
}

#[cfg(unix)]
fn fill(bytes: &mut [u8]) -> io::Result<()> {
    fs::File::open("/dev/urandom")?.read_exact(bytes)
}

#[cfg(not(unix))]
fn fill(_bytes: &mut [u8]) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "sealed proof attestation requires Unix randomness",
    ))
}
