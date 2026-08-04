//! DogeOS Storage V2 body semantics.

use dogeos_reth_primitives::{DogeosBlock, DogeosBlockBody, DogeosPrimitives};
use reth_db_api::transaction::{DbTx, DbTxMut};
use reth_provider::providers::{ChainStorage, NodeTypesForProvider};
use reth_provider::{DatabaseProvider, ProviderResult};
use reth_storage_api::{BlockBodyReader, BlockBodyWriter, ReadBodyInput};

/// Storage policy for DogeOS block bodies.
///
/// Transactions are persisted independently by Reth. DogeOS has no ommers and deliberately does
/// not adopt EIP-4895 withdrawals, even though its EVM rules include Shanghai-era behavior. This
/// implementation prevents generic Ethereum storage from reconstructing a missing withdrawals
/// list as `Some([])` after Shanghai.
#[derive(Debug, Default, Clone, Copy)]
pub struct DogeosStorage;

impl<Provider> BlockBodyReader<Provider> for DogeosStorage {
    type Block = DogeosBlock;

    fn read_block_bodies(
        &self,
        _provider: &Provider,
        inputs: Vec<ReadBodyInput<'_, Self::Block>>,
    ) -> ProviderResult<Vec<DogeosBlockBody>> {
        Ok(inputs
            .into_iter()
            .map(|(_header, transactions)| DogeosBlockBody {
                transactions,
                ommers: Vec::new(),
                withdrawals: None,
            })
            .collect())
    }
}

impl<Provider> BlockBodyWriter<Provider, DogeosBlockBody> for DogeosStorage {
    fn write_block_bodies(
        &self,
        _provider: &Provider,
        _bodies: Vec<(u64, Option<&DogeosBlockBody>)>,
    ) -> ProviderResult<()> {
        // Transactions are written by the generic provider. DogeOS has no additional body data.
        Ok(())
    }

    fn remove_block_bodies_above(&self, _provider: &Provider, _block: u64) -> ProviderResult<()> {
        // There are no DogeOS-specific body tables to unwind.
        Ok(())
    }
}

impl ChainStorage<DogeosPrimitives> for DogeosStorage {
    fn reader<TX, Types>(
        &self,
    ) -> impl reth_storage_api::ChainStorageReader<DatabaseProvider<TX, Types>, DogeosPrimitives>
    where
        TX: DbTx + 'static,
        Types: NodeTypesForProvider<Primitives = DogeosPrimitives>,
    {
        self
    }

    fn writer<TX, Types>(
        &self,
    ) -> impl reth_storage_api::ChainStorageWriter<DatabaseProvider<TX, Types>, DogeosPrimitives>
    where
        TX: DbTxMut + DbTx + 'static,
        Types: NodeTypesForProvider<Primitives = DogeosPrimitives>,
    {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use reth_primitives_traits::RecoveredBlock;
    use reth_provider::{BlockReader, BlockWriter};

    #[test]
    fn body_reconstruction_never_invents_withdrawals() {
        let post_shanghai_header = Header {
            number: 42,
            timestamp: u64::MAX,
            ..Default::default()
        };

        let bodies = DogeosStorage
            .read_block_bodies(&(), vec![(&post_shanghai_header, Vec::new())])
            .unwrap();

        assert_eq!(bodies.len(), 1);
        assert!(bodies[0].transactions.is_empty());
        assert!(bodies[0].ommers.is_empty());
        assert!(bodies[0].withdrawals.is_none());
    }

    #[test]
    fn provider_roundtrip_preserves_absent_withdrawals() {
        let factory = reth_provider::test_utils::create_test_provider_factory_with_node_types::<
            crate::DogeosNodeTypes,
        >(dogeos_chainspec::DOGEOS_DEV.clone());
        let block = RecoveredBlock::new_unhashed(
            alloy_consensus::Block {
                header: Header {
                    number: 0,
                    timestamp: u64::MAX,
                    ..Default::default()
                },
                body: DogeosBlockBody {
                    transactions: Vec::new(),
                    ommers: Vec::new(),
                    withdrawals: None,
                },
            },
            Vec::new(),
        );

        let writer = factory.provider_rw().unwrap();
        writer.insert_block(&block).unwrap();
        writer.commit().unwrap();

        let reader = factory.provider().unwrap();
        let stored = reader.block_by_number(0).unwrap().unwrap();
        assert!(stored.body.withdrawals.is_none());
        assert!(stored.body.ommers.is_empty());
    }
}
