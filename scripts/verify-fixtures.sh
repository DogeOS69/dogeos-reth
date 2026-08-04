#!/usr/bin/env sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo_root"

for fixture in fixtures/*/*.json; do
    jq -e 'type == "object" and (.schema | type == "string") and (.provenance | type == "string")' \
        "$fixture" > /dev/null
done

shasum -a 256 -c fixtures/SHA256SUMS

echo "fixture corpus verified"
