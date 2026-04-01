#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# ---------------------------------------------------------------------------
# Read the version from package.json (source of truth)
# ---------------------------------------------------------------------------
VERSION=$(node -p "require('./package.json').version")

if [ -z "$VERSION" ]; then
  echo "❌ Could not read version from package.json"
  exit 1
fi

TAG="v${VERSION}"

echo "📦 Version from package.json: ${VERSION}"

# ---------------------------------------------------------------------------
# Sync version into Cargo.toml and SKILL.md
# ---------------------------------------------------------------------------
CARGO_VERSION=$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)
SKILL_VERSION=$(sed -n 's/^version: *"\{0,1\}\([^"]*\)"\{0,1\}/\1/p' SKILL.md | head -1)

DIRTY=0

if [ "$CARGO_VERSION" != "$VERSION" ]; then
  echo "🔄 Updating Cargo.toml: ${CARGO_VERSION} → ${VERSION}"
  sed -i.bak "s/^version *= *\".*\"/version = \"${VERSION}\"/" Cargo.toml && rm -f Cargo.toml.bak
  DIRTY=1
fi

if [ "$SKILL_VERSION" != "$VERSION" ]; then
  echo "🔄 Updating SKILL.md: ${SKILL_VERSION} → ${VERSION}"
  sed -i.bak "s/^version: *.*$/version: \"${VERSION}\"/" SKILL.md && rm -f SKILL.md.bak
  DIRTY=1
fi

# ---------------------------------------------------------------------------
# Commit synced version files if anything changed
# ---------------------------------------------------------------------------
if [ "$DIRTY" -eq 1 ]; then
  echo "📝 Committing version sync"
  git add Cargo.toml SKILL.md
  git commit -m "chore: sync version to ${VERSION}"
fi

# ---------------------------------------------------------------------------
# Check if tag already exists
# ---------------------------------------------------------------------------
if git rev-parse "$TAG" >/dev/null 2>&1; then
  echo "❌ Tag ${TAG} already exists. Bump the version in package.json first."
  exit 1
fi

# ---------------------------------------------------------------------------
# Ensure working tree is clean
# ---------------------------------------------------------------------------
if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "❌ Working tree is dirty. Commit or stash changes before releasing."
  exit 1
fi

# ---------------------------------------------------------------------------
# Extract release notes from CHANGELOG.md for this version
#
# Looks for the section between ## [<VERSION>] and the next ## heading (or EOF).
# Falls back to a generic message if the section is missing.
# ---------------------------------------------------------------------------
NOTES=""
if [ -f CHANGELOG.md ]; then
  # Extract lines between "## [VERSION]" and the next "## [" heading
  NOTES=$(awk -v ver="$VERSION" '
    BEGIN { found=0 }
    /^## \[/ {
      if (found) exit
      if (index($0, "[" ver "]")) { found=1; next }
    }
    found { print }
  ' CHANGELOG.md | sed -e '/./,$!d' -e :a -e '/^\s*$/{ $d; N; ba; }')
fi

if [ -z "$NOTES" ]; then
  echo "⚠️  No CHANGELOG.md entry found for [${VERSION}], using default message"
  NOTES="Release ${TAG}"
fi

echo ""
echo "📋 Release notes:"
echo "---"
echo "$NOTES"
echo "---"
echo ""

# ---------------------------------------------------------------------------
# Create annotated tag with release notes and push
# ---------------------------------------------------------------------------
echo "🏷️  Creating tag ${TAG}"
git tag -a "$TAG" -m "Release ${TAG}" -m "$NOTES"

echo "🚀 Pushing commit(s) and tag ${TAG} to origin"
git push origin HEAD
git push origin "$TAG"

echo ""
echo "✅ Released ${TAG}"
echo "   GitHub Actions will now build binaries and publish to npm."
