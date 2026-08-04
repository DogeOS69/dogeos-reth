#!/usr/bin/env sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

metadata_file=$(mktemp)
trap 'rm -f "$metadata_file"' EXIT
cargo metadata --locked --offline --format-version 1 > "$metadata_file"

provider_manifest=$(jq -r '
    .packages[]
    | select(.name == "reth-provider" and .version == "2.0.0")
    | .manifest_path
' "$metadata_file")

if [ -z "$provider_manifest" ]; then
    echo "Reth 2 provider source was not found in the locked graph" >&2
    exit 1
fi

provider_source=$(dirname "$provider_manifest")/src/providers/rocksdb/provider.rs
if rg -q 'set_sync\(true\)' "$provider_source"; then
    echo "RocksDB synchronous-write durability code is present"
    exit 0
fi

echo "release blocker: locked Reth 2 provider has no set_sync(true) write option" >&2
echo "source: $provider_source" >&2
exit 1
