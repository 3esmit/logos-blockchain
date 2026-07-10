//! Well-known wire fixtures for this crate's codecs.
//!
//! Each [`wire_fixtures!`](lb_wire::wire_fixtures) emits the type's
//! `WireExamples` impl (the fixture the codec traits require) plus a
//! `#[cfg(test)]` round-trip test. Gathering them here keeps the codec impls in
//! [`crate::wire`] free of fixture noise and gives a single auditable list of
//! every golden vector.

use lb_wire::wire_fixtures;

use crate::Epoch;

wire_fixtures!(Epoch, Self::new(1) => "01000000");
