"""RLP-encodes a DogeOS L1 message transaction (type 0x7e).

Field order, from dogeos-protocol-types `TxL1Message::rlp_encode_fields`:
queue_index, gas_limit, to, value, input, sender.

Usage: l1_message.py QUEUE_INDEX GAS_LIMIT TO VALUE INPUT_HEX SENDER
"""

import sys


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


def address(value: str) -> bytes:
    raw = bytes.fromhex(value.removeprefix("0x"))
    if len(raw) != 20:
        raise SystemExit(f"not an address: {value}")
    return raw


def main() -> None:
    if len(sys.argv) != 7:
        raise SystemExit(__doc__)
    queue_index, gas_limit, to, value, data, sender = sys.argv[1:]
    fields = [
        integer(int(queue_index, 0)),
        integer(int(gas_limit, 0)),
        address(to),
        integer(int(value, 0)),
        bytes.fromhex(data.removeprefix("0x")),
        address(sender),
    ]
    print("0x7e" + rlp(fields).hex())


if __name__ == "__main__":
    main()
