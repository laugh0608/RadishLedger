#!/bin/sh
set -eu
task_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$task_root/apps/m0_probe"
flutter pub get --enforce-lockfile
dart format --output=none --set-exit-if-changed lib hook test integration_test test_driver
flutter analyze --fatal-infos
flutter test
