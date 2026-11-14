#!/usr/bin/env bash
# Runs the book's doc tests. Extra arguments go to `mdbook test`.
#
# Building with --target keeps proc-macro dependencies out of the target's deps
# dir. Without it, deps holds two rlibs of crates like tokio (host and target
# features differ) and rustc rejects them with E0464. The proc-macro libraries
# themselves are still needed, so collect them alongside the target libraries.
set -euo pipefail

target_dir="${CARGO_TARGET_DIR:-./target}"
host="$(rustc -vV | sed -n 's/^host: //p')"
out="$target_dir/$host/mdbook-test"
libs="$target_dir/mdbook-test-libs"

rm -rf "$out" "$target_dir/mdbook-test" "$libs"
cargo build --profile mdbook-test --features mdbook-test --lib --target "$host"
mkdir -p "$libs"
find "$out" -type f -name '*.rlib' -exec cp {} "$libs/" \;
find "$target_dir/mdbook-test" -type f \( -name '*.so' -o -name '*.dylib' \) -exec cp {} "$libs/" \;

CARGO_MANIFEST_DIR="$(pwd)" mdbook test book -L "$libs" "$@"
