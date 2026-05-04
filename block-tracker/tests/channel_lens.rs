//! End-to-end test of `BlockView` / `BlockTree` modeling zone-sdk's
//! channel inscription tracking.
//!
//! Mirrors the data shapes used by `zone-sdk/src/state.rs::TxState`:
//!   - state: `HashTrieSetSync<TxHash>` — cumulative safe-tx set per branch
//!   - per-block input: `Vec<InscriptionInfo>` — inscriptions in this block
//!   - id: `HeaderId` (modeled here as `[u8; 32]` to avoid an `lb-core`
//!     dev-dep; the real type is just a newtype around a 32-byte array)
//!
//! The test exercises the multi-sequencer reorg pattern:
//!   - two competing branches publish inscriptions
//!   - reorg switches from one tip to the other
//!   - `inputs_between(lcm, tip)` produces the orphaned/adopted lists that
//!     drive `Event::ChannelUpdate` in zone-sdk

use logos_blockchain_block_tracker::{BlockTree, BlockView};
use rpds::HashTrieSetSync;

type HeaderId = [u8; 32];
type TxHash = [u8; 32];
type MsgId = [u8; 32];

const fn id(b: u8) -> [u8; 32] {
    let mut a = [0u8; 32];
    a[0] = b;
    a
}

#[derive(Debug, Clone, PartialEq)]
struct InscriptionInfo {
    tx_hash: TxHash,
    parent_msg: MsgId,
    this_msg: MsgId,
    payload: Vec<u8>,
}

struct ChannelLens;

impl BlockView for ChannelLens {
    type Input = Vec<InscriptionInfo>;
    type State = HashTrieSetSync<TxHash>;

    fn apply(&self, parent: &Self::State, input: &Self::Input) -> Self::State {
        let mut state = parent.clone();
        for ins in input {
            state.insert_mut(ins.tx_hash);
        }
        state
    }

    fn default_state() -> Self::State {
        HashTrieSetSync::new_sync()
    }
}

fn ins(tx: u8, this: u8, parent: u8) -> InscriptionInfo {
    InscriptionInfo {
        tx_hash: id(tx),
        parent_msg: id(parent),
        this_msg: id(this),
        payload: vec![tx],
    }
}

#[test]
fn linear_safe_set_grows() {
    let genesis = id(0);
    let mut tree = BlockTree::<HeaderId, ChannelLens>::new(genesis);
    tree.process(id(1), genesis, vec![ins(0xa1, 1, 0)], &ChannelLens);
    tree.process(id(2), id(1), vec![ins(0xa2, 2, 1)], &ChannelLens);
    tree.process(id(3), id(2), vec![ins(0xa3, 3, 2)], &ChannelLens);

    let safe_set = tree.state_at(&id(3)).unwrap();
    assert_eq!(safe_set.size(), 3);
    assert!(safe_set.contains(&id(0xa1)));
    assert!(safe_set.contains(&id(0xa2)));
    assert!(safe_set.contains(&id(0xa3)));
}

