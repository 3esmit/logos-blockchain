use blake2::Digest as _;
use cryptarchia_engine::Slot;
use groth16::{Fr, fr_from_bytes_unchecked, fr_to_bytes};
use nomos_core::{
    crypto::{ZkDigest, ZkHasher},
    utils::merkle::{MerkleNode, MerklePath},
};
use nomos_utils::blake_rng::{Blake2b256, BlakeRng256, BlakeRng256Seed, SeedableRng as _};
use rand::RngCore as _;
use rayon::{
    iter::{IntoParallelRefIterator as _, ParallelIterator as _},
    prelude::ParallelSlice as _,
};

use crate::pol::SlotSecret;

pub type CachedTree = Vec<Vec<Fr>>;

#[derive(Clone)]
pub struct MerklePolCache {
    pub cached_tree: CachedTree,
    pub lower_sub_trees: Vec<MerklePolSubtree>,
    pub tree_depth: usize,
    pub starting_slot: Slot,
}

impl MerklePolCache {
    #[must_use]
    pub fn new(
        seed: BlakeRng256Seed,
        starting_slot: Slot,
        tree_depth: usize,
        cache_depth: usize,
    ) -> Self {
        let sub_tree_depth = tree_depth - cache_depth;
        let number_of_subtrees = 2usize.pow(cache_depth as u32);
        // we have to pre-cache the leaves so we can parallelize the subtree computation
        let lower_sub_trees: Vec<_> = Self::leaves_from_seed(seed)
            .take(number_of_subtrees)
            .map(|seed| MerklePolSubtree::new(seed, sub_tree_depth))
            .collect();

        let lower_tree_roots: Vec<Fr> = lower_sub_trees
            .par_iter()
            .map(|subtree| {
                *subtree
                    .merkle_path_for_index(0)
                    .first()
                    .map(MerkleNode::item)
                    .expect("Root should be always present")
            })
            .collect();

        // compute the root slot secret
        let cached_tree = compute_cached_tree_from_leafs(&lower_tree_roots);

        Self {
            cached_tree,
            lower_sub_trees,
            tree_depth,
            starting_slot,
        }
    }

    pub fn leaves_from_seed(seed: BlakeRng256Seed) -> impl Iterator<Item = Fr> {
        let mut rng = BlakeRng256::from_seed(seed);
        std::iter::repeat_with(move || {
            let mut bytes = [0u8; 31];
            rng.fill_bytes(&mut bytes);
            fr_from_bytes_unchecked(&bytes)
        })
    }

    #[must_use]
    pub fn root_slot_secret(&self) -> SlotSecret {
        // this should always be there
        self.cached_tree[0][0].into()
    }

    #[must_use]
    pub const fn cache_depth(&self) -> usize {
        self.cached_tree.len()
    }

    #[must_use]
    pub fn merkle_path_for_slot(&self, slot: Slot) -> MerklePath<Fr> {
        let index = usize::try_from(slot.into_inner() - self.starting_slot.into_inner())
            .expect("Slot difference should always fit in usize");
        let mut cached_path = get_merkle_path(&self.cached_tree, index, self.cached_tree.len());
        let subtree_leaf_length = 2usize.pow((self.tree_depth - self.cache_depth()) as u32);
        let subtree_index = index / subtree_leaf_length;
        let leaf_index = index % subtree_leaf_length;
        let current_path = self.lower_sub_trees[subtree_index].merkle_path_for_index(leaf_index);
        cached_path.extend(current_path);
        cached_path
    }
}

#[derive(Clone)]
pub struct MerklePolSubtree {
    seed: Fr,
    tree_depth: usize,
}

impl MerklePolSubtree {
    #[must_use]
    pub const fn new(seed: Fr, tree_depth: usize) -> Self {
        Self { seed, tree_depth }
    }

    #[must_use]
    pub fn merkle_path_for_index(&self, index: usize) -> MerklePath<Fr> {
        let hashed_leafs: Vec<_> = std::iter::successors(Some(self.seed), |seed| {
            Some(fr_from_bytes_unchecked(
                &Blake2b256::digest(fr_to_bytes(seed))[..31],
            ))
        })
        .take(2usize.pow(self.tree_depth as u32))
        .collect();
        let merkle_tree = compute_cached_tree_from_leafs(&hashed_leafs);
        get_merkle_path(&merkle_tree, index, self.depth())
    }

    #[must_use]
    pub const fn depth(&self) -> usize {
        self.tree_depth
    }
}

