#!/bin/sh
# Fetch the five UCD files a Nomina training run needs.
#
# Usage: hack/fetch-ucd.sh <unicode-version> <dest> [emoji-version]
#
# `<dest>` is a scratch directory. The script writes an extracted-release tree (`ucd/...` and
# `emoji/...`) and refuses to run when an earlier candidate path already exists in `<dest>`,
# because the trainer probes those first and would silently keep training the old release. To
# update the vendored `crates/zeta-unicode/data`, copy the five files into it instead.
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
if ! printf '%s\n' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
    printf 'error: Unicode version must be <major>.<minor>.<patch>, got %s\n' "$version" >&2
    exit 1
fi
emoji_version=${3:-${version%.*}}
if ! printf '%s\n' "$emoji_version" | grep -Eq '^[0-9]+\.[0-9]+$'; then
    printf 'error: Emoji version must be <major>.<minor>, got %s\n' "$emoji_version" >&2
    exit 1
fi

# Every path the trainer probes before the file written below is a stale-data hazard: it wins
# the probe, so training would read the old release while this script reports success. The
# order mirrors the `InputSpec` candidates in `zeta-unicode-build`, up to the written file.
for path in \
    DerivedName.txt \
    extracted/DerivedName.txt \
    ucd/DerivedName.txt \
    DerivedGeneralCategory.txt \
    extracted/DerivedGeneralCategory.txt \
    ucd/DerivedGeneralCategory.txt \
    NameAliases.txt \
    extracted/NameAliases.txt \
    NamedSequences.txt \
    extracted/NamedSequences.txt \
    emoji-test.txt
do
    if [ -e "$dest/$path" ]; then
        printf 'error: %s already exists; %s must be a scratch directory\n' \
            "$dest/$path" "$dest" >&2
        printf '       The trainer probes that path before the fetched file, so training\n' >&2
        printf '       would keep using it. Update the vendored data by copying the five UCD\n' >&2
        printf '       files instead.\n' >&2
        exit 1
    fi
done

public="https://www.unicode.org/Public/$version"
ucd="$public/ucd"
emoji="https://www.unicode.org/Public/emoji/$emoji_version"
mkdir -p "$dest/ucd/extracted" "$dest/emoji"

# Download to `<path>.part` and move into place, so a failed run leaves no partial file at
# the final path.
fetch() {
    url=$1
    path=$2
    if curl -fsSL --retry 3 --connect-timeout 15 --max-time 300 --proto '=https' \
        -o "$path.part" "$url"
    then
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

# The first lines of DerivedName.txt name its release, e.g. `# DerivedName-18.0.0.txt`;
# anything else means we fetched a different Unicode version than requested. A BOM or a
# leading comment line must not make a valid download look wrong, so search the header.
if ! grep -m1 -Fq -- "# DerivedName-$version" "$dest/ucd/extracted/DerivedName.txt"; then
    printf 'error: DerivedName.txt does not mention Unicode %s\n' "$version" >&2
    exit 1
fi

# `emoji-test.txt` advertises its Emoji release on a `# Version: <major>.<minor>` line; a
# mismatched header means the fallback URL served a different release than requested.
if ! grep -m1 -Eq -- "^# Version: ${emoji_version}[[:space:]]*\$" "$dest/emoji/emoji-test.txt"; then
    printf 'error: emoji-test.txt does not mention Emoji %s\n' "$emoji_version" >&2
    exit 1
fi

printf 'Fetched Unicode %s (emoji %s) into %s\n' "$version" "$emoji_version" "$dest"
printf 'Train with:\n  cargo run -p zeta-unicode-build --bin zeta-unicode-train -- \\\n'
printf '    --data-dir %s --out-dir /tmp/nomina-%s --report --verify\n' "$dest" "$version"
