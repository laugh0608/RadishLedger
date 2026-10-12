#!/bin/sh
set -eu
task_script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
exec python3 -B "$task_script_dir/check-repo.py" --only text "$@"
