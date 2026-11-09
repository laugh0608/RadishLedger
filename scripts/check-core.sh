#!/bin/sh
set -eu
task_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$task_root"
cargo fmt --all -- --check
cargo test --locked --workspace --all-features --jobs 4
cargo clippy --locked --workspace --all-targets --all-features --jobs 4 -- -D warnings
clang -std=c11 -Wall -Wextra -Werror -pedantic -fsyntax-only -x c crates/ledger-mobile/include/radishledger_probe.h
