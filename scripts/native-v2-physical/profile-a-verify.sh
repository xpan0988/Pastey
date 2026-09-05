#!/bin/zsh
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
script_name="$0"
runner="$repo_root/scripts/native-v2-physical/run-mac.sh"
report_dir="$HOME/pastey-physical/reports"
run_id=""

usage() {
  print -u2 "usage: $script_name --run-id RUN_ID [--report-dir DIR]"
  exit 2
}

while (( $# > 0 )); do
  case "$1" in
    --run-id) (( $# >= 2 )) || usage; run_id="$2"; shift 2 ;;
    --report-dir) (( $# >= 2 )) || usage; report_dir="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) usage ;;
  esac
done

[[ -n "$run_id" ]] || usage
if (( ${#run_id} > 64 )) || [[ "$run_id" == *[^A-Za-z0-9_-]* ]]; then
  print -u2 "ERROR: --run-id must be 1-64 ASCII letters, digits, hyphens, or underscores"
  exit 2
fi

attempt_id="physical-native-v2-attempt-${run_id}"
requester_evidence="$report_dir/native-v2-physical-requester-${attempt_id}.json"
windows_evidence="$report_dir/native-v2-physical-windows-host-${attempt_id}.json"

"$runner" verify --profile a \
  --requester-report "$requester_evidence" \
  --windows-report "$windows_evidence" \
  --output-dir "$report_dir"
