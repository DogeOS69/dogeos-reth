#!/usr/bin/env sh
set -eu

if [ "$#" -ne 1 ]; then
    echo "usage: $0 /path/to/dogeos-reth" >&2
    exit 64
fi

oracle_dir=$1
expected_revision=6b62297a0a8a3d88c873a0fb2a11b52d2cc8824f
expected_lockfile=853f76864b35545c47e71261607d3e20fc0d2534f45a6b34ca063c3a0dc713c5

test -d "$oracle_dir/.git"
test "$(git -C "$oracle_dir" rev-parse HEAD)" = "$expected_revision"
test "$(shasum -a 256 "$oracle_dir/Cargo.lock" | awk '{print $1}')" = "$expected_lockfile"

check_file() {
    expected_hash=$1
    relative_path=$2
    actual_hash=$(shasum -a 256 "$oracle_dir/$relative_path" | awk '{print $1}')
    test "$actual_hash" = "$expected_hash"
}

check_file 87b23f048986196bdcffe74159b1bdf2924865196af6bda98cabcb4d2cd842da crates/scroll/chainspec/res/genesis/dogeos.json
check_file c6effe795d7a5b000f07167ca1c97b6fa8c428acfdcf65641ee6bf9e4b32390b crates/scroll/chainspec/res/genesis/chikyu_dogeos.json
check_file 2e450321d7bf396ca9597d86e1cc5e603065e1ee78663f4b3b85d8265ba92619 crates/scroll/chainspec/res/genesis/dev.json

echo "oracle baseline verified: $expected_revision"
