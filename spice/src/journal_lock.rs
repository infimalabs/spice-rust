use std::fs;
use std::io::{self, Seek, SeekFrom, Write};

use sill::{Acquisition, LockFile, LockName, Possession};
use spice::repository::Repository;

/// Exclusive resident-writer boundary held by offline journal maintenance.
pub(crate) struct JournalLock {
    possession: Possession,
}

impl JournalLock {
    pub(crate) fn acquire(repository: &Repository) -> Result<Self, Error> {
        let root = repository.state_root();
        fs::create_dir_all(&root).map_err(Error::Io)?;
        let name = LockName::new("resident").expect("resident is a valid fixed lock name");
        let lock = LockFile::new(name, root.join("resident"));
        match lock.try_acquire().map_err(Error::Io)? {
            Acquisition::Acquired(possession) => {
                let held = Self { possession };
                held.write_holder().map_err(Error::Io)?;
                Ok(held)
            }
            Acquisition::Contended => Err(Error::Contended),
        }
    }

    fn write_holder(&self) -> io::Result<()> {
        let file = self.possession.file();
        file.set_len(0)?;
        let mut writer = file;
        writer.seek(SeekFrom::Start(0))?;
        writeln!(
            writer,
            "pid={};operation=journal-maintenance",
            std::process::id()
        )?;
        writer.flush()?;
        file.sync_data()
    }
}

impl Drop for JournalLock {
    fn drop(&mut self) {
        let _ = self.possession.file().set_len(0);
        let _ = self.possession.file().sync_data();
    }
}

pub(crate) enum Error {
    Contended,
    Io(io::Error),
}
