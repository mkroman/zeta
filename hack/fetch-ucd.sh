#!/bin/sh
# Fetch the five UCD files a Nomina training run needs.
#
# Usage: hack/fetch-ucd.sh <unicode-version> <dest> [emoji-version]
#
# The emoji version defaults to the Unicode version; older releases such as Unicode 10 use a
# separate Emoji release (for example `hack/fetch-ucd.sh 10.0.0 /tmp/ucd-10 5.0`).
set -eu

version=${1:?usage: hack/fetch-ucd.sh <unicode-version> <dest> [emoji-version]}
dest=${2:?usage: hack/fetch-ucd.sh <unicode-version> <dest> [emoji-version]}
emoji_version=${3:-$version}

base="https://www.unicode.org/Public/$version/ucd"
mkdir -p "$dest/ucd/extracted" "$dest/emoji"

fetch() {
    url=$1
    path=$2
    curl -fsSL --retry 3 -o "$path" "$url"
}

fetch "$base/extracted/DerivedName.txt" "$dest/ucd/extracted/DerivedName.txt"
fetch "$base/extracted/DerivedGeneralCategory.txt" "$dest/ucd/extracted/DerivedGeneralCategory.txt"
fetch "$base/NameAliases.txt" "$dest/ucd/NameAliases.txt"
fetch "$base/NamedSequences.txt" "$dest/ucd/NamedSequences.txt"

# Newer UCD releases publish emoji-test under the UCD tree; older ones use the standalone
# emoji release directory.
if ! curl -fsSL --retry 3 -o "$dest/emoji/emoji-test.txt" "$base/emoji/emoji-test.txt" 2>/dev/null; then
    fetch "https://www.unicode.org/Public/emoji/$emoji_version/emoji-test.txt" \
        "$dest/emoji/emoji-test.txt"
fi

printf 'Fetched Unicode %s (emoji %s) into %s\n' "$version" "$emoji_version" "$dest"
printf 'Train with:\n  cargo run -p zeta-unicode-build --bin zeta-unicode-train -- \\\n'
printf '    --data-dir %s --out-dir /tmp/nomina-%s --report --verify\n' "$dest" "$version"
