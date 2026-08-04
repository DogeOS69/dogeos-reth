#!/usr/bin/env sh
set -eu

metadata_file=$(mktemp)
trap 'rm -f "$metadata_file"' EXIT

cargo metadata --locked --offline --format-version 1 > "$metadata_file"

reth_source="git+https://github.com/paradigmxyz/reth.git?rev=eb4c15e5e36d8776d46629beae4c0a69af7ab04f#eb4c15e5e36d8776d46629beae4c0a69af7ab04f"
revm_scroll_source="git+https://github.com/DogeOS69/dogeos-revm.git?branch=chore/upgrade-revm-v36#1b87ecf17af029ac2f39e8ad362f3503ff2f4583"
da_codec_source="git+https://github.com/scroll-tech/da-codec?rev=54929786434f00efd00431517a332f1ec8ca58d4#54929786434f00efd00431517a332f1ec8ca58d4"

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

jq -e --arg source "$revm_scroll_source" '
    [.packages[] | select(.name == "revm-scroll") | .source]
    == [$source]
' "$metadata_file" > /dev/null

jq -e --arg source "$reth_source" '
    [.packages[] | select(.name == "reth-node-builder") | .source]
    == [$source]
' "$metadata_file" > /dev/null

jq -e \
    --arg reth "$reth_source" \
    --arg revm_scroll "$revm_scroll_source" \
    --arg da_codec "$da_codec_source" '
    ([.packages[].source | select(. != null and startswith("git+"))] | unique | sort)
    == ([$reth, $revm_scroll, $da_codec] | sort)
' "$metadata_file" > /dev/null

jq -e '
    .workspace_members as $members
    | [.packages[]
       | select(.id as $id | $members | index($id))
       | .name
       | select(test("^reth(-|$)"))]
    | length == 0
' "$metadata_file" > /dev/null

echo "dependency graph verified: upstream Reth 2 / REVM 36 / DogeOS revm-scroll"
