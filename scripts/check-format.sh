#!/bin/bash

# Get list of staged server files that need formatting check
CHANGED_FILES=$(git diff --cached --name-only | grep -E "^api/.*\.rs$")

if [ -n "$CHANGED_FILES" ]; then
    echo "Checking format for changed rust server files:"
    echo "$CHANGED_FILES"
    cd api && cargo fmt -- --check
else
    echo "No server files changed, skipping format check"
    exit 0
fi


