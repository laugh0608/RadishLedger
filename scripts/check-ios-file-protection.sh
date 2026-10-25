#!/bin/sh
set -eu
task_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
task_tmp=$(mktemp -d "${TMPDIR:-/tmp}/radishledger-file-protection.XXXXXX")
trap 'rm -rf -- "$task_tmp"' EXIT HUP INT TERM
xcrun swiftc -module-cache-path "$task_tmp/module-cache" \
  "$task_root/apps/m0_probe/ios/Runner/M0FileProtectionProbe.swift" \
  "$task_root/scripts/tests/m0_file_protection_test.swift" -o "$task_tmp/probe-test"
M0_FILE_PROTECTION_TEST_ROOT="$task_tmp" "$task_tmp/probe-test"
