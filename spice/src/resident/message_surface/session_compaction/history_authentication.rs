//! Bounded content authentication for preserved-session history checkpoints.

#[cfg(test)]
use std::cell::RefCell;
use std::sync::{Arc, Mutex};

use super::*;

#[derive(Clone)]
pub(in crate::resident::message_surface) struct HistoryCheckpoint {
    frontier: Option<u64>,
    authentication: Arc<Mutex<HistoryAuthentication>>,
}

struct HistoryAuthentication {
    length: u64,
    state: HistoryAuthenticationState,
}

enum HistoryAuthenticationState {
    Building {
        stamp: FileStamp,
        offset: u64,
        observed: blake3::Hasher,
    },
    Ready {
        stamp: FileStamp,
        expected: blake3::Hash,
    },
    Checking {
        stamp: FileStamp,
        offset: u64,
        observed: blake3::Hasher,
        expected: blake3::Hash,
    },
    Fault(String),
}

#[derive(Debug)]
pub(super) struct HistoryAuthenticationStep {
    pub(super) stamp: Option<FileStamp>,
    pub(super) work: HistoryWork,
}

impl std::fmt::Debug for HistoryCheckpoint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HistoryCheckpoint")
            .field("next", &self.next())
            .finish_non_exhaustive()
    }
}

impl HistoryCheckpoint {
    pub(in crate::resident::message_surface) fn next(&self) -> u64 {
        let authenticated = self.authentication.lock().map_or(0, |authentication| {
            matches!(
                authentication.state,
                HistoryAuthenticationState::Ready { .. }
            )
            .then_some(authentication.length)
            .unwrap_or(0)
        });
        self.frontier
            .map_or(authenticated, |frontier| frontier.min(authenticated))
    }

    pub(in crate::resident::message_surface) fn retreat(&self, next: u64) -> Self {
        Self {
            frontier: Some(self.frontier.map_or(next, |frontier| frontier.min(next))),
            authentication: Arc::clone(&self.authentication),
        }
    }
}

pub(in crate::resident::message_surface) fn history_checkpoint(
    path: &PathBuf,
) -> io::Result<HistoryCheckpoint> {
    let source = FileStamp::read(&File::open(path)?)?;
    let state = if source.length == 0 {
        HistoryAuthenticationState::Ready {
            stamp: source,
            expected: blake3::hash(&[]),
        }
    } else {
        HistoryAuthenticationState::Building {
            stamp: source,
            offset: 0,
            observed: blake3::Hasher::new(),
        }
    };
    Ok(HistoryCheckpoint {
        frontier: None,
        authentication: Arc::new(Mutex::new(HistoryAuthentication {
            length: source.length,
            state,
        })),
    })
}

pub(super) fn advance_history_authentication(
    path: &PathBuf,
    checkpoint: &HistoryCheckpoint,
    started: Instant,
) -> io::Result<HistoryAuthenticationStep> {
    let mut authentication = checkpoint.authentication.lock().map_err(|_| {
        io::Error::other("preserved session history authentication lock is poisoned")
    })?;
    let state = std::mem::replace(
        &mut authentication.state,
        HistoryAuthenticationState::Fault(
            "preserved session history authentication was interrupted".to_owned(),
        ),
    );
    let result = advance_history_authentication_state(path, authentication.length, state, started);
    match result {
        Ok((state, step)) => {
            authentication.state = state;
            Ok(step)
        }
        Err(error) => {
            authentication.state = HistoryAuthenticationState::Fault(error.to_string());
            Err(error)
        }
    }
}

fn advance_history_authentication_state(
    path: &PathBuf,
    length: u64,
    state: HistoryAuthenticationState,
    started: Instant,
) -> io::Result<(HistoryAuthenticationState, HistoryAuthenticationStep)> {
    let mut file = File::open(path)?;
    let current = FileStamp::read(&file)?;
    match state {
        HistoryAuthenticationState::Ready { stamp, expected } if current == stamp => Ok((
            HistoryAuthenticationState::Ready { stamp, expected },
            HistoryAuthenticationStep {
                stamp: Some(stamp),
                work: HistoryWork::default(),
            },
        )),
        HistoryAuthenticationState::Ready { stamp, expected } => {
            require_history_extension(stamp, current, length)?;
            hash_history_authentication_slice(
                &mut file,
                length,
                HistoryAuthenticationState::Checking {
                    stamp: current,
                    offset: 0,
                    observed: blake3::Hasher::new(),
                    expected,
                },
                started,
            )
        }
        HistoryAuthenticationState::Building {
            stamp,
            offset,
            observed,
        } => {
            if current != stamp {
                return Err(history_source_changed(
                    "before its initial prefix authentication completed",
                ));
            }
            hash_history_authentication_slice(
                &mut file,
                length,
                HistoryAuthenticationState::Building {
                    stamp,
                    offset,
                    observed,
                },
                started,
            )
        }
        HistoryAuthenticationState::Checking {
            stamp,
            offset,
            observed,
            expected,
        } => {
            let state = if current == stamp {
                HistoryAuthenticationState::Checking {
                    stamp,
                    offset,
                    observed,
                    expected,
                }
            } else {
                require_history_extension(stamp, current, length)?;
                HistoryAuthenticationState::Checking {
                    stamp: current,
                    offset: 0,
                    observed: blake3::Hasher::new(),
                    expected,
                }
            };
            hash_history_authentication_slice(&mut file, length, state, started)
        }
        HistoryAuthenticationState::Fault(detail) => {
            Err(io::Error::new(io::ErrorKind::InvalidData, detail))
        }
    }
}

