//! Committing bytes before anything interprets them.
//!
//! The append is synchronized before the locus is returned, and so is the
//! directory when the append created the member: bytes committed under a name
//! that is not yet committed are bytes a crash can leave unreachable. A caller
//! that holds a locus therefore holds a promise the filesystem has already
//! made, which is what lets every decoder downstream be allowed to fail:
//! whatever it could not understand is still on disk, byte for byte, at a
//! position that outlives the process that captured it.

use std::io::Write;

use crate::{Error, Locus, Operation, Tape, open_member};

impl Tape {
    /// Append exact bytes and return the locus naming them.
    ///
    /// The bytes are written and synchronized before this returns, and so is
    /// the directory entry when this append is the one that created the
    /// member. Nothing inspects, normalizes, or re-frames the bytes: a tee
    /// that understood its input would be a decoder, and a decoder that failed
    /// would take the capture down with it.
    ///
    /// # Errors
    ///
    /// Refuses when another writer holds the tape. Returns an I/O failure when
    /// the member cannot be opened, extended, or synchronized, or when the
    /// directory holding a newly created member cannot be synchronized. No
    /// locus is returned in any of those cases.
    pub fn tee(&self, bytes: &[u8]) -> Result<Locus, Error> {
        let _possession = self.possess()?;
        let index = self.read_index()?;
        self.recover(&index)?;

        let member = index.next_member();
        let path = self.plain_path(member);
        let (mut file, created) =
            open_member(&path).map_err(|error| Error::io(Operation::Append, &path, error))?;

        // Under the writer lock the end of the member is the offset this
        // append will occupy, so the locus is known before the write.
        let offset = file
            .metadata()
            .map_err(|error| Error::io(Operation::Append, &path, error))?
            .len();

        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| Error::io(Operation::Append, path, error))?;

        // An append that created the member left the directory holding a name
        // that is not yet durable. Committing it here rather than on every
        // append keeps the cost on the one append that incurred it, and the
        // failure is returned rather than swallowed: a locus this call did not
        // return is a locus nothing downstream can be holding.
        if created {
            self.sync_directory()?;
        }

        Ok(Locus::new(member, offset, bytes.len() as u64))
    }
}
