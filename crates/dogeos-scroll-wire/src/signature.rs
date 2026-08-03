use alloy_consensus::Header;
use alloy_primitives::{Address, B256, Signature, U256, keccak256};
use alloy_rlp::Encodable;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BlockSignatureError {
    #[error("failed to recover block signer")]
    RecoveryFailed,
    #[error("block signer {actual} is not authorized; expected {expected}")]
    UnauthorizedSigner { expected: Address, actual: Address },
}

/// Hashes the inherited signing header encoding, which excludes `extra_data`.
pub fn signature_hash(header: &Header) -> B256 {
    let mut encoded = Vec::new();
    alloy_rlp::Header {
        list: true,
        payload_length: signing_payload_length(header),
    }
    .encode(&mut encoded);
    header.parent_hash.encode(&mut encoded);
    header.ommers_hash.encode(&mut encoded);
    header.beneficiary.encode(&mut encoded);
    header.state_root.encode(&mut encoded);
    header.transactions_root.encode(&mut encoded);
    header.receipts_root.encode(&mut encoded);
    header.logs_bloom.encode(&mut encoded);
    header.difficulty.encode(&mut encoded);
    U256::from(header.number).encode(&mut encoded);
    U256::from(header.gas_limit).encode(&mut encoded);
    U256::from(header.gas_used).encode(&mut encoded);
    header.timestamp.encode(&mut encoded);
    header.mix_hash.encode(&mut encoded);
    header.nonce.encode(&mut encoded);
    if let Some(base_fee) = header.base_fee_per_gas {
        U256::from(base_fee).encode(&mut encoded);
    }
    keccak256(encoded)
}

fn signing_payload_length(header: &Header) -> usize {
    header.parent_hash.length()
        + header.ommers_hash.length()
        + header.beneficiary.length()
        + header.state_root.length()
        + header.transactions_root.length()
        + header.receipts_root.length()
        + header.logs_bloom.length()
        + header.difficulty.length()
        + U256::from(header.number).length()
        + U256::from(header.gas_limit).length()
        + U256::from(header.gas_used).length()
        + header.timestamp.length()
        + header.mix_hash.length()
        + header.nonce.length()
        + header
            .base_fee_per_gas
            .map_or(0, |fee| U256::from(fee).length())
}

/// Verifies the signature when an authorized signer is configured.
pub fn verify_block_signature(
    header: &Header,
    signature: &Signature,
    authorized_signer: Option<Address>,
) -> Result<(), BlockSignatureError> {
    let Some(expected) = authorized_signer else {
        return Ok(());
    };
    let actual = reth_primitives_traits::crypto::secp256k1::recover_signer(
        signature,
        signature_hash(header),
    )
    .map_err(|_| BlockSignatureError::RecoveryFailed)?;
    if actual != expected {
        return Err(BlockSignatureError::UnauthorizedSigner { expected, actual });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::Bytes;

    #[test]
    fn signing_hash_excludes_extra_data_but_commits_to_header_fields() {
        let mut header = Header {
            number: 7,
            ..Default::default()
        };
        let expected = signature_hash(&header);
        header.extra_data = Bytes::from_static(b"signature bytes are excluded");
        assert_eq!(signature_hash(&header), expected);
        header.number += 1;
        assert_ne!(signature_hash(&header), expected);
    }
}
