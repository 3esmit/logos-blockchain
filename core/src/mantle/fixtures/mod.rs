//! Centralized well-known wire fixtures for every Mantle codec.
//!
//! Each [`wire_fixtures!`](lb_wire::wire_fixtures) below emits the type's
//! `WireExamples` impl — the fixture the codec traits require — plus a
//! `#[cfg(test)]` round-trip test. Gathering them in one module keeps the type
//! definitions clean (`#[derive(WireCodec)]` only) and gives a single auditable
//! list of every golden vector.

mod channel;
mod genesis;
mod ledger;
mod ops;
mod tx;
