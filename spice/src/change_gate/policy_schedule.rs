//! Deterministic bounded execution for independent focused policy arms.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

pub(super) enum Outcome<T, E> {
    Returned(Result<T, E>),
    Panicked,
}

pub(super) struct Completion<T, E> {
    pub(super) index: usize,
    pub(super) elapsed: Duration,
    pub(super) outcome: Outcome<T, E>,
}

/// Prepare in input order and execute at most `concurrency` inputs at once.
///
/// A failed preparation, execution, or worker panic removes pending positions
/// after that canonical index. Earlier positions still run, active work is
/// joined, and every started result is returned in canonical input order.
pub(super) fn run<I, W, T, E>(
    inputs: Vec<(usize, I)>,
    concurrency: usize,
    mut prepare: impl FnMut(I) -> Result<W, E>,
    execute: impl Fn(W) -> Result<T, E> + Sync,
    mut observe: impl FnMut(usize, Duration, &Outcome<T, E>) -> Result<(), E>,
) -> Vec<Completion<T, E>>
where
    I: Send,
    W: Send,
    T: Send,
    E: Send,
{
    let limit = concurrency.max(1);
    let mut pending = inputs.into_iter().collect::<VecDeque<_>>();
    let mut completions = Vec::new();
    let mut failure_cutoff = None;

    thread::scope(|scope| {
        let (sender, receiver) = mpsc::channel();
        let execute = &execute;
        let mut active = 0_usize;

        while active > 0 || !pending.is_empty() {
            while active < limit {
                let Some((index, input)) = pending.pop_front() else {
                    break;
                };
                let started = Instant::now();
                let work = match prepare(input) {
                    Ok(work) => work,
                    Err(error) => {
                        let mut completion = Completion {
                            index,
                            elapsed: started.elapsed(),
                            outcome: Outcome::Returned(Err(error)),
                        };
                        if let Err(error) =
                            observe(completion.index, completion.elapsed, &completion.outcome)
                        {
                            completion.outcome = Outcome::Returned(Err(error));
                        }
                        completions.push(completion);
                        lower_failure_cutoff(&mut failure_cutoff, &mut pending, index);
                        break;
                    }
                };
                let sender = sender.clone();
                scope.spawn(move || {
                    let outcome = catch_unwind(AssertUnwindSafe(|| execute(work)))
                        .map_or(Outcome::Panicked, Outcome::Returned);
                    let completion = Completion {
                        index,
                        elapsed: started.elapsed(),
                        outcome,
                    };
                    let _ = sender.send(completion);
                });
                active += 1;
            }

            if active == 0 {
                break;
            }
            let mut completion = receiver
                .recv()
                .expect("each bounded policy worker returns one completion");
            active -= 1;
            if let Err(error) = observe(completion.index, completion.elapsed, &completion.outcome) {
                completion.outcome = Outcome::Returned(Err(error));
            }
            if !matches!(completion.outcome, Outcome::Returned(Ok(_))) {
                lower_failure_cutoff(&mut failure_cutoff, &mut pending, completion.index);
            }
            completions.push(completion);
        }
    });

    completions.sort_by_key(|completion| completion.index);
    completions
}

fn lower_failure_cutoff<I>(
    cutoff: &mut Option<usize>,
    pending: &mut VecDeque<(usize, I)>,
    failed: usize,
) {
    *cutoff = Some(cutoff.map_or(failed, |current| current.min(failed)));
    let cutoff = cutoff.expect("the failure cutoff was assigned above");
    pending.retain(|(index, _)| *index < cutoff);
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier, Mutex};

    use super::*;

    #[test]
    fn bounded_workers_overlap_without_crossing_the_limit_and_return_canonically() {
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let initial = Arc::new(Barrier::new(3));
        let completions = run(
            (0_usize..8).map(|index| (index, index)).collect(),
            3,
            Ok::<_, ()>,
            {
                let active = Arc::clone(&active);
                let maximum = Arc::clone(&maximum);
                let initial = Arc::clone(&initial);
                move |input| {
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    maximum.fetch_max(now, Ordering::SeqCst);
                    if input < 3 {
                        initial.wait();
                    }
                    thread::sleep(Duration::from_millis((8 - input) as u64));
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok(input)
                }
            },
            |_, _, _| Ok(()),
        );

        assert_eq!(maximum.load(Ordering::SeqCst), 3);
        assert_eq!(completions.len(), 8);
        for (expected, completion) in completions.into_iter().enumerate() {
            assert_eq!(completion.index, expected);
            let Outcome::Returned(Ok(value)) = completion.outcome else {
                panic!("bounded fixture must complete normally");
            };
            assert_eq!(value, expected);
        }
    }

    #[test]
    fn failure_dispatches_only_earlier_canonical_work_and_returns_the_first() {
        let started = Arc::new(Mutex::new(Vec::new()));
        let completions = run(
            [2_usize, 0, 1, 3, 4, 5]
                .into_iter()
                .map(|index| (index, index))
                .collect(),
            2,
            Ok::<_, usize>,
            {
                let started = Arc::clone(&started);
                move |input| {
                    started.lock().expect("started fixture lock").push(input);
                    match input {
                        1 => {
                            thread::sleep(Duration::from_millis(20));
                            Err(input)
                        }
                        2 => {
                            thread::sleep(Duration::from_millis(5));
                            Err(input)
                        }
                        _ => {
                            thread::sleep(Duration::from_millis(30));
                            Ok(input)
                        }
                    }
                }
            },
            |_, _, _| Ok(()),
        );

        let mut started = started.lock().expect("started fixture lock").clone();
        started.sort_unstable();
        assert_eq!(started, [0, 1, 2]);
        let first_failure = completions.into_iter().find_map(|completion| {
            let Outcome::Returned(Err(error)) = completion.outcome else {
                return None;
            };
            Some((completion.index, error))
        });
        assert_eq!(first_failure, Some((1, 1)));
    }
}
