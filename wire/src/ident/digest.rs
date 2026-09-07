use std::thread;

use blake3::hazmat::{
    ChainingValue, HasherExt as _, Mode, left_subtree_len, merge_subtrees_non_root,
    merge_subtrees_root,
};
use blake3::{CHUNK_LEN, Hash, Hasher};

use super::Domain;

const PARALLEL_INPUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_HASH_WORKERS: usize = 16;

// Preserve the standard BLAKE3 tree and domain framing while distributing only
// the large executable inputs that dominate installed-hook authentication.
pub(super) fn compute(domain: Domain, bytes: &[u8]) -> Hash {
    if !matches!(domain, Domain::InstallArtifact | Domain::GateTool)
        || bytes.len() < PARALLEL_INPUT_BYTES
    {
        return sequential(domain, bytes);
    }
    let mut prefix = domain.tag().as_bytes().to_vec();
    prefix.push(0);
    let framed = FramedInput {
        prefix: &prefix,
        payload: bytes,
    };
    let workers = thread::available_parallelism()
        .map_or(1, std::num::NonZero::get)
        .min(MAX_HASH_WORKERS);
    parallel_root(framed, workers)
}

fn sequential(domain: Domain, bytes: &[u8]) -> Hash {
    let mut hasher = Hasher::new();
    hasher.update(domain.tag().as_bytes());
    hasher.update(&[0]);
    hasher.update(bytes);
    hasher.finalize()
}

#[derive(Clone, Copy)]
struct FramedInput<'a> {
    prefix: &'a [u8],
    payload: &'a [u8],
}

impl FramedInput<'_> {
    fn len(self) -> usize {
        self.prefix.len() + self.payload.len()
    }

    fn update(self, hasher: &mut Hasher, start: usize, length: usize) {
        let end = start + length;
        if start < self.prefix.len() {
            hasher.update(&self.prefix[start..end.min(self.prefix.len())]);
        }
        if end > self.prefix.len() {
            let payload_start = start.saturating_sub(self.prefix.len());
            let payload_end = end - self.prefix.len();
            hasher.update(&self.payload[payload_start..payload_end]);
        }
    }
}

fn parallel_root(input: FramedInput<'_>, workers: usize) -> Hash {
    if workers <= 1 || input.len() <= CHUNK_LEN {
        let mut hasher = Hasher::new();
        input.update(&mut hasher, 0, input.len());
        return hasher.finalize();
    }
    let left_len = left_subtree_len(input.len() as u64) as usize;
    let (left_workers, right_workers) = divide_workers(workers);
    let (left, right) = thread::scope(|scope| {
        let left = scope.spawn(|| subtree(input, 0, left_len, 0, left_workers));
        let right = subtree(
            input,
            left_len,
            input.len() - left_len,
            left_len as u64,
            right_workers,
        );
        (left.join().expect("scoped digest worker completes"), right)
    });
    merge_subtrees_root(&left, &right, Mode::Hash)
}

fn subtree(
    input: FramedInput<'_>,
    start: usize,
    length: usize,
    offset: u64,
    workers: usize,
) -> ChainingValue {
    if workers <= 1 || length <= CHUNK_LEN {
        let mut hasher = Hasher::new();
        hasher.set_input_offset(offset);
        input.update(&mut hasher, start, length);
        return hasher.finalize_non_root();
    }
    let left_len = left_subtree_len(length as u64) as usize;
    let (left_workers, right_workers) = divide_workers(workers);
    let (left, right) = thread::scope(|scope| {
        let left = scope.spawn(|| subtree(input, start, left_len, offset, left_workers));
        let right = subtree(
            input,
            start + left_len,
            length - left_len,
            offset + left_len as u64,
            right_workers,
        );
        (left.join().expect("scoped digest worker completes"), right)
    });
    merge_subtrees_non_root(&left, &right, Mode::Hash)
}

fn divide_workers(workers: usize) -> (usize, usize) {
    let left = workers / 2;
    (left, workers - left)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_tree_matches_blake3_for_irregular_boundaries() {
        for length in [
            CHUNK_LEN + 1,
            2 * CHUNK_LEN,
            2 * CHUNK_LEN + 1,
            17 * CHUNK_LEN - 1,
            17 * CHUNK_LEN,
            17 * CHUNK_LEN + 1,
            1024 * CHUNK_LEN + 37,
        ] {
            let input = vec![0x5a; length];
            for workers in 1..=MAX_HASH_WORKERS {
                let framed = FramedInput {
                    prefix: &input[..17.min(input.len())],
                    payload: &input[17.min(input.len())..],
                };
                assert_eq!(
                    parallel_root(framed, workers),
                    blake3::hash(&input),
                    "length={length} workers={workers}",
                );
            }
        }
    }
}
