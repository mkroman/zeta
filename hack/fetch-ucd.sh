#!/bin/sh
# Fetch the five UCD files a Nomina training run needs.
#
# Usage: hack/fetch-ucd.sh <unicode-version> <dest> [emoji-version]
#
# The emoji version defaults to the Unicode version without its patch component, since the
# standalone Emoji release directories are tagged `<major>.<minor>`; older releases such as
# Unicode 10 use a separate Emoji release (for example `hack/fetch-ucd.sh 10.0.0 /tmp/ucd-10 5.0`).
set -eu

command -v curl >/dev/null 2>&1 || {
    printf 'error: curl is required\n' >&2
    exit 1
}

version=${1:?usage: hack/fetch-ucd.sh <unicode-version> <dest> [emoji-version]}
dest=${2:?usage: hack/fetch-ucd.sh <unicode-version> <dest> [emoji-version]}
emoji_version=${3:-${version%.*}}

public="https://www.unicode.org/Public/$version"
ucd="$public/ucd"
emoji="https://www.unicode.org/Public/emoji/$emoji_version"
mkdir -p "$dest/ucd/extracted" "$dest/emoji"

# Download to `<path>.part` and move into place, so a failed run leaves no partial file at
# the final path.
fetch() {
    url=$1
    path=$2
    if curl -fsSL --retry 3 -o "$path.part" "$url"; then
        mv "$path.part" "$path"
    else
        rm -f "$path.part"
        return 1
    fi
}

fetch "$ucd/extracted/DerivedName.txt" "$dest/ucd/extracted/DerivedName.txt"
fetch "$ucd/extracted/DerivedGeneralCategory.txt" "$dest/ucd/extracted/DerivedGeneralCategory.txt"
fetch "$ucd/NameAliases.txt" "$dest/ucd/NameAliases.txt"
fetch "$ucd/NamedSequences.txt" "$dest/ucd/NamedSequences.txt"

# Newer Unicode releases publish emoji-test under `/Public/<ver>/emoji`, outside the UCD
# tree; older ones only carry it in the standalone emoji release directory.
if ! fetch "$public/emoji/emoji-test.txt" "$dest/emoji/emoji-test.txt"; then
    fetch "$emoji/emoji-test.txt" "$dest/emoji/emoji-test.txt"
fi

for file in \
    "$dest/ucd/extracted/DerivedName.txt" \
    "$dest/ucd/extracted/DerivedGeneralCategory.txt" \
    "$dest/ucd/NameAliases.txt" \
    "$dest/ucd/NamedSequences.txt" \
    "$dest/emoji/emoji-test.txt"
do
    if [ ! -s "$file" ]; then
        printf 'error: %s is missing or empty\n' "$file" >&2
        exit 1
    fi
done

# The first line of DerivedName.txt names its release, e.g. `# DerivedName-18.0.0.txt`;
# anything else means we fetched a different Unicode version than requested.
if ! head -n 1 "$dest/ucd/extracted/DerivedName.txt" | grep -Fq -- "# DerivedName-$version"; then
    printf 'error: DerivedName.txt does not mention Unicode %s\n' "$version" >&2
    exit 1
fi

printf 'Fetched Unicode %s (emoji %s) into %s\n' "$version" "$emoji_version" "$dest"
printf 'Train with:\n  cargo run -p zeta-unicode-build --bin zeta-unicode-train -- \\\n'
printf '    --data-dir %s --out-dir /tmp/nomina-%s --report --verify\n' "$dest" "$version"
