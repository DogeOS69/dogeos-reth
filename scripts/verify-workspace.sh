#!/usr/bin/env sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

cargo fmt --all -- --check
scripts/verify-dependency-graph.sh
scripts/verify-fixtures.sh
scripts/audit-rocksdb-durability.sh
cargo test --workspace --all-targets --locked --offline

echo "workspace migration gates verified"
