//! Generic per-branch state tracker over a block tree.
//!
//! A [`BlockView`] is a strategy that describes how to derive per-block
//! state from an input. A [`BlockTree`] maintains those states across the
//! chain's branching structure, supports reorgs via lowest-common-ancestor
//! lookup, and prunes finalized history as LIB advances.
//!
//! The `Input` type is generic so consumers can feed not just blocks but
//! richer per-position data (e.g. `(Block, Payouts)`) — useful when the
//! domain-of-interest depends on data that the protocol computes alongside
//! a block but that doesn't appear in transactions.
//!
//! By convention, the driver is responsible for summarizing raw chain data
//! into the lens's `Input` type before calling [`BlockTree::process`]. The
//! lens's `apply` method is itself a pure derivation `(parent, input) ->
//! child`, but the tree exposes mutation primitives (e.g.
//! [`BlockTree::iter_states_mut`]) for cross-branch maintenance that
//! doesn't fit a per-block fold.

mod tree;

pub use tree::{BlockTree, PrunedBlock};

/// View into a chain: how to fold per-block input into accumulated branch
/// state.
///
/// Implementations are pure functions of their arguments. The driver feeds
/// `Input` for each block; the tracker takes care of branching and reorgs.
pub trait BlockView {
    /// Per-position input. May be a block summary, or a tuple bundling a
    /// block with additional protocol-derived data the lens needs.
    type Input;

    /// Accumulated state along a branch (e.g. UTXO set, safe-tx set).
    ///
    /// Required to be `Clone` because the tree stores one state per block;
    /// implementations should use a persistent data structure (e.g. `rpds`)
    /// to keep cloning cheap across deep histories.
    type State: Clone;

    /// Derive child state from parent state plus this block's input.
    fn apply(&self, parent: &Self::State, input: &Self::Input) -> Self::State;

    /// Initial state at genesis (before any block has been applied).
    fn default_state() -> Self::State;
}
