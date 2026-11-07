#!/usr/bin/env bash
# Runs the book's doc tests. Extra arguments go to `mdbook test`.
#
# Building with --target keeps proc-macro dependencies out of the target's deps
# dir. Without it, deps holds two rlibs of crates like tokio (host and target
# features differ) and rustc rejects them with E0464. The proc-macro libraries
# themselves are still needed, so they are copied across.
set -euo pipefail

target_dir="${CARGO_TARGET_DIR:-./target}"
host="$(rustc -vV | sed -n 's/^host: //p')"
out="$target_dir/$host/mdbook-test"

rm -rf "$out" "$target_dir/mdbook-test"
cargo build --profile mdbook-test --features mdbook-test --lib --target "$host"
find "$target_dir/mdbook-test/deps" -maxdepth 1 \( -name '*.so' -o -name '*.dylib' \) -exec cp {} "$out/deps/" \;

CARGO_MANIFEST_DIR="$(pwd)" mdbook test book -L "$out,$out/deps" "$@"
