#!/bin/bash

set -e

pushd repo

cat <<EOF | cargo login
${CRATES_API_TOKEN}
EOF

# Dependency order: each crate must be on crates.io before the next one that
# depends on it is published.
cargo publish -p errlanes-derive --all-features --no-verify
cargo publish -p errlanes --all-features --no-verify
cargo publish -p es-entity-macros --all-features --no-verify
cargo publish -p es-entity --all-features --no-verify
