use blake2::Digest as _;
use cryptarchia_engine::Slot;
use groth16::{Fr, GROTH16_SAFE_BYTES_SIZE, fr_from_bytes_unchecked, fr_to_bytes};
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
pub struct CachedPoLMerkleTree {
    pub cached_tree: CachedTree,
    pub lower_sub_trees: Vec<PolMerkleSubtree>,
    pub tree_depth: usize,
    pub starting_slot: Slot,
}

impl CachedPoLMerkleTree {
    #[must_use]
    pub fn new(
        seed: BlakeRng256Seed,
        starting_slot: Slot,
        tree_depth: usize,
        cache_depth: usize,
    ) -> Self {
        let sub_tree_depth = tree_depth.saturating_sub(cache_depth);
        let number_of_subtrees = 2usize.pow(cache_depth as u32);
        // we have to pre-cache the leaves so we can parallelize the subtree computation
        let lower_sub_trees: Vec<_> = Self::leaves_from_seed(seed)
            .take(number_of_subtrees)
            .map(|seed| PolMerkleSubtree::new(seed, sub_tree_depth))
            .collect();

        // get the roots for the subtrees
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

    /// Compute the leaves (seeds of the subtrees) from a seed
    pub fn leaves_from_seed(seed: BlakeRng256Seed) -> impl Iterator<Item = Fr> {
        let mut rng = BlakeRng256::from_seed(seed);
        std::iter::repeat_with(move || {
            let mut bytes = [0u8; GROTH16_SAFE_BYTES_SIZE];
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
        self.cached_tree.len().saturating_sub(1)
    }

    /// Compute the location of a slot secret in reference to the internal
    /// location. # Returns
    /// 1) The index of the slot in the internal tree.
    /// 2) The index of the subtree in the internal tree.
    /// 3) The index of the leaf in the subtree.
    fn indexes_from_slot(&self, slot: Slot) -> (usize, usize, usize) {
        debug_assert!(
            slot >= self.starting_slot,
            "Slots should not be checked for the past"
        );
        let index = usize::try_from(
            slot.into_inner()
                .saturating_sub(self.starting_slot.into_inner()),
        )
        .expect("Slot difference should always fit in usize");
        let subtree_leaf_length = 2usize.pow((self.tree_depth - self.cache_depth()) as u32);
        let subtree_index = index / subtree_leaf_length;
        let leaf_index = index % subtree_leaf_length;
        (index, subtree_index, leaf_index)
    }

    /// Generates the Merkle path for a given `slot`, **it does not contain the
    /// root**.
    #[must_use]
    pub fn merkle_path_for_slot(&self, slot: Slot) -> MerklePath<Fr> {
        let (index, subtree_index, leaf_index) = self.indexes_from_slot(slot);
        let mut cached_path = get_merkle_path(&self.cached_tree, index, self.cached_tree.len());
        let current_path = self.lower_sub_trees[subtree_index].merkle_path_for_index(leaf_index);
        cached_path.extend(current_path);
        cached_path
    }

    #[must_use]
    pub fn slot_secret_for_slot(&self, slot: Slot) -> Fr {
        let (_, subtree_index, leaf_index) = self.indexes_from_slot(slot);
        self.lower_sub_trees[subtree_index].slot_secret_for_index(leaf_index)
    }
}

/// Represents a partial Merkle subtree.
///
/// This structure encapsulates the initial seed and the depth of the subtree,
/// enabling the re-computation of the subtree's leafs secrets.
#[derive(Clone)]
pub struct PolMerkleSubtree {
    /// A field element (`Fr`) that serves as the initial seed for constructing
    /// the Merkle subtree
    seed: Fr,
    /// The depth of the subtree, i.e. the number of levels in the tree
    tree_depth: usize,
}

impl PolMerkleSubtree {
    #[must_use]
    pub const fn new(seed: Fr, tree_depth: usize) -> Self {
        Self { seed, tree_depth }
    }

    /// Generates a Merkle path for a given leaf index in the Merkle tree.
    ///
    /// # Arguments
    ///
    /// * `index` - The index of the leaf node for which the Merkle path is to
    ///   be generated.
    ///
    /// # Returns
    ///
    /// A `MerklePath` object containing the path from the specified leaf index
    /// to the root of the Merkle tree.
    #[must_use]
    pub fn merkle_path_for_index(&self, index: usize) -> MerklePath<Fr> {
        let hashed_leafs = self.compute_leafs();
        let merkle_tree = compute_cached_tree_from_leafs(&hashed_leafs);
        get_merkle_path(&merkle_tree, index, self.depth())
    }

    /// Computes the leaf nodes of a Merkle tree using a pseudorandom hashing
    /// process.
    ///
    /// This function generates leaf nodes based on the `seed` initialized in
    /// the struct and iteratively hashes it to produce a sequence of
    /// pseudorandom values. The hashing process employs the `Blake2b256`
    /// algorithm and constrains the output size to `GROTH16_SAFE_BYTES_SIZE` so
    /// it fits in a `Fr`.
    ///
    /// # Returns
    /// A `Vec<Fr>` containing all the computed leaf nodes of the Merkle tree.
    /// The number of leaf nodes is determined by the tree depth:
    /// `2^(tree_depth)`.
    fn compute_leafs(&self) -> Vec<Fr> {
        let hashed_leafs: Vec<_> = std::iter::successors(Some(self.seed), |seed| {
            Some(fr_from_bytes_unchecked(
                &Blake2b256::digest(fr_to_bytes(seed))[..GROTH16_SAFE_BYTES_SIZE],
            ))
        })
        .take(2usize.pow(self.tree_depth as u32))
        .collect();
        hashed_leafs
    }

    /// Retrieves the secret associated with the specified slot index.
    ///
    /// This function computes the hashed leaf nodes of the corresponding data
    /// and returns the secret value at the given `index`.
    ///
    /// # Parameters
    /// - `index`: The position of the slot for which the secret is needed. Must
    ///   be within the bounds of the computed hashed leaf nodes array.
    ///
    /// # Returns
    /// - `Fr`: The secret value located at the specified index.
    ///
    /// # Panics
    /// This function will panic if `index` is out of bounds of the
    /// `hashed_leafs` array.
    #[must_use]
    pub fn slot_secret_for_index(&self, index: usize) -> Fr {
        let hashed_leafs = self.compute_leafs();
        hashed_leafs[index]
    }

    #[must_use]
    pub const fn depth(&self) -> usize {
        self.tree_depth
    }
}

fn compute_cached_tree_from_leafs(tree_leafs: &[Fr]) -> Vec<Vec<Fr>> {
    // reduce a tree from the leafs hashes
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
        assert!(
            sibling_index < cached_tree[level_idx].len(),
            "Tree should be constructed properly"
        );
        let sibling_value = cached_tree[level_idx][sibling_index];
        // Orientation is relative to the current node position
        let node = if sibling_index.is_multiple_of(2) {
            MerkleNode::Left(sibling_value)
        } else {
            MerkleNode::Right(sibling_value)
        };
        path.push(node);

        // move to index for the next level up
        current_index /= 2;
    }
    path.reverse();
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get_test_seed() -> BlakeRng256Seed {
        BlakeRng256Seed::from([42u8; 32])
    }

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
            vec![MerkleNode::Right(l1[1]), MerkleNode::Right(leaves[1])], // 1
            vec![MerkleNode::Right(l1[1]), MerkleNode::Left(leaves[0])],  // 2
            vec![MerkleNode::Left(l1[0]), MerkleNode::Right(leaves[3])],  // 3
            vec![MerkleNode::Left(l1[0]), MerkleNode::Left(leaves[2])],
        ];
        for (i, expected) in expected_path.iter().enumerate() {
            let path = get_merkle_path(&cached, i, depth);
            assert_eq!(path.len(), expected.len());
            for (a, b) in path.iter().zip(expected) {
                assert_node_match(a, b);
                assert_eq!(*a.item(), *b.item());
            }
        }
    }

