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
    let tag = domain.tag().as_bytes();
    let mut framed = Vec::with_capacity(tag.len() + 1 + bytes.len());
    framed.extend_from_slice(tag);
    framed.push(0);
    framed.extend_from_slice(bytes);
    let workers = thread::available_parallelism()
        .map_or(1, std::num::NonZero::get)
        .min(MAX_HASH_WORKERS);
    parallel_root(&framed, workers)
}

fn sequential(domain: Domain, bytes: &[u8]) -> Hash {
    let mut hasher = Hasher::new();
    hasher.update(domain.tag().as_bytes());
    hasher.update(&[0]);
    hasher.update(bytes);
    hasher.finalize()
}

fn parallel_root(input: &[u8], workers: usize) -> Hash {
    if workers <= 1 || input.len() <= CHUNK_LEN {
        return blake3::hash(input);
    }
    let left_len = left_subtree_len(input.len() as u64) as usize;
    let (left, right) = input.split_at(left_len);
    let (left_workers, right_workers) = divide_workers(workers);
    let (left, right) = thread::scope(|scope| {
        let left = scope.spawn(|| subtree(left, 0, left_workers));
        let right = subtree(right, left_len as u64, right_workers);
        (left.join().expect("scoped digest worker completes"), right)
    });
    merge_subtrees_root(&left, &right, Mode::Hash)
}

fn subtree(input: &[u8], offset: u64, workers: usize) -> ChainingValue {
    if workers <= 1 || input.len() <= CHUNK_LEN {
        let mut hasher = Hasher::new();
        hasher.set_input_offset(offset).update(input);
        return hasher.finalize_non_root();
    }
    let left_len = left_subtree_len(input.len() as u64) as usize;
    let (left, right) = input.split_at(left_len);
    let (left_workers, right_workers) = divide_workers(workers);
    let (left, right) = thread::scope(|scope| {
        let left = scope.spawn(|| subtree(left, offset, left_workers));
        let right = subtree(right, offset + left_len as u64, right_workers);
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
                assert_eq!(
                    parallel_root(&input, workers),
                    blake3::hash(&input),
                    "length={length} workers={workers}",
                );
            }
        }
    }
}
