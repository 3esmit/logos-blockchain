use std::sync::OnceLock;

use ark_ff::Field as _;
#[cfg(feature = "serde")]
use lb_groth16::serde::serde_fr;
use lb_poseidon2::{Digest, Fr};
use rpds::StackSync;

const EMPTY_VALUE: Fr = Fr::ZERO;
const ACCEPTABLE_MAX_HEIGHT: u8 = 32;

/// An append-only persistent Merkle Mountain Range (MMR), which can accept up
/// to 2^(`MAX_HEIGHT`-1) elements (leaves).
///
/// Compared to other merkle tree variants, this does not store leaves but
/// only the necessary internal nodes to update the root hash with new
/// additions. This makes it very space efficient, especially for large trees,
/// as we only need to store O(log n) nodes for n leaves.
///
/// Note on (de)serialization: serde will not preserve structural sharing since
/// it does not know which nodes are shared. This is ok if you only
/// (de)serialize one version of the tree, but if you dump multiple expect to
/// find multiple copes of the same nodes in the deserialized output. If you
/// need to preserve structural sharing, you should use a custom serialization.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone)]
pub struct MerkleMountainRange<T, Hash, const MAX_HEIGHT: u8 = ACCEPTABLE_MAX_HEIGHT> {
    roots: StackSync<Root>,
    #[cfg_attr(feature = "serde", serde(skip))]
    _hash: std::marker::PhantomData<(T, Hash)>,
}

impl<T, Hash, const MAX_HEIGHT: u8> PartialEq for MerkleMountainRange<T, Hash, MAX_HEIGHT> {
    fn eq(&self, other: &Self) -> bool {
        self.roots == other.roots
    }
}

impl<T, Hash, const MAX_HEIGHT: u8> Eq for MerkleMountainRange<T, Hash, MAX_HEIGHT> {}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Root {
    #[cfg_attr(feature = "serde", serde(with = "serde_fr"))]
    root: Fr,
    height: u8,
}

impl<const MAX_HEIGHT: u8, T, Hash> Default for MerkleMountainRange<T, Hash, MAX_HEIGHT>
where
    T: AsRef<Fr>,
    Hash: Digest,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<const MAX_HEIGHT: u8, T, Hash> MerkleMountainRange<T, Hash, MAX_HEIGHT>
where
    T: AsRef<Fr>,
    Hash: Digest,
{
    #[must_use]
    pub fn new() -> Self {
        assert!(
            MAX_HEIGHT <= ACCEPTABLE_MAX_HEIGHT,
            "MAX_HEIGHT must be <= {ACCEPTABLE_MAX_HEIGHT}"
        );
        Self {
            roots: StackSync::new_sync(),
            _hash: std::marker::PhantomData,
        }
    }

    pub fn push(&self, elem: T) -> Result<(Self, MerklePath), MmrFull> {
        if self.roots.peek().is_some_and(|r| r.height == MAX_HEIGHT) {
            return Err(MmrFull);
        }

        let index = self.len();
        let leaf_hash = Hash::digest(&[*elem.as_ref()]);
        let mut last_root = Root { root: leaf_hash, height: 1 };
        let mut roots = self.roots.clone();
        let mut siblings = Vec::with_capacity((MAX_HEIGHT - 1) as usize);

        // Cascade: merge with existing roots of the same height.
        // Each consumed root is a LEFT sibling of the pushed element.
        while let Some(root) = roots.peek().copied() {
            if last_root.height == root.height {
                roots.pop_mut();
                siblings.push(root.root);
                last_root = Root {
                    root: Hash::compress(&[root.root, last_root.root]),
                    height: last_root.height + 1,
                };
                assert!(
                    last_root.height <= MAX_HEIGHT,
                    "Height must be less than or equal to {MAX_HEIGHT}"
                );
            } else {
                break;
            }
        }

        // Remaining roots and empty subtrees form the higher-level siblings.
        // This mirrors the frontier_root logic.
        let mut height = last_root.height;
        for remaining in roots.iter() {
            while height < remaining.height {
                siblings.push(empty_subtree_root::<Hash>(height));
                height += 1;
            }
            siblings.push(remaining.root);
            height += 1;
        }
        while height < MAX_HEIGHT {
            siblings.push(empty_subtree_root::<Hash>(height));
            height += 1;
        }

        roots = roots.push(last_root);

        let mmr = Self {
            roots,
            _hash: std::marker::PhantomData,
        };
        let path = MerklePath { index, leaf_hash, siblings };
        Ok((mmr, path))
    }

    #[must_use]
    pub fn frontier_root(&self) -> Fr {
        let mut iter = self.roots.iter();

        let (mut root, mut height) = match iter.next() {
            Some(last) => (last.root, last.height),
            None => {
                // MMR is empty. Return the root of an entirely empty tree.
                return empty_subtree_root::<Hash>(MAX_HEIGHT);
            }
        };

        for last in iter {
            while height < last.height {
                root = Hash::compress(&[root, empty_subtree_root::<Hash>(height)]);
                height += 1;
            }
            root = Hash::compress(&[last.root, root]);
            height += 1;
        }

        assert!(height <= MAX_HEIGHT);
        while height < MAX_HEIGHT {
            root = Hash::compress(&[root, empty_subtree_root::<Hash>(height)]);
            height += 1;
        }
        assert_eq!(height, MAX_HEIGHT);

        root
    }

    #[must_use]
    pub const fn capacity(&self) -> usize {
        Self::num_leaves(MAX_HEIGHT)
    }

    const fn num_leaves(height: u8) -> usize {
        1 << (height - 1)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.roots.iter().map(|r| Self::num_leaves(r.height)).sum()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }
}

