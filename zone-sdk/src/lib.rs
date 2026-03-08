pub mod indexer;
pub mod sequencer;
pub mod state;

use lb_core::mantle::ops::channel::MsgId;

// Reexports
pub use lb_chain_broadcast_service::BlockInfo;
pub use lb_chain_service as chain;
pub use lb_common_http_client as http;
pub use lb_common_http_client::{BasicAuthCredentials, CommonHttpClient, Error as HttpError};
pub use lb_core::{block, header, mantle};
pub use lb_key_management_system_service as kms;

/// A zone block — opaque data published to / read from a channel.
pub struct ZoneBlock {
    /// The unique identifier of this inscription.
    pub id: MsgId,
    /// The opaque inscription data.
    pub data: Vec<u8>,
}