fn compute_cached_tree_from_leafs(tree_leafs: &[Fr]) -> Vec<Vec<Fr>> {
    let mut cached_tree: Vec<Vec<Fr>> = std::iter::successors(Some(tree_leafs.to_vec()), |leafs| {
        if leafs.len() <= 1 {
            return None;
        }
        Some(
            leafs
                .par_chunks(2)
                .map(|pair| <[Fr; 2]>::try_from(pair).unwrap())
                .map(|pair| <ZkHasher as ZkDigest>::compress(&pair))
                .collect(),
        )
    })
    .collect();
    cached_tree.reverse();
    cached_tree
}

#[must_use]
pub fn get_merkle_path(
    cached_tree: &CachedTree,
    mut current_index: usize,
    tree_depth: usize,
) -> MerklePath<Fr> {
    let mut path = MerklePath::new();

    // Iterate from leaves level (tree_depth - 1) up to, but not including, the root
    // level (0)
    for level_idx in (1..tree_depth).rev() {
        // sibling is the adjacent index: flip the lowest bit
        let sibling_index = current_index ^ 1;

        if sibling_index < cached_tree[level_idx].len() {
            let sibling_value = cached_tree[level_idx][current_index];
            // Orientation is relative to the current node position
            let node = if current_index.is_multiple_of(2) {
                MerkleNode::Left(sibling_value)
            } else {
                MerkleNode::Right(sibling_value)
            };
            path.push(node);
        }

        // move to parent index for the next level up
        current_index /= 2;
    }
    path.reverse();
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper to build a small deterministic tree and verify paths
    fn build_cached_tree() -> CachedTree {
        // Build 4 leaves (tree depth = 3: levels 0=root, 1=internal, 2=leaves)
        let leaves: Vec<Fr> = vec![
            fr_from_bytes_unchecked(&[1u8; 31]),
            fr_from_bytes_unchecked(&[2u8; 31]),
            fr_from_bytes_unchecked(&[3u8; 31]),
            fr_from_bytes_unchecked(&[4u8; 31]),
        ];

        // Level 1 (internal)
        let level1 = vec![
            <ZkHasher as ZkDigest>::compress(&[leaves[0], leaves[1]]),
            <ZkHasher as ZkDigest>::compress(&[leaves[2], leaves[3]]),
        ];
        // Level 0 (root)
        let root = vec![<ZkHasher as ZkDigest>::compress(&[level1[0], level1[1]])];

        vec![root, level1, leaves]
    }

    #[test]
    fn compute_cached_tree_from_leafs_works() {
        let expected = build_cached_tree();
        let leaves: Vec<Fr> = vec![
            fr_from_bytes_unchecked(&[1u8; 31]),
            fr_from_bytes_unchecked(&[2u8; 31]),
            fr_from_bytes_unchecked(&[3u8; 31]),
            fr_from_bytes_unchecked(&[4u8; 31]),
        ];
        let cached_tree = compute_cached_tree_from_leafs(leaves.as_slice());
        // Sanity: expected layout sizes: level 0 -> 1, level 1 -> 2, level 2 -> 4
        assert_eq!(cached_tree.len(), 3);
        assert_eq!(cached_tree[0].len(), 1);
        assert_eq!(cached_tree[1].len(), 2);
        assert_eq!(cached_tree[2].len(), 4);
        assert_eq!(cached_tree, expected);
    }

    fn assert_node_match<T>(a: &MerkleNode<T>, b: &MerkleNode<T>) {
        assert!(matches!(
            (a, b),
            (MerkleNode::Left(_), MerkleNode::Left(_))
                | (MerkleNode::Right(_), MerkleNode::Right(_))
        ));
    }
    #[test]
    fn merkle_path_selects_correct_siblings_and_orientations() {
        let cached = build_cached_tree();
        let depth = 3usize;
        let l1 = &cached[1];
        let leaves = &cached[2];
        let expected_path = [
            vec![MerkleNode::Left(l1[0]), MerkleNode::Left(leaves[0])], // 1
            vec![MerkleNode::Left(l1[0]), MerkleNode::Right(leaves[1])], // 2
            vec![MerkleNode::Right(l1[1]), MerkleNode::Left(leaves[2])], // 3
            vec![MerkleNode::Right(l1[1]), MerkleNode::Right(leaves[3])],
        ];
        for i in 0..4 {
            let path = get_merkle_path(&cached, i, depth);
            let expected = &expected_path[i];
            assert_eq!(path.len(), expected.len());
            for (a, b) in path.iter().zip(expected) {
                assert_node_match(a, b);
                assert_eq!(*a.item(), *b.item());
            }
        }
    }
}