fn empty_subtree_root<Hash: Digest>(height: u8) -> Fr {
    static PRECOMPUTED_EMPTY_ROOTS: OnceLock<[Fr; ACCEPTABLE_MAX_HEIGHT as usize]> =
        OnceLock::new();
    assert!(
        (1..=ACCEPTABLE_MAX_HEIGHT).contains(&height),
        "Height:{height} must be in 1..={ACCEPTABLE_MAX_HEIGHT}"
    );
    PRECOMPUTED_EMPTY_ROOTS.get_or_init(|| {
        let mut hashes = [EMPTY_VALUE; ACCEPTABLE_MAX_HEIGHT as usize];
        for i in 1..ACCEPTABLE_MAX_HEIGHT as usize {
            hashes[i] = Hash::compress(&[hashes[i - 1], hashes[i - 1]]);
        }
        hashes
    })[(height - 1) as usize]
}

#[derive(Debug, thiserror::Error)]
#[error("MMR is full")]
pub struct MmrFull;

#[derive(Debug, Clone)]
pub struct MerklePath {
    pub index: usize,
    pub leaf_hash: Fr,
    pub siblings: Vec<Fr>,
}

impl MerklePath {
    pub fn compute_root<Hash: Digest>(&self) -> Fr {
        let mut hash = self.leaf_hash;
        for (k, sibling) in self.siblings.iter().enumerate() {
            if (self.index >> k) & 1 == 0 {
                hash = Hash::compress(&[hash, *sibling]);
            } else {
                hash = Hash::compress(&[*sibling, hash]);
            }
        }
        hash
    }

    pub fn update<Hash: Digest>(&mut self, other: &MerklePath) {
        let diff = self.index ^ other.index;
        assert!(diff != 0, "cannot update with the same index");
        let k = (usize::BITS - 1 - diff.leading_zeros()) as usize;

        let mut subtree_root = other.leaf_hash;
        for level in 0..k {
            if (other.index >> level) & 1 == 0 {
                subtree_root = Hash::compress(&[subtree_root, other.siblings[level]]);
            } else {
                subtree_root = Hash::compress(&[other.siblings[level], subtree_root]);
            }
        }
        self.siblings[k] = subtree_root;
    }
}

#[cfg(test)]
mod test {
    use ark_ff::PrimeField as _;
    use proptest_macro::property_test;

    use super::*;
    type ZkHasher = lb_poseidon2::Poseidon2Bn254Hasher;

    struct TestFr(Fr);
    impl AsRef<Fr> for TestFr {
        fn as_ref(&self) -> &Fr {
            &self.0
        }
    }

    impl From<&[u8]> for TestFr {
        fn from(value: &[u8]) -> Self {
            Self(b2p(value))
        }
    }

