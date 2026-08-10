//! Durable progress for the disposable journal-to-inbox fold.
//!
//! The journal is the source, the record in this module is progress, and the
//! lane inbox is the disposable output. The journal retains the full replay
//! horizon, startup and `spice steer repair LANE` are the refill paths, and a
//! parked record is the degraded answer when the output cannot be published.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use wire::intent::DeliveryStanding;
use wire::{CanonicalUuid, WireVersion};

const DIRECTORY: &str = "steering-fold";
const MAX_RECORD_BYTES: u64 = 64 * 1024;

/// One lane's admitted fold progress.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    v: WireVersion,
    lane: CanonicalUuid,
    delivery: DeliveryStanding,
}

impl Record {
    /// Bind one observable delivery standing to its lane.
    #[must_use]
    pub const fn new(lane: CanonicalUuid, delivery: DeliveryStanding) -> Self {
        Self {
            v: WireVersion::CURRENT,
            lane,
            delivery,
        }
    }

    /// Current delivery standing carried by this progress record.
    #[must_use]
    pub const fn delivery(&self) -> &DeliveryStanding {
        &self.delivery
    }
}

/// Root of the fold-progress family beneath durable repository data.
#[must_use]
pub fn root(data: &Path) -> PathBuf {
    data.join(DIRECTORY)
}

/// Exact progress-record path for one lane.
#[must_use]
pub fn path(data: &Path, lane: CanonicalUuid) -> PathBuf {
    root(data).join(lane.to_string())
}

/// Read and validate one lane's progress without creating or repairing it.
///
/// # Errors
///
/// Returns an I/O, size, wire, version, lane, or standing error rather than
/// interpreting unusable evidence as no progress.
pub fn read(data: &Path, lane: CanonicalUuid) -> Result<Option<Record>, Error> {
    let path = path(data, lane);
    let file = match fs::File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(Error::Read(error)),
    };
    let metadata = file.metadata().map_err(Error::Read)?;
    if !metadata.is_file() {
        return Err(Error::Invalid(
            "progress path is not a regular file".to_owned(),
        ));
    }
    if metadata.len() > MAX_RECORD_BYTES {
        return Err(Error::Invalid(format!(
            "progress record is {} bytes, above the {MAX_RECORD_BYTES}-byte bound",
            metadata.len()
        )));
    }
    let capacity = usize::try_from(metadata.len())
        .map_err(|_| Error::Invalid("progress size exceeds platform capacity".to_owned()))?;
    let mut bytes = Vec::with_capacity(capacity);
    file.take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(Error::Read)?;
    if bytes.len() > capacity {
        return Err(Error::Invalid(
            "progress record grew while it was being read".to_owned(),
        ));
    }
    let record: Record = wire::decode(&bytes).map_err(|error| Error::Wire(error.to_string()))?;
    if !record.v.is_current() {
        return Err(Error::Invalid(format!(
            "progress record carries wire version {}",
            record.v.0
        )));
    }
    if record.lane != lane {
        return Err(Error::Invalid(format!(
            "progress record names lane {}, not {lane}",
            record.lane
        )));
    }
    if matches!(record.delivery, DeliveryStanding::Unknown) {
        return Err(Error::Invalid(
            "durable progress cannot record unknown standing".to_owned(),
        ));
    }
    Ok(Some(record))
}

/// Atomically replace one lane's admitted progress record.
///
/// # Errors
///
/// Returns a directory, wire, or atomic-publication error. The caller must not
/// claim a parked failure or successful publication unless this write lands.
pub fn publish(data: &Path, record: &Record) -> Result<(), Error> {
    let root = root(data);
    fs::create_dir_all(&root).map_err(Error::Directory)?;
    let bytes = wire::encode(record).map_err(|error| Error::Wire(error.to_string()))?;
    let destination = atomic::Destination::new(root.join(record.lane.to_string()))?;
    destination
        .publish(|staged| staged.write_all(&bytes))
        .map_err(Error::Publication)
}

/// Why progress could not be observed or durably replaced.
#[derive(Debug)]
pub enum Error {
    /// The progress family directory could not be created.
    Directory(std::io::Error),
    /// The current record could not be read.
    Read(std::io::Error),
    /// The current record did not cross the canonical wire codec.
    Wire(String),
    /// The current record contradicted the selected lane or record contract.
    Invalid(String),
    /// Atomic publication could not be prepared.
    Destination(atomic::Error),
    /// The complete staged record could not replace the prior one.
    Publication(atomic::Error),
}

impl From<atomic::Error> for Error {
    fn from(error: atomic::Error) -> Self {
        Self::Destination(error)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Directory(error) => write!(formatter, "fold directory: {error}"),
            Self::Read(error) => write!(formatter, "fold record: {error}"),
            Self::Wire(error) => write!(formatter, "fold wire: {error}"),
            Self::Invalid(detail) => write!(formatter, "fold record: {detail}"),
            Self::Destination(error) | Self::Publication(error) => {
                write!(formatter, "fold publication: {error}")
            }
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;
    use wire::Seq;

    fn lane(last: u8) -> CanonicalUuid {
        format!("018f82d4-6f59-7d06-92e3-bce9131163{last:02}")
            .parse()
            .expect("fixture lane")
    }

    fn root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("steer-fold-{name}-{}", std::process::id()))
    }

    #[test]
    fn published_and_parked_progress_replace_atomically() {
        let data = root("replace");
        let _ = fs::remove_dir_all(&data);
        let lane = lane(1);
        let published = Record::new(
            lane,
            DeliveryStanding::Published {
                source: Seq::new(3).expect("source"),
            },
        );
        publish(&data, &published).expect("published progress lands");
        assert_eq!(read(&data, lane).expect("read"), Some(published));

        let parked = Record::new(
            lane,
            DeliveryStanding::Parked {
                source: Seq::new(5).expect("source"),
                keys: vec!["1kJK6M0V".to_owned()],
                failure: "inbox directory: blocked".to_owned(),
                repair: format!("spice steer repair {lane}"),
            },
        );
        publish(&data, &parked).expect("parked progress replaces published");
        assert_eq!(read(&data, lane).expect("read"), Some(parked));
        fs::remove_dir_all(data).expect("fixture cleanup");
    }

    #[test]
    fn missing_is_unknown_but_corruption_and_lane_mismatch_are_loud() {
        let data = root("refusal");
        let _ = fs::remove_dir_all(&data);
        let selected = lane(2);
        assert_eq!(read(&data, selected).expect("missing is observable"), None);

        let other = lane(3);
        let mismatched = Record::new(
            other,
            DeliveryStanding::Published {
                source: Seq::new(7).expect("source"),
            },
        );
        fs::create_dir_all(super::root(&data)).expect("fold root");
        fs::write(
            path(&data, selected),
            wire::encode(&mismatched).expect("wire"),
        )
        .expect("mismatch fixture");
        assert!(
            read(&data, selected)
                .unwrap_err()
                .to_string()
                .contains("not")
        );

        fs::write(path(&data, selected), b"not-json").expect("corrupt fixture");
        assert!(
            read(&data, selected)
                .unwrap_err()
                .to_string()
                .contains("fold wire")
        );
        fs::remove_dir_all(data).expect("fixture cleanup");
    }
}
