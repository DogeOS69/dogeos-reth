//! Reth-bound primitive adapters for DogeOS.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/scroll-tech/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(feature = "std"), no_std)]

use once_cell as _;

pub mod transaction;
pub use transaction::{ScrollTransactionSigned, tx_type::ScrollTxType};

use reth_primitives_traits::Block;

mod receipt;
pub use receipt::ScrollReceipt;

/// DogeOS block type using the inherited Scroll transaction envelope.
pub type DogeosBlock = alloy_consensus::Block<ScrollTransactionSigned>;

/// Scroll-specific block body type.
pub type DogeosBlockBody = <DogeosBlock as Block>::Body;

/// Primitive types for Scroll Node.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DogeosPrimitives;

impl reth_primitives_traits::NodePrimitives for DogeosPrimitives {
    type Block = DogeosBlock;
    type BlockHeader = alloy_consensus::Header;
    type BlockBody = DogeosBlockBody;
    type SignedTx = ScrollTransactionSigned;
    type Receipt = ScrollReceipt;
}
