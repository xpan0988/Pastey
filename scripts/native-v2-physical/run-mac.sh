#!/bin/zsh
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$repo_root"
exec cargo run --manifest-path src-tauri/Cargo.toml --bin pastey-native-v2-physical-harness -- "$@"