    #[test]
    fn cache_depth_is_correct() {
        let tree_depth = 4;
        let cache_depth = 2;
        let merkle_cache =
            CachedPoLMerkleTree::new(get_test_seed(), Slot::new(0), tree_depth, cache_depth);
        assert_eq!(merkle_cache.cache_depth(), cache_depth);
    }

    #[test]
    fn slot_secret_for_slot_is_correct() {
        let tree_depth = 4;
        let cache_depth = 2;
        let merkle_cache =
            CachedPoLMerkleTree::new(get_test_seed(), Slot::new(0), tree_depth, cache_depth);

        for slot_number in 0..16 {
            let slot = Slot::new(slot_number);
            // Get slot secret for slot
            let slot_secret = merkle_cache.slot_secret_for_slot(slot);

            // Calculate expected values
            let subtree_leaf_length = 2usize.pow((tree_depth - cache_depth) as u32);
            let expected_subtree_index = slot_number as usize / subtree_leaf_length;
            let expected_leaf_index = slot_number as usize % subtree_leaf_length;

            // Get expected slot secret
            let expected = merkle_cache.lower_sub_trees[expected_subtree_index]
                .slot_secret_for_index(expected_leaf_index);

            assert_eq!(slot_secret, expected, "Failed for slot {slot_number}");
        }
    }
}
