#!/usr/bin/env bash
#
# Run the cargo-fuzz targets in parallel, with optional corpus restore/persist.
#
# Single source of truth, shared by:
#   - the Concourse `fuzz` job (ci/pipeline.yml) — restore/store steps pass the
#     corpus in/out as a tarball via CORPUS_TARBALL_IN/OUT (GCS handled by the
#     google/cloud-sdk steps, not here)
#   - `nix run .#fuzz`                          — flake-provided toolchain
#   - `make fuzz`                               — local, in the dev shell
#
# Env vars (all optional, local-friendly defaults):
#   FUZZ_SECONDS        seconds to fuzz each target (default: 60)
#   FUZZ_JOBS           libFuzzer `-jobs` per target; cores = 2 targets * FUZZ_JOBS
#                       (unset => 1 process per target => 2 cores)
#   CORPUS_TARBALL_IN   glob of a corpus tarball to extract before fuzzing
#   CORPUS_TARBALL_OUT  path to write the evolved corpus tarball after fuzzing
#
# Requires: bash, git, cargo, tar, and cargo-fuzz (auto-installed if missing).

set -euo pipefail

ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
cd "$ROOT"

if ! command -v cargo-fuzz >/dev/null 2>&1; then
  echo "cargo-fuzz not found on PATH; installing..."
  cargo install cargo-fuzz --locked
fi

FUZZ_SECONDS="${FUZZ_SECONDS:-60}"

# Restore the corpus (no-op unless CORPUS_TARBALL_IN is set and matches a file).
mkdir -p fuzz/corpus
if [ -n "${CORPUS_TARBALL_IN:-}" ] && compgen -G "$CORPUS_TARBALL_IN" >/dev/null; then
  echo "restoring corpus from $CORPUS_TARBALL_IN"
  tar -xzf $CORPUS_TARBALL_IN -C fuzz/
fi

(cd fuzz && cargo fuzz build --sanitizer=none)

JOBS_ARG=""
if [ -n "${FUZZ_JOBS:-}" ]; then
  JOBS_ARG="-jobs=$FUZZ_JOBS"
fi

echo "fuzzing both targets in parallel for ${FUZZ_SECONDS}s${JOBS_ARG:+ ($JOBS_ARG per target)}"
pids=""
rc=0
for target in fuzz_parse_constraint_detail fuzz_event_hydration; do
  (cd fuzz && cargo fuzz run "$target" --sanitizer=none -- \
    -max_total_time="$FUZZ_SECONDS" -timeout=25 $JOBS_ARG \
    -artifact_prefix="artifacts/$target/") &
  pids="$pids $!"
done
for p in $pids; do
  wait "$p" || rc=1
done

if [ "$rc" -ne 0 ]; then
  echo "==== FUZZ CRASH DETECTED ===="
  find fuzz/artifacts -type f -print || true
  exit "$rc"
fi

# Package the evolved corpus (no-op unless CORPUS_TARBALL_OUT is set).
if [ -n "${CORPUS_TARBALL_OUT:-}" ]; then
  mkdir -p "$(dirname "$CORPUS_TARBALL_OUT")"
  tar -czf "$CORPUS_TARBALL_OUT" -C fuzz corpus
  echo "packaged corpus -> $CORPUS_TARBALL_OUT"
fi
