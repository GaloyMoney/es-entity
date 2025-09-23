#!/bin/bash

#! Auto synced from Shared CI Resources repository
#! Don't change this file, instead change it in github.com/GaloyMoney/concourse-shared

pushd repo

# First time
if [[ $(cat ../version/version) == "0.0.0" ]]; then
  git cliff --config ../pipeline-tasks/ci/vendor/config/git-cliff.toml > ../artifacts/gh-release-notes.md
else
  export prev_ref="$(git rev-list -n 1 "$(cat ../version/version)")"
  export new_ref="$(git rev-parse HEAD)"
  git cliff --config ../pipeline-tasks/ci/vendor/config/git-cliff.toml "$prev_ref..$new_ref" > ../artifacts/gh-release-notes.md
fi

popd

# Generate Changelog
echo "CHANGELOG:"
echo "-------------------------------"
cat artifacts/gh-release-notes.md
echo "-------------------------------"

# ------------ BUMP VERSION ------------

echo -n "Prev Version: "
cat version/version
echo ""

CURR_VER="$(cat version/version)"

# Parse X.Y.Z
read -r VMAJ VMIN VPATCH < <(tr '.' ' ' < version/version)

IS_PRE_1=0
if (( VMAJ == 0 )); then
  IS_PRE_1=1
fi

HAS_BREAKING=0
HAS_FEATURES=0
if grep -q '\[**breaking**\]' artifacts/gh-release-notes.md; then
  HAS_BREAKING=1
fi
if grep -q '^### Features' artifacts/gh-release-notes.md; then
  HAS_FEATURES=1
fi

# First release default
if [[ "$CURR_VER" == "0.0.0" ]]; then
  # Start at 0.1.0 (pre-1.0.0 line; the middle slot = manual-breaking channel)
  echo "0.1.0" > version/version

else
  if (( IS_PRE_1 == 1 )); then
    # Pre-1.0.0 -> 0.MAJOR.(minor|patch)
    if (( HAS_BREAKING == 1 )); then
      echo "Breaking change detected on pre-1.0.0. The middle component (0.Y.Z) is your MANUAL breaking channel." >&2
      echo "Please bump the middle component manually (e.g., 0.$((VMIN+1)).0) and re-run." >&2
      exit 2
    fi

    if (( HAS_FEATURES == 1 )); then
      echo "Pre-1.0.0 feature detected — bumping PATCH (third component) to reflect new functionality safely."
      bump2version patch --current-version "$CURR_VER" --allow-dirty version/version
    else
      echo "Pre-1.0.0 non-feature changes — bumping PATCH (third component)."
      bump2version patch --current-version "$CURR_VER" --allow-dirty version/version
    fi

  else
    # >= 1.0.0 -> MAJOR.MINOR.PATCH
    if (( HAS_BREAKING == 1 )); then
      echo "Breaking change detected on >=1.0.0 but major is MANUAL only." >&2
      echo "Please bump MAJOR manually (e.g., $((VMAJ+1)).0.0) and re-run." >&2
      exit 2
    fi

    if (( HAS_FEATURES == 1 )); then
      echo "Feature section found, bumping MINOR..."
      bump2version minor --current-version "$CURR_VER" --allow-dirty version/version
    else
      echo "Only patches and fixes found - bumping PATCH..."
      bump2version patch --current-version "$CURR_VER" --allow-dirty version/version
    fi
  fi
fi

echo -n "Release Version: "
cat version/version
echo ""

# ------------ ARTIFACTS ------------

cat version/version > artifacts/gh-release-tag
echo "v$(cat version/version) Release" > artifacts/gh-release-name
