#!/bin/bash

# Run workspace formatting when staged Rust or Cargo manifest files change.
CHANGED_FILES=$(git diff --cached --name-only | grep -E '(^|/)Cargo\.toml$|\.rs$')

if [ -n "$CHANGED_FILES" ]; then
    echo "Checking workspace format for staged Rust files:"
    echo "$CHANGED_FILES"
    cargo fmt --all -- --check
else
    echo "No staged Rust files changed, skipping format check"
    exit 0
fi
