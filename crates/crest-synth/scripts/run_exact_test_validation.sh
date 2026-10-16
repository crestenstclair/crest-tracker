#!/usr/bin/env bash
set -u
set -o pipefail

require_one_test() {
  local evidence_file="$1"
  rg -q '^running 1 test$' "$evidence_file"
}

if [[ "${1:-}" == "--self-test" ]]; then
  evidence_file="$(mktemp "${TMPDIR:-/tmp}/crest-zero-selection.XXXXXX")"
  trap 'rm -f "$evidence_file"' EXIT
  {
    echo 'running 0 tests'
    echo 'test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 244 filtered out'
    echo 'CREST_ACCEPTANCE broad-suite passed'
  } >"$evidence_file"
  if require_one_test "$evidence_file"; then
    echo 'zero-selection evidence was incorrectly accepted' >&2
    exit 1
  fi
  echo 'CREST_TEST_VALIDATION zero-selection-rejected passed'
  exit 0
fi

if [[ "$#" -ne 3 ]]; then
  echo 'usage: run_exact_test_validation.sh <target> <exact-test> <marker>' >&2
  exit 2
fi

target="$1"
selector="$2"
marker="$3"
evidence_file="$(mktemp "${TMPDIR:-/tmp}/crest-exact-test.XXXXXX")"
trap 'rm -f "$evidence_file"' EXIT

cargo test --test "$target" "$selector" -- --exact --nocapture 2>&1 | tee "$evidence_file"
test_status=${PIPESTATUS[0]}
if [[ "$test_status" -ne 0 ]]; then
  exit "$test_status"
fi
if ! require_one_test "$evidence_file"; then
  echo "declared selector '$selector' did not execute exactly one test" >&2
  exit 1
fi
if ! rg -Fq "$marker" "$evidence_file"; then
  echo "declared selector '$selector' did not emit '$marker'" >&2
  exit 1
fi

printf 'CREST_TEST_VALIDATION {"target":"%s","selector":"%s","testsExecuted":1}\n' \
  "$target" "$selector"
