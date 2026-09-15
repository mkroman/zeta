#!/bin/sh
# Strip a release binary while archiving its symbols for offline symbolization.
#
# On Linux the symbols and line tables are split into a sibling `<binary>.debug` file and
# referenced from the stripped binary with a `.gnu_debuglink` section, so debuggers find them
# automatically when both files live in the same directory. On macOS the unstripped binary is
# kept as the debug artifact and the shipped binary has its DWARF and local symbols removed.
#
# Usage: strip-release.sh <binary> [debug-file]
set -eu

binary="${1:?usage: strip-release.sh <binary> [debug-file]}"
debug="${2:-${binary}.debug}"

case "$(uname -s)" in
  Linux)
    objcopy --only-keep-debug --compress-debug-sections=zlib "${binary}" "${debug}"
    strip --strip-all "${binary}"
    objcopy --add-gnu-debuglink="${debug}" "${binary}"
    ;;
  Darwin)
    cp "${binary}" "${debug}"
    strip -S -x "${binary}"
    ;;
  *)
    echo "unsupported platform: $(uname -s)" >&2
    exit 1
    ;;
esac

echo "stripped ${binary}; symbols archived in ${debug}" >&2
