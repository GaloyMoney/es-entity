#!/usr/bin/env bash
#
# Run the cargo-fuzz targets in parallel, with optional corpus restore/persist.
#
# Single source of truth, shared by:
#   - the Concourse `fuzz` job (ci/pipeline.yml) — sets GCS_BUCKET/CREDS/PREFIX
#   - `nix run .#fuzz`                          — flake-provided toolchain
#   - `make fuzz`                               — local, in the dev shell
#
# Corpus handling (mutually exclusive):
#   GCS_BUCKET (+ GCS_CREDS + GCS_PREFIX)  — download latest / upload new via gsutil
#                                            (CI). Self-bootstraps: first run
#                                            finds no corpus and fuzzes from scratch.
#   CORPUS_TARBALL_IN / CORPUS_TARBALL_OUT  — local tarball (elsewhere).
#
# Other env vars (optional, local-friendly defaults):
#   FUZZ_SECONDS        seconds to fuzz each target (default: 60)
#   FUZZ_JOBS           libFuzzer `-jobs` per target; cores = 2 targets * FUZZ_JOBS
#                       (unset => 1 process per target => 2 cores)
#
# Requires: bash, git, cargo, tar, and cargo-fuzz (auto-installed if missing).
# GCS mode additionally requires: gcloud + gsutil (google-cloud-sdk).

set -euo pipefail

ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
cd "$ROOT"

if ! command -v cargo-fuzz >/dev/null 2>&1; then
  echo "cargo-fuzz not found on PATH; installing..."
  cargo install cargo-fuzz --locked
fi

FUZZ_SECONDS="${FUZZ_SECONDS:-60}"

# ── Restore the corpus ────────────────────────────────────────────────
mkdir -p fuzz/corpus
if [ -n "${GCS_BUCKET:-}" ]; then
  printf '%s' "${GCS_CREDS:?GCS_BUCKET set but GCS_CREDS missing}" > /tmp/gcs-key.json
  gcloud auth activate-service-account --key-file=/tmp/gcs-key.json >/dev/null
  prefix="gs://$GCS_BUCKET/${GCS_PREFIX:?GCS_BUCKET set but GCS_PREFIX missing}"
  latest="$(gsutil ls "$prefix/corpus-v*.tgz" 2>/dev/null | sort | tail -1 || true)"
  if [ -n "$latest" ]; then
    echo "restoring corpus from $latest"
    gsutil cp "$latest" /tmp/corpus-in.tgz
    tar -xzf /tmp/corpus-in.tgz -C fuzz/
  else
    echo "no existing corpus in GCS; bootstrapping from scratch"
  fi
elif [ -n "${CORPUS_TARBALL_IN:-}" ] && compgen -G "$CORPUS_TARBALL_IN" >/dev/null; then
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

# ── Persist the evolved corpus ────────────────────────────────────────
if [ -n "${GCS_BUCKET:-}" ]; then
  ts=$(date -u +%Y%m%d-%H%M%S)
  tar -czf /tmp/corpus-v${ts}.tgz -C fuzz corpus
  gsutil cp /tmp/corpus-v${ts}.tgz "$prefix/corpus-v${ts}.tgz"
  echo "uploaded corpus-v${ts}.tgz"
elif [ -n "${CORPUS_TARBALL_OUT:-}" ]; then
  mkdir -p "$(dirname "$CORPUS_TARBALL_OUT")"
  tar -czf "$CORPUS_TARBALL_OUT" -C fuzz corpus
  echo "packaged corpus -> $CORPUS_TARBALL_OUT"
fi
