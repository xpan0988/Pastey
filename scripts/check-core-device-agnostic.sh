#!/usr/bin/env bash
# Report device vocabulary that leaks into Pastey physical Core.
#
# Core may only understand opaque IDs, schema digests, bounded dimensions,
# fingerprints and witness classes. Device bindings (bindings/) and test files
# are excluded. Usage: check-core-device-agnostic.sh [--strict]
#   default   print matches and the hit count, always exit 0
#   --strict  additionally exit 1 when the hit count is non-zero
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
core="$root/src-tauri/src/physical"
pattern='microduck|duck|vx_|vyaw|twist|stand|walk|velstand|onnx|mujoco|policy_sha|fallen|settl|yaw'

strict=0
if [[ "${1:-}" == "--strict" ]]; then
  strict=1
elif [[ $# -gt 0 ]]; then
  echo "usage: $0 [--strict]" >&2
  exit 2
fi

files=()
while IFS= read -r -d '' file; do
  files+=("$file")
done < <(find "$core" -type f -name '*.rs' \
  -not -path "$core/bindings/*" \
  -not -name 'tests.rs' \
  -not -name '*_tests.rs' \
  -print0 | sort -z)

matches=""
if [[ ${#files[@]} -gt 0 ]]; then
  matches="$(grep -n -i -E "$pattern" "${files[@]}" || true)"
fi

if [[ -n "$matches" ]]; then
  hits="$(printf '%s\n' "$matches" | wc -l | tr -d ' ')"
  printf '%s\n' "$matches" | sed "s|^$root/||"
  echo
  echo "Per file:"
  printf '%s\n' "$matches" | sed "s|^$root/||" | cut -d: -f1 | sort | uniq -c | sort -rn
else
  hits=0
fi

echo "core-device-agnostic hits: $hits"
if [[ $strict -eq 1 && $hits -ne 0 ]]; then
  exit 1
fi