fn hash_history_authentication_slice(
    file: &mut File,
    length: u64,
    state: HistoryAuthenticationState,
    started: Instant,
) -> io::Result<(HistoryAuthenticationState, HistoryAuthenticationStep)> {
    let (stamp, offset, mut observed, expected) = match state {
        HistoryAuthenticationState::Building {
            stamp,
            offset,
            observed,
        } => (stamp, offset, observed, None),
        HistoryAuthenticationState::Checking {
            stamp,
            offset,
            observed,
            expected,
        } => (stamp, offset, observed, Some(expected)),
        _ => unreachable!("only pending authentication states read a slice"),
    };
    let count = length
        .saturating_sub(offset)
        .min(HISTORY_BYTE_BUDGET as u64);
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0; usize::try_from(count).expect("history budget fits usize")];
    file.read_exact(&mut bytes)?;
    #[cfg(test)]
    AFTER_HISTORY_AUTHENTICATION_READ.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
    let building = expected.is_none();
    observed.update(&bytes);
    let work = HistoryWork {
        source_bytes: count,
        decoded_records: 0,
        passes: 1,
        elapsed: started.elapsed(),
    };
    if !work.within_limits() {
        return Err(history_budget_exhausted(work));
    }
    let post = FileStamp::read(file)?;
    if post != stamp {
        require_history_extension(stamp, post, length)?;
        if building {
            return Err(history_source_changed(
                "during its initial prefix authentication",
            ));
        }
        return Ok((
            HistoryAuthenticationState::Checking {
                stamp: post,
                offset: 0,
                observed: blake3::Hasher::new(),
                expected: expected.expect("checked authentication has an expected digest"),
            },
            HistoryAuthenticationStep { stamp: None, work },
        ));
    }
    let next = offset.saturating_add(count);
    if next < length {
        let state = match expected {
            Some(expected) => HistoryAuthenticationState::Checking {
                stamp,
                offset: next,
                observed,
                expected,
            },
            None => HistoryAuthenticationState::Building {
                stamp,
                offset: next,
                observed,
            },
        };
        return Ok((state, HistoryAuthenticationStep { stamp: None, work }));
    }
    let digest = observed.finalize();
    if expected.is_some_and(|expected| expected != digest) {
        return Err(history_source_changed(
            "after its authenticated prefix was rewritten",
        ));
    }
    Ok((
        HistoryAuthenticationState::Ready {
            stamp,
            expected: digest,
        },
        HistoryAuthenticationStep { stamp: None, work },
    ))
}

pub(super) fn observe_history_source_movement(
    checkpoint: &HistoryCheckpoint,
    observed: FileStamp,
) -> io::Result<()> {
    let mut authentication = checkpoint.authentication.lock().map_err(|_| {
        io::Error::other("preserved session history authentication lock is poisoned")
    })?;
    let HistoryAuthenticationState::Ready { stamp, expected } = authentication.state else {
        return Ok(());
    };
    require_history_extension(stamp, observed, authentication.length)?;
    authentication.state = HistoryAuthenticationState::Checking {
        stamp: observed,
        offset: 0,
        observed: blake3::Hasher::new(),
        expected,
    };
    Ok(())
}

fn require_history_extension(
    previous: FileStamp,
    observed: FileStamp,
    length: u64,
) -> io::Result<()> {
    if previous.extension(observed, length).is_some() {
        Ok(())
    } else {
        Err(history_source_changed("after Canvas bootstrap"))
    }
}

fn history_source_changed(when: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("preserved session history source changed {when}"),
    )
}

#[cfg(test)]
thread_local! {
    pub(super) static AFTER_HISTORY_AUTHENTICATION_READ: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
}