    // bytes to poseidon field element
    fn b2p(b: &[u8]) -> Fr {
        let mut repr = [0u8; 32];
        assert!(b.len() <= 32);
        let len = b.len().min(32);
        repr[..len].copy_from_slice(&b[..len]);
        Fr::from_le_bytes_mod_order(&repr)
    }

    pub fn leaf(data: &[u8]) -> Fr {
        ZkHasher::digest(&[b2p(data)])
    }

    #[test]
    #[expect(clippy::clone_on_copy, reason = "for the sake of the test")]
    fn test_empty_roots() {
        let mut root = Fr::ZERO;
        for i in 1..=32 {
            assert_eq!(root, empty_subtree_root::<ZkHasher>(i));
            root = <ZkHasher as Digest>::compress(&[root.clone(), root]);
        }
    }

    fn padded_leaves(elements: impl IntoIterator<Item = impl AsRef<[u8]>>, height: u8) -> Vec<Fr> {
        let mut leaves = elements
            .into_iter()
            .map(|e| leaf(e.as_ref()))
            .collect::<Vec<_>>();
        let pad = (1 << (height - 1) as usize) - leaves.len();
        leaves.extend(std::iter::repeat_n(EMPTY_VALUE, pad));
        leaves
    }

    fn root(elements: &[Fr]) -> Fr {
        let n = elements.len();
        assert!(n.is_power_of_two());
        let mut nodes = elements.to_vec();
        for h in (1..=n.ilog2()).rev() {
            for i in 0..2usize.pow(h - 1) {
                nodes[i] = <ZkHasher as Digest>::compress(&[nodes[i * 2], nodes[i * 2 + 1]]);
            }
        }

        nodes[0]
    }

    #[property_test]
    fn test_frontier_root_8(elems: Vec<[u8; 32]>) {
        let mut mmr = <MerkleMountainRange<TestFr, ZkHasher, 8>>::new();
        for elem in &elems {
            (mmr, _) = mmr.push(elem.as_ref().into()).unwrap();
        }
        assert_eq!(mmr.frontier_root(), root(&padded_leaves(elems, 8)));
    }

    #[ignore = "very slow"]
    #[property_test]
    fn test_frontier_root_16(elems: Vec<[u8; 32]>) {
        let mut mmr = <MerkleMountainRange<TestFr, ZkHasher, 16>>::new();
        for elem in &elems {
            (mmr, _) = mmr.push(elem.as_ref().into()).unwrap();
        }
        assert_eq!(mmr.frontier_root(), root(&padded_leaves(elems, 16)));
    }

    #[test]
    fn test_empty_tree() {
        let mmr = <MerkleMountainRange<TestFr, ZkHasher, 3>>::new();
        assert_eq!(mmr.len(), 0);
        assert!(mmr.is_empty());
        assert_eq!(mmr.frontier_root(), empty_subtree_root::<ZkHasher>(3));
    }

