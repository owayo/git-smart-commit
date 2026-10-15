#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
CARGO_TOML="$PROJECT_ROOT/Cargo.toml"

# Cargo.toml から現在のバージョンを取得する。
CURRENT_VERSION=$(grep '^version = ' "$CARGO_TOML" | head -1 | sed 's/version = "\(.*\)"/\1/')

if [ -z "$CURRENT_VERSION" ]; then
    echo "Error: Could not find version in Cargo.toml"
    exit 1
fi

# バージョンを major / minor / patch に分ける。
IFS='.' read -r MAJOR MINOR PATCH <<< "$CURRENT_VERSION"

# 更新する桁を選ぶ（既定は patch）。
BUMP_TYPE="${1:-patch}"

case "$BUMP_TYPE" in
    major)
        MAJOR=$((MAJOR + 1))
        MINOR=0
        PATCH=0
        ;;
    minor)
        MINOR=$((MINOR + 1))
        PATCH=0
        ;;
    patch)
        PATCH=$((PATCH + 1))
        ;;
    *)
        echo "Usage: $0 [major|minor|patch]"
        echo "  major: 0.1.0 -> 1.0.0"
        echo "  minor: 0.1.0 -> 0.2.0"
        echo "  patch: 0.1.0 -> 0.1.1 (default)"
        exit 1
        ;;
esac

NEW_VERSION="$MAJOR.$MINOR.$PATCH"

echo "📦 Updating version: $CURRENT_VERSION → $NEW_VERSION"

# Cargo.toml のバージョンを更新する。
if [[ "$OSTYPE" == "darwin"* ]]; then
    # macOS
    sed -i '' "s/^version = \"$CURRENT_VERSION\"/version = \"$NEW_VERSION\"/" "$CARGO_TOML"
else
    # Linux
    sed -i "s/^version = \"$CURRENT_VERSION\"/version = \"$NEW_VERSION\"/" "$CARGO_TOML"
fi

echo "✅ Updated Cargo.toml"
echo "🎉 Version updated to $NEW_VERSION"
