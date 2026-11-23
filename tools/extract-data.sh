#!/usr/bin/env bash
# Thin wrapper: the implementation is cross-platform Rust in xtask/ (see xtask/src/). Same arguments and env as before.
set -euo pipefail
cd "$(dirname "$0")/.."
exec cargo xtask extract-data "$@"
