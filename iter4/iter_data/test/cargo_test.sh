#!/usr/bin/env bash
exec "$(dirname "$0")/../../tools/cargo-test-crate.sh" iter_data
