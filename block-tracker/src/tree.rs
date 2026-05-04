use std::{
    collections::{HashMap, HashSet},
    hash::Hash,
};

use crate::BlockView;

/// A block reported by [`BlockTree::advance_lib`] as newly finalized.
///
/// The `input` is moved out of the tree at finalization time — once a
/// block is below LIB, the tree no longer needs its per-block data.
/// Consumers that want to materialize "finalized" effects (e.g. mark
/// rewards as spendable) read these.
pub struct PrunedBlock<Id, Input> {
    pub id: Id,
    pub input: Input,
}

/// Per-branch state tracker over a block tree.
///
/// Stores per-block accumulated state and per-block input along every
/// branch. Reorgs are handled by switching to a different tip;
/// [`Self::find_lcm`] gives the common ancestor for diffing.
///
/// As LIB advances, the chain segment below the new LIB is finalized and
/// returned to the caller; sibling branches of that segment are pruned.
pub struct BlockTree<Id, V: BlockView> {
    states: HashMap<Id, V::State>,
    inputs: HashMap<Id, V::Input>,
    parent_map: HashMap<Id, Id>,
    lib: Id,
}

impl<Id, V> BlockTree<Id, V>
where
    Id: Clone + Eq + Hash,
    V: BlockView,
{
    /// Create a new tree rooted at `genesis` with default state.
    ///
    /// `genesis` becomes the initial LIB. It has no parent in `parent_map`
    /// and no input in `inputs` (it's the base case for the fold).
    pub fn new(genesis: Id) -> Self {
        let mut states = HashMap::new();
        states.insert(genesis.clone(), V::default_state());
        Self {
            states,
            inputs: HashMap::new(),
            parent_map: HashMap::new(),
            lib: genesis,
        }
    }

    /// Process a block.
    ///
    /// Derives the branch state from the parent's state via the lens, and
    /// stores both the state and the input keyed by `id`. If the block has
    /// already been processed, its state and input are overwritten —
    /// reprocessing is safe in a chain context because block ids are
    /// content-addressed (same id ⇒ same content).
    ///
    /// If `parent` is not known to the tree (e.g. after backfill where LIB
    /// advanced between batches and the parent was pruned), the lens is
    /// applied against `V::default_state()` and the parent relationship is
    /// still recorded — so a future descendant can still walk the lineage.
    pub fn process(&mut self, id: Id, parent: Id, input: V::Input, view: &V) {
        let parent_state = self
            .states
            .get(&parent)
            .cloned()
            .unwrap_or_else(V::default_state);
        let state = view.apply(&parent_state, &input);
        self.states.insert(id.clone(), state);
        self.inputs.insert(id.clone(), input);
        self.parent_map.insert(id, parent);
    }

    /// Accumulated state at `id`, if known.
    pub fn state_at(&self, id: &Id) -> Option<&V::State> {
        self.states.get(id)
    }

    /// Per-block input at `id`, if known and not yet finalized.
    pub fn input_at(&self, id: &Id) -> Option<&V::Input> {
        self.inputs.get(id)
    }

    /// Mutable iterator over all branch states in the tree.
    ///
    /// Useful for tree-wide mutations that aren't expressible as a per-block
    /// fold — e.g. shrinking states in response to information learned
    /// outside the lens (such as which entries became finalized in an
    /// external system). The lens itself remains a pure function; this
    /// escape hatch lets the driver perform cross-branch maintenance.
    pub fn iter_states_mut(&mut self) -> impl Iterator<Item = &mut V::State> + '_ {
        self.states.values_mut()
    }

    /// Current LIB.
    pub const fn lib(&self) -> &Id {
        &self.lib
    }

    /// Lowest common ancestor of `a` and `b`, walking the parent chain.
    ///
    /// Returns `None` if the two ids are not in the same tree.
    pub fn find_lcm(&self, a: &Id, b: &Id) -> Option<Id> {
        let mut a_chain: HashSet<Id> = HashSet::new();
        let mut cur = a.clone();
        loop {
            a_chain.insert(cur.clone());
            match self.parent_map.get(&cur) {
                Some(p) => cur = p.clone(),
                None => break,
            }
        }
        let mut cur = b.clone();
        loop {
            if a_chain.contains(&cur) {
                return Some(cur);
            }
            match self.parent_map.get(&cur) {
                Some(p) => cur = p.clone(),
                None => return None,
            }
        }
    }

    /// Inputs along the path from `from` (exclusive) up to `to` (inclusive),
    /// in oldest-first order.
    ///
    /// Returns an empty vec if `from` is not an ancestor of `to`, or if
    /// any input on the path has been finalized (and therefore moved out).
    pub fn inputs_between(&self, from: &Id, to: &Id) -> Vec<&V::Input> {
        let mut chain = Vec::new();
        let mut cur = to.clone();
        while &cur != from {
            let Some(input) = self.inputs.get(&cur) else {
                return Vec::new();
            };
            chain.push(input);
            match self.parent_map.get(&cur) {
                Some(p) => cur = p.clone(),
                None => return Vec::new(),
            }
        }
        chain.reverse();
        chain
    }

    /// Advance LIB to `new_lib`.
    ///
    /// Returns the chain segment from old LIB (exclusive) up to `new_lib`
    /// (inclusive) in oldest-first order — these are the newly finalized
    /// blocks. Their inputs are moved out of the tree (no longer needed
    /// for branch computation).
    ///
    /// All branches that are not descendants of `new_lib` are pruned —
    /// they've been definitively orphaned by finalization.
    ///
    /// # Panics
    /// Panics if `new_lib` is not a descendant of the current LIB.
    pub fn advance_lib(&mut self, new_lib: Id) -> Vec<PrunedBlock<Id, V::Input>> {
        if new_lib == self.lib {
            return Vec::new();
        }
        let path = self.path_from_lib_to(&new_lib);
        let keep = self.subtree_of(&new_lib);

        let mut finalized = Vec::with_capacity(path.len());
        for id in path {
            let input = self
                .inputs
                .remove(&id)
                .expect("block on finalized path must have an input");
            self.parent_map.remove(&id);
            // Keep state at the new LIB — it's the new root for future
            // branch computation. Drop everything strictly below it.
            if id != new_lib {
                self.states.remove(&id);
            }
            finalized.push(PrunedBlock { id, input });
        }

        let to_drop: Vec<Id> = self
            .states
            .keys()
            .filter(|id| !keep.contains(id))
            .cloned()
            .collect();
        for id in to_drop {
            self.states.remove(&id);
            self.inputs.remove(&id);
            self.parent_map.remove(&id);
        }

        self.lib = new_lib;
        finalized
    }

    /// Walk `parent_map` from `target` back to current LIB; oldest-first.
    fn path_from_lib_to(&self, target: &Id) -> Vec<Id> {
        let mut path = Vec::new();
        let mut cur = target.clone();
        while cur != self.lib {
            path.push(cur.clone());
            match self.parent_map.get(&cur) {
                Some(p) => cur = p.clone(),
                None => panic!("target is not a descendant of current LIB"),
            }
        }
        path.reverse();
        path
    }

    /// Set of `root` and all its known descendants.
    fn subtree_of(&self, root: &Id) -> HashSet<Id> {
        let mut children: HashMap<&Id, Vec<&Id>> = HashMap::new();
        for (child, parent) in &self.parent_map {
            children.entry(parent).or_default().push(child);
        }
        let mut keep: HashSet<Id> = HashSet::new();
        keep.insert(root.clone());
        let mut frontier = vec![root.clone()];
        while let Some(node) = frontier.pop() {
            if let Some(kids) = children.get(&node) {
                for k in kids {
                    if keep.insert((*k).clone()) {
                        frontier.push((*k).clone());
                    }
                }
            }
        }
        keep
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// Toy lens: each input is a `&str` tag; state is the accumulated set
    /// of tags along a branch. Sufficient to exercise tree mechanics
    /// without dragging in any chain types.
    struct TagLens;

    impl BlockView for TagLens {
        type Input = &'static str;
        type State = BTreeSet<&'static str>;

        fn apply(&self, parent: &Self::State, input: &Self::Input) -> Self::State {
            let mut s = parent.clone();
            s.insert(*input);
            s
        }

        fn default_state() -> Self::State {
            BTreeSet::new()
        }
    }

    fn linear() -> BlockTree<u64, TagLens> {
        // 0 (genesis) -> 1 -> 2 -> 3
        let mut tree = BlockTree::<u64, TagLens>::new(0);
        tree.process(1, 0, "a", &TagLens);
        tree.process(2, 1, "b", &TagLens);
        tree.process(3, 2, "c", &TagLens);
        tree
    }

    #[test]
    fn linear_apply_accumulates() {
        let tree = linear();
        assert_eq!(tree.state_at(&3).unwrap(), &BTreeSet::from(["a", "b", "c"]));
    }

    #[test]
    fn inputs_between_returns_oldest_first() {
        let tree = linear();
        let path = tree.inputs_between(&0, &3);
        assert_eq!(path, vec![&"a", &"b", &"c"]);
    }

    #[test]
    fn fork_diverges_state() {
        // 0 -> 1 -> 2a
        //        \-> 2b
        let mut tree = BlockTree::<u64, TagLens>::new(0);
        tree.process(1, 0, "shared", &TagLens);
        tree.process(20, 1, "a-only", &TagLens);
        tree.process(21, 1, "b-only", &TagLens);

        assert_eq!(
            tree.state_at(&20).unwrap(),
            &BTreeSet::from(["shared", "a-only"])
        );
        assert_eq!(
            tree.state_at(&21).unwrap(),
            &BTreeSet::from(["shared", "b-only"])
        );
    }

    #[test]
    fn lcm_finds_fork_point() {
        let mut tree = BlockTree::<u64, TagLens>::new(0);
        tree.process(1, 0, "x", &TagLens);
        tree.process(2, 1, "y", &TagLens);
        tree.process(20, 1, "a", &TagLens);
        tree.process(21, 20, "a2", &TagLens);

        assert_eq!(tree.find_lcm(&2, &21), Some(1));
        assert_eq!(tree.find_lcm(&2, &2), Some(2));
        assert_eq!(tree.find_lcm(&21, &0), Some(0));
    }

    #[test]
    fn lcm_unrelated_returns_none() {
        let tree = BlockTree::<u64, TagLens>::new(0);
        assert_eq!(tree.find_lcm(&999, &0), None);
    }

    #[test]
    fn advance_lib_returns_finalized_segment_oldest_first() {
        let mut tree = linear();
        let pruned = tree.advance_lib(3);
        let ids: Vec<u64> = pruned.iter().map(|p| p.id).collect();
        assert_eq!(ids, vec![1, 2, 3]);
        assert_eq!(*tree.lib(), 3);
        assert_eq!(tree.state_at(&3).unwrap(), &BTreeSet::from(["a", "b", "c"]));
    }

    #[test]
    fn advance_lib_prunes_orphan_branches() {
        // 0 -> 1 -> 2a (becomes new LIB)
        //        \-> 2b (should be pruned)
        let mut tree = BlockTree::<u64, TagLens>::new(0);
        tree.process(1, 0, "x", &TagLens);
        tree.process(20, 1, "a", &TagLens);
        tree.process(21, 1, "b", &TagLens);

        drop(tree.advance_lib(20));
        assert!(tree.state_at(&20).is_some(), "new LIB kept");
        assert!(tree.state_at(&21).is_none(), "orphan branch pruned");
    }

    #[test]
    fn advance_lib_keeps_descendants_of_new_lib() {
        // 0 -> 1 -> 2 -> 3a
        //               \-> 3b
        // advance to 2 — 3a and 3b are descendants of new LIB, both kept.
        let mut tree = BlockTree::<u64, TagLens>::new(0);
        tree.process(1, 0, "a", &TagLens);
        tree.process(2, 1, "b", &TagLens);
        tree.process(30, 2, "c1", &TagLens);
        tree.process(31, 2, "c2", &TagLens);

        drop(tree.advance_lib(2));
        assert!(tree.state_at(&30).is_some());
        assert!(tree.state_at(&31).is_some());
        assert!(tree.state_at(&1).is_none(), "below new LIB pruned");
    }

    #[test]
    fn advance_lib_no_op_when_unchanged() {
        let mut tree = linear();
        let pruned = tree.advance_lib(0);
        assert!(pruned.is_empty());
    }

    #[test]
    #[should_panic(expected = "not a descendant")]
    fn advance_lib_to_unrelated_panics() {
        let mut tree = BlockTree::<u64, TagLens>::new(0);
        tree.process(1, 0, "a", &TagLens);
        tree.process(20, 1, "b", &TagLens);
        // 999 is not in the tree — should panic.
        drop(tree.advance_lib(999));
    }

    #[test]
    fn reorg_workflow_produces_correct_diffs() {
        // Simulate the multi-sequencer reorg pattern:
        // 0 -> 1 -> 2a -> 3a   (old canonical)
        //        \-> 2b -> 3b  (new canonical after reorg)
        //
        // From a switch from 3a to 3b:
        //   orphaned = inputs on 1..3a = ["2a", "3a"] (oldest first)
        //   adopted  = inputs on 1..3b = ["2b", "3b"] (oldest first)
        let mut tree = BlockTree::<u64, TagLens>::new(0);
        tree.process(1, 0, "x", &TagLens);
        tree.process(20, 1, "2a", &TagLens);
        tree.process(30, 20, "3a", &TagLens);
        tree.process(21, 1, "2b", &TagLens);
        tree.process(31, 21, "3b", &TagLens);

        let lcm = tree.find_lcm(&30, &31).unwrap();
        assert_eq!(lcm, 1);

        let orphaned: Vec<&&str> = tree.inputs_between(&lcm, &30);
        let adopted: Vec<&&str> = tree.inputs_between(&lcm, &31);
        assert_eq!(orphaned, vec![&"2a", &"3a"]);
        assert_eq!(adopted, vec![&"2b", &"3b"]);
    }
}
