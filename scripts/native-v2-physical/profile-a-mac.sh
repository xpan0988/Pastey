#!/bin/zsh
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
script_name="$0"
runner="$repo_root/scripts/native-v2-physical/run-mac.sh"
app_data_dir="$HOME/pastey-physical/mac-app-data"
report_dir="$HOME/pastey-physical/reports"
bridge_id=""
windows_host_ref=""
run_id=""

usage() {
  print -u2 "usage: $script_name --bridge-id BRIDGE_ID --windows-host-ref HOSTREF_WINDOWS --run-id RUN_ID [--app-data-dir DIR] [--report-dir DIR]"
  exit 2
}

while (( $# > 0 )); do
  case "$1" in
    --bridge-id) (( $# >= 2 )) || usage; bridge_id="$2"; shift 2 ;;
    --windows-host-ref) (( $# >= 2 )) || usage; windows_host_ref="$2"; shift 2 ;;
    --run-id) (( $# >= 2 )) || usage; run_id="$2"; shift 2 ;;
    --app-data-dir) (( $# >= 2 )) || usage; app_data_dir="$2"; shift 2 ;;
    --report-dir) (( $# >= 2 )) || usage; report_dir="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) usage ;;
  esac
done

[[ -n "$bridge_id" && -n "$windows_host_ref" && -n "$run_id" ]] || usage
if (( ${#run_id} > 64 )) || [[ "$run_id" == *[^A-Za-z0-9_-]* ]]; then
  print -u2 "ERROR: --run-id must be 1-64 ASCII letters, digits, hyphens, or underscores"
  exit 2
fi

cd "$repo_root"
if ! worktree_status="$(git status --porcelain=v1 --untracked-files=all)"; then
  print -u2 "ERROR: unable to determine repository worktree status"
  exit 1
fi
if [[ -n "$worktree_status" ]]; then
  print -u2 "ERROR: Profile A requires a clean repository worktree before launch"
  exit 1
fi

attempt_id="physical-native-v2-attempt-${run_id}"
requester_evidence="$report_dir/native-v2-physical-requester-${attempt_id}.json"
windows_evidence_filename="native-v2-physical-windows-host-${attempt_id}.json"
mkdir -p "$report_dir"
if [[ -e "$requester_evidence" ]]; then
  print -u2 "ERROR: requester evidence already exists; choose a fresh --run-id"
  exit 1
fi

"$runner" run --profile a \
  --app-data-dir "$app_data_dir" \
  --bridge-id "$bridge_id" \
  --remote-host-ref "$windows_host_ref" \
  --run-id "$run_id" \
  --report-dir "$report_dir"

print "REQUESTER_EVIDENCE_JSON=$requester_evidence"
print "EXPECTED_WINDOWS_EVIDENCE_FILENAME=$windows_evidence_filename"
