#!/bin/bash

# Get list of staged server files that need clippy check
CHANGED_FILES=$(git diff --cached --name-only | grep -E "^api/.*\.rs$")

if [ -n "$CHANGED_FILES" ]; then
    echo "Running clippy with autofix for changed rust server files:"
    echo "$CHANGED_FILES"
    cd api && cargo clippy --fix --allow-dirty --allow-staged -- -D warnings && cargo clippy -- -D warnings
else
    echo "No server files changed, skipping clippy check"
    exit 0
fi
