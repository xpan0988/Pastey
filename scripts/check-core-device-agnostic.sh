#!/usr/bin/env bash
# Report device vocabulary in Pastey physical code, split into two columns:
#   core     src-tauri/src/physical, excluding adapters/, bindings/ and tests
#   binding  src-tauri/src/physical/adapters/, src-tauri/src/physical/bindings/
#            and the top-level bindings/ crates (tests excluded)
# Core may only understand opaque IDs, schema digests, bounded dimensions,
# fingerprints and witness classes; the binding column is informational.
# Usage: check-core-device-agnostic.sh [--strict]
#   default   print Core matches and both counts, always exit 0
#   --strict  additionally exit 1 when the Core count is non-zero
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
physical="$root/src-tauri/src/physical"
pattern='microduck|duck|vx_|vyaw|twist|stand|walk|velstand|onnx|mujoco|policy_sha|fallen|settl|yaw'

strict=0
if [[ "${1:-}" == "--strict" ]]; then
  strict=1
elif [[ $# -gt 0 ]]; then
  echo "usage: $0 [--strict]" >&2
  exit 2
fi

# Prints matching lines ("path:line:text") for the given files.
matches() {
  if [[ $# -gt 0 ]]; then
    grep -n -i -E "$pattern" "$@" | sed "s|^$root/||" || true
  fi
}
count() {
  if [[ -n "$1" ]]; then printf '%s\n' "$1" | wc -l | tr -d ' '; else echo 0; fi
}
rust_files() {
  find "$@" -type f -name '*.rs' -not -name 'tests.rs' -not -name '*_tests.rs' \
    -not -path '*/tests/*' -print0 2>/dev/null | sort -z
}

core_files=()
while IFS= read -r -d '' f; do core_files+=("$f"); done < <(
  find "$physical" -type f -name '*.rs' \
    -not -path "$physical/adapters/*" -not -path "$physical/bindings/*" \
    -not -name 'tests.rs' -not -name '*_tests.rs' -print0 | sort -z)

binding_dirs=()
for d in "$physical/adapters" "$physical/bindings" "$root/bindings"; do
  [[ -d "$d" ]] && binding_dirs+=("$d")
done
binding_files=()
if [[ ${#binding_dirs[@]} -gt 0 ]]; then
  while IFS= read -r -d '' f; do binding_files+=("$f"); done < <(rust_files "${binding_dirs[@]}")
fi

core="$(matches "${core_files[@]+"${core_files[@]}"}")"
binding="$(matches "${binding_files[@]+"${binding_files[@]}"}")"

if [[ -n "$core" ]]; then
  echo "Core matches:"
  printf '%s\n' "$core"
  echo
fi
printf '%-58s %6s %8s\n' "file" "core" "binding"
{
  [[ -n "$core" ]] && printf '%s\n' "$core" | cut -d: -f1 | sort | uniq -c | awk '{print $2, $1, 0}'
  [[ -n "$binding" ]] && printf '%s\n' "$binding" | cut -d: -f1 | sort | uniq -c | awk '{print $2, 0, $1}'
} | sort -k2,2nr -k3,3nr | awk '{printf "%-58s %6s %8s\n", $1, ($2 ? $2 : "-"), ($3 ? $3 : "-")}'

core_hits="$(count "$core")"
binding_hits="$(count "$binding")"
echo "core-device-agnostic hits: core=$core_hits binding=$binding_hits"
if [[ $strict -eq 1 && $core_hits -ne 0 ]]; then
  exit 1
fi
