#!/bin/bash

set -e

pushd repo

cat <<EOF | cargo login
${CRATES_API_TOKEN}
EOF

cargo publish -p es-entity-macros --all-features --no-verify
cargo publish -p es-entity --all-features --no-verify
