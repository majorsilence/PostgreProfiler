#!/usr/bin/env bash
# Run the pg_regress suite with pg_profiler preloaded.
#   scripts/regress.sh [17|18] [extra `cargo pgrx regress` args, e.g. --add name]
# `cargo pgrx regress` starts the pgrx-managed server, which only preloads the
# library if it is listed in that server's postgresql.conf, so add it first.
set -euo pipefail
v="${1:-18}"
shift || true
conf="$HOME/.pgrx/data-$v/postgresql.conf"
grep -q "shared_preload_libraries.*pg_profiler" "$conf" \
  || echo "shared_preload_libraries = 'pg_profiler'" >> "$conf"
exec cargo pgrx regress "pg$v" "$@"
