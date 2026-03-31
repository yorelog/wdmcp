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
# Sync version into Cargo.toml and skill.json
# ---------------------------------------------------------------------------
CARGO_VERSION=$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)
SKILL_VERSION=$(node -p "require('./skill.json').version")

DIRTY=0

if [ "$CARGO_VERSION" != "$VERSION" ]; then
  echo "🔄 Updating Cargo.toml: ${CARGO_VERSION} → ${VERSION}"
  sed -i.bak "s/^version *= *\".*\"/version = \"${VERSION}\"/" Cargo.toml && rm -f Cargo.toml.bak
  DIRTY=1
fi

if [ "$SKILL_VERSION" != "$VERSION" ]; then
  echo "🔄 Updating skill.json: ${SKILL_VERSION} → ${VERSION}"
  node -e "
    const fs = require('fs');
    const p = './skill.json';
    const j = JSON.parse(fs.readFileSync(p, 'utf8'));
    j.version = '${VERSION}';
    fs.writeFileSync(p, JSON.stringify(j, null, 2) + '\n');
  "
  DIRTY=1
fi

# ---------------------------------------------------------------------------
# Commit synced version files if anything changed
# ---------------------------------------------------------------------------
if [ "$DIRTY" -eq 1 ]; then
  echo "📝 Committing version sync"
  git add Cargo.toml skill.json
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
# Create tag and push
# ---------------------------------------------------------------------------
echo "🏷️  Creating tag ${TAG}"
git tag -a "$TAG" -m "Release ${TAG}"

echo "🚀 Pushing tag ${TAG} to origin"
git push origin "$TAG"

echo ""
echo "✅ Released ${TAG}"
echo "   GitHub Actions will now build binaries and publish to npm."