    #[test]
    #[expect(clippy::cognitive_complexity, reason = "test continuity")]
    fn test_mmr_push() {
        const HEIGHT: u8 = 3; // max 2^(3-1) = 4 leaves
        let mut mmr = <MerkleMountainRange<TestFr, ZkHasher, HEIGHT>>::new();
        assert_eq!(mmr.capacity(), 4);
        assert_eq!(mmr.len(), 0);
        let frontier_root0 = mmr.frontier_root();
        assert_eq!(frontier_root0, empty_subtree_root::<ZkHasher>(HEIGHT));

        let path0;
        (mmr, path0) = mmr.push(b"hello".as_ref().into()).unwrap();
        assert_eq!(mmr.len(), 1);
        assert_eq!(mmr.roots.size(), 1);
        assert_eq!(mmr.roots.peek().unwrap().height, 1);
        assert_eq!(mmr.roots.peek().unwrap().root, leaf(b"hello"));
        let frontier_root1 = mmr.frontier_root();
        assert_ne!(frontier_root1, frontier_root0);
        assert_eq!(path0.compute_root::<ZkHasher>(), frontier_root1);

        let path1;
        (mmr, path1) = mmr.push(b"world".as_ref().into()).unwrap();
        assert_eq!(mmr.len(), 2);
        assert_eq!(mmr.roots.size(), 1);
        assert_eq!(mmr.roots.peek().unwrap().height, 2);
        assert_eq!(
            mmr.roots.peek().unwrap().root,
            <ZkHasher as Digest>::compress(&[leaf(b"hello"), leaf(b"world")])
        );
        let frontier_root2 = mmr.frontier_root();
        assert_ne!(frontier_root2, frontier_root1);
        assert_eq!(path1.compute_root::<ZkHasher>(), frontier_root2);

        let path2;
        (mmr, path2) = mmr.push(b"!".as_ref().into()).unwrap();
        assert_eq!(mmr.len(), 3);
        assert_eq!(mmr.roots.size(), 2);
        let top_root = mmr.roots.iter().last().unwrap();
        assert_eq!(top_root.height, 2);
        assert_eq!(
            top_root.root,
            <ZkHasher as Digest>::compress(&[leaf(b"hello"), leaf(b"world")])
        );
        assert_eq!(mmr.roots.peek().unwrap().height, 1);
        assert_eq!(mmr.roots.peek().unwrap().root, leaf(b"!"));
        let frontier_root3 = mmr.frontier_root();
        assert_ne!(frontier_root3, frontier_root2);
        assert_eq!(path2.compute_root::<ZkHasher>(), frontier_root3);

        let path3;
        (mmr, path3) = mmr.push(b"!".as_ref().into()).unwrap();
        assert_eq!(mmr.len(), 4);
        assert_eq!(mmr.roots.size(), 1);
        assert_eq!(mmr.roots.peek().unwrap().height, 3);
        assert_eq!(
            mmr.roots.peek().unwrap().root,
            <ZkHasher as Digest>::compress(&[
                <ZkHasher as Digest>::compress(&[leaf(b"hello"), leaf(b"world")]),
                <ZkHasher as Digest>::compress(&[leaf(b"!"), leaf(b"!")])
            ])
        );
        let frontier_root4 = mmr.frontier_root();
        assert_ne!(frontier_root4, frontier_root3);
        assert_eq!(path3.compute_root::<ZkHasher>(), frontier_root4);

        assert!(matches!(
            mmr.push(b"already full".as_ref().into()),
            Err(MmrFull)
        ));
    }

    #[test]
    fn test_merkle_path_update() {
        const HEIGHT: u8 = 3;
        let mut mmr = <MerkleMountainRange<TestFr, ZkHasher, HEIGHT>>::new();

        // Push 3 elements, track path for element at index 2
        let (mmr2, _) = mmr.push(b"a".as_ref().into()).unwrap();
        let (mmr3, _) = mmr2.push(b"b".as_ref().into()).unwrap();
        let (mmr4, path2) = mmr3.push(b"c".as_ref().into()).unwrap();
        mmr = mmr4;

        // path2 should verify against current frontier root
        assert_eq!(path2.compute_root::<ZkHasher>(), mmr.frontier_root());

        // Push a 4th element
        let path3;
        (mmr, path3) = mmr.push(b"d".as_ref().into()).unwrap();

        // path2 is now stale
        assert_ne!(path2.compute_root::<ZkHasher>(), mmr.frontier_root());

        // Update path2 using path3
        let mut path2 = path2;
        path2.update::<ZkHasher>(&path3);
        assert_eq!(path2.compute_root::<ZkHasher>(), mmr.frontier_root());
    }

    #[test]
    fn test_merkle_path_update_all() {
        const HEIGHT: u8 = 4;
        type Mmr = MerkleMountainRange<TestFr, ZkHasher, HEIGHT>;
        let mut mmr = Mmr::new();
        let mut paths = Vec::new();

        let elems: &[&[u8]] = &[b"a", b"b", b"c", b"d", b"e", b"f", b"g", b"h"];
        for elem in elems {
            let path;
            (mmr, path) = mmr.push((*elem).into()).unwrap();

            // Update all previous paths with the new push proof
            for p in &mut paths {
                MerklePath::update::<ZkHasher>(p, &path);
            }

            // All paths should verify against the current root
            let frontier = mmr.frontier_root();
            for p in &paths {
                assert_eq!(p.compute_root::<ZkHasher>(), frontier);
            }
            assert_eq!(path.compute_root::<ZkHasher>(), frontier);

            paths.push(path);
        }
    }
}
