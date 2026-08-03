#!/usr/bin/env sh
set -eu

metadata_file=$(mktemp)
trap 'rm -f "$metadata_file"' EXIT

cargo metadata --locked --offline --format-version 1 > "$metadata_file"

single_package() {
    name=$1
    version=$2
    jq -e --arg name "$name" --arg version "$version" '
        [.packages[] | select(.name == $name and .version == $version)] | length == 1
    ' "$metadata_file" > /dev/null
}

single_package revm 36.0.0
single_package alloy-evm 0.30.0
single_package alloy-consensus 1.8.2
single_package alloy-primitives 1.5.7

jq -e '
    [.packages[] | select(.name == "revm-scroll") | .source]
    == ["git+https://github.com/DogeOS69/dogeos-revm.git?rev=1b87ecf17af029ac2f39e8ad362f3503ff2f4583#1b87ecf17af029ac2f39e8ad362f3503ff2f4583"]
' "$metadata_file" > /dev/null

jq -e '
    [.packages[] | select(.name == "reth-node-builder") | .source]
    == ["git+https://github.com/paradigmxyz/reth.git?rev=eb4c15e5e36d8776d46629beae4c0a69af7ab04f#eb4c15e5e36d8776d46629beae4c0a69af7ab04f"]
' "$metadata_file" > /dev/null

echo "dependency graph verified: Reth 2 / REVM 36 / DogeOS revm-scroll"
