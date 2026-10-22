"""RLP-encodes the header of an empty Engine V1 execution payload.

Printed so `cast keccak` can hash it: that hash is the block hash the node will compute, so a
probe payload can be edited and re-hashed instead of being rejected for a stale hash.

Only empty payloads are supported — the transactions root of a block with transactions needs a
Merkle-Patricia trie, which is not worth reimplementing here.

Usage: header_rlp.py PAYLOAD_JSON [BASE_FEE_OVERRIDE]
"""

import json
import sys

# A DogeOS header fixes these: no ommers, difficulty one, zero nonce, and a mix hash of zero.
EMPTY_OMMER_ROOT = "0x1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347"
EMPTY_TRIE_ROOT = "0x56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421"
DIFFICULTY = 1


def length_prefix(length: int, offset: int) -> bytes:
    if length < 56:
        return bytes([offset + length])
    encoded = length.to_bytes((length.bit_length() + 7) // 8, "big")
    return bytes([offset + 55 + len(encoded)]) + encoded


def rlp(item) -> bytes:
    if isinstance(item, list):
        body = b"".join(rlp(entry) for entry in item)
        return length_prefix(len(body), 0xC0) + body
    if len(item) == 1 and item[0] < 0x80:
        return item
    return length_prefix(len(item), 0x80) + item


def integer(value: int) -> bytes:
    return b"" if value == 0 else value.to_bytes((value.bit_length() + 7) // 8, "big")


def raw(value: str) -> bytes:
    return bytes.fromhex(value.removeprefix("0x"))


def main() -> None:
    if not 2 <= len(sys.argv) <= 3:
        raise SystemExit(__doc__)
    payload = json.loads(sys.argv[1])
    if payload["transactions"]:
        raise SystemExit("only an empty payload's header can be encoded here")
    base_fee = int(sys.argv[2], 0) if len(sys.argv) == 3 else int(payload["baseFeePerGas"], 16)

    fields = [
        raw(payload["parentHash"]),
        raw(EMPTY_OMMER_ROOT),
        raw(payload["feeRecipient"]),
        raw(payload["stateRoot"]),
        raw(EMPTY_TRIE_ROOT),
        raw(payload["receiptsRoot"]),
        raw(payload["logsBloom"]),
        integer(DIFFICULTY),
        integer(int(payload["blockNumber"], 16)),
        integer(int(payload["gasLimit"], 16)),
        integer(int(payload["gasUsed"], 16)),
        integer(int(payload["timestamp"], 16)),
        raw(payload["extraData"]),
        raw(payload["prevRandao"]),
        bytes(8),
        integer(base_fee),
    ]
    print("0x" + rlp(fields).hex())


if __name__ == "__main__":
    main()
