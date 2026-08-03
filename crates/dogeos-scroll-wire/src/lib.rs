//! DogeOS-owned implementation of the inherited `scroll/1` RLPx protocol.

mod config;
mod connection;
mod error;
mod manager;
mod protocol;

pub use config::ScrollWireConfig;
pub use error::AnnounceBlockError;
pub use manager::{LRU_CACHE_SIZE, ScrollWireManager};
pub use protocol::{NewBlock, ScrollWireEvent, ScrollWireProtocolHandler};