#[test]
fn competing_sequencers_reorg_produces_orphaned_and_adopted() {
    // Two sequencers publish on a fork:
    //   genesis -> 1 -> 2a -> 3a   (sequencer A's branch)
    //                \-> 2b -> 3b  (sequencer B's branch wins)
    //
    // After a reorg from 3a to 3b, zone-sdk should emit:
    //   orphaned = [inscriptions in 2a, inscriptions in 3a]
    //   adopted  = [inscriptions in 2b, inscriptions in 3b]
    let genesis = id(0);
    let mut tree = BlockTree::<HeaderId, ChannelLens>::new(genesis);

    tree.process(id(1), genesis, vec![ins(0x10, 1, 0)], &ChannelLens);

    // Sequencer A's branch
    tree.process(id(0x2a), id(1), vec![ins(0xa2, 0x2a, 1)], &ChannelLens);
    tree.process(
        id(0x3a),
        id(0x2a),
        vec![ins(0xa3, 0x3a, 0x2a)],
        &ChannelLens,
    );

    // Sequencer B's branch (becomes canonical)
    tree.process(id(0x2b), id(1), vec![ins(0xb2, 0x2b, 1)], &ChannelLens);
    tree.process(
        id(0x3b),
        id(0x2b),
        vec![ins(0xb3, 0x3b, 0x2b)],
        &ChannelLens,
    );

    // Reorg: lcm gives us the fork point; inputs_between gives orphaned/adopted.
    let lcm = tree.find_lcm(&id(0x3a), &id(0x3b)).expect("lcm exists");
    assert_eq!(lcm, id(1), "fork point is block 1");

    let orphaned: Vec<_> = tree
        .inputs_between(&lcm, &id(0x3a))
        .into_iter()
        .flatten()
        .collect();
    let adopted: Vec<_> = tree
        .inputs_between(&lcm, &id(0x3b))
        .into_iter()
        .flatten()
        .collect();

    assert_eq!(orphaned.len(), 2);
    assert_eq!(orphaned[0].tx_hash, id(0xa2));
    assert_eq!(orphaned[1].tx_hash, id(0xa3));

    assert_eq!(adopted.len(), 2);
    assert_eq!(adopted[0].tx_hash, id(0xb2));
    assert_eq!(adopted[1].tx_hash, id(0xb3));
}

#[test]
fn safe_set_diverges_per_branch() {
    // Each branch should only see its own inscriptions in the safe set.
    let genesis = id(0);
    let mut tree = BlockTree::<HeaderId, ChannelLens>::new(genesis);

    tree.process(id(1), genesis, vec![ins(0x10, 1, 0)], &ChannelLens);
    tree.process(id(0x2a), id(1), vec![ins(0xa2, 0x2a, 1)], &ChannelLens);
    tree.process(id(0x2b), id(1), vec![ins(0xb2, 0x2b, 1)], &ChannelLens);

    let a_set = tree.state_at(&id(0x2a)).unwrap();
    let b_set = tree.state_at(&id(0x2b)).unwrap();

    assert!(a_set.contains(&id(0x10)), "shared block visible on A");
    assert!(a_set.contains(&id(0xa2)), "A's own inscription visible");
    assert!(!a_set.contains(&id(0xb2)), "B's inscription not on A");

    assert!(b_set.contains(&id(0x10)));
    assert!(b_set.contains(&id(0xb2)));
    assert!(!b_set.contains(&id(0xa2)));
}

#[test]
fn lib_advance_finalizes_segment_and_prunes_competing_branch() {
    // genesis -> 1 -> 2a (canonical, becomes new LIB)
    //              \-> 2b (competing branch — pruned at finalization)
    let genesis = id(0);
    let mut tree = BlockTree::<HeaderId, ChannelLens>::new(genesis);
    tree.process(id(1), genesis, vec![ins(0x10, 1, 0)], &ChannelLens);
    tree.process(id(0x2a), id(1), vec![ins(0xa2, 0x2a, 1)], &ChannelLens);
    tree.process(id(0x2b), id(1), vec![ins(0xb2, 0x2b, 1)], &ChannelLens);

    let pruned = tree.advance_lib(id(0x2a));

    // Finalized segment is [block 1, block 2a] in oldest-first order.
    let finalized_ids: Vec<_> = pruned.iter().map(|p| p.id).collect();
    assert_eq!(finalized_ids, vec![id(1), id(0x2a)]);

    // The pruned blocks carry their per-block inscriptions for the
    // consumer to materialize as "finalized" effects.
    assert_eq!(pruned[0].input.len(), 1);
    assert_eq!(pruned[0].input[0].tx_hash, id(0x10));
    assert_eq!(pruned[1].input[0].tx_hash, id(0xa2));

    // State at the new LIB is preserved; competing branch is gone.
    assert!(tree.state_at(&id(0x2a)).is_some(), "new LIB state kept");
    assert!(
        tree.state_at(&id(0x2b)).is_none(),
        "competing branch pruned"
    );

    let safe_set = tree.state_at(&id(0x2a)).unwrap();
    assert_eq!(safe_set.size(), 2, "safe set unchanged by finalization");
}
