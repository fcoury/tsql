# Releasing tsql

This document describes how to create a new release of tsql.

## Overview

When you push a Git tag matching `v*.*.*` (e.g., `v0.1.0`), the release workflow automatically:

1. Builds binaries for all supported platforms
2. Creates a GitHub Release with the binaries
3. Publishes both crates to crates.io

## Prerequisites

### 1. Set up CARGO_REGISTRY_TOKEN

Before your first release, you need to add your crates.io API token as a GitHub secret:

1. Go to [crates.io/settings/tokens](https://crates.io/settings/tokens)
2. Create a new token with **publish-update** scope
3. Go to your GitHub repository → Settings → Secrets and variables → Actions
4. Click "New repository secret"
5. Name: `CARGO_REGISTRY_TOKEN`
6. Value: paste your crates.io token
7. Click "Add secret"

### 2. Ensure CI Passes

Before releasing, make sure the `main` branch CI is green.

## Release Process

### Step 1: Update Version Numbers

Update the version in `Cargo.toml`:

```toml
[workspace.package]
version = "0.2.0"  # New version
```

Both `tsql` and `tui-syntax` inherit this version from the workspace.

### Step 2: Update CHANGELOG (Optional)

If you maintain a CHANGELOG.md, update it with the changes in this release.

### Step 3: Commit the Version Bump

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: bump version to 0.2.0"
git push origin main
```

### Step 4: Create and Push the Tag

```bash
# Create an annotated tag
git tag -a v0.2.0 -m "Release v0.2.0"

# Push the tag to trigger the release
git push origin v0.2.0
```

### Step 5: Monitor the Release

1. Go to the [Actions tab](https://github.com/fcoury/tsql/actions) on GitHub
2. Watch the "Release" workflow
3. Once complete, check the [Releases page](https://github.com/fcoury/tsql/releases)

## What Happens Automatically

### Build Job

For each platform in the matrix:
- Checks out the code
- Sets up Rust with the target toolchain
- Builds a release binary
- Creates a tarball (Unix) or zip (Windows)
- Uploads as a workflow artifact

**Platforms built:**
| Target | Archive |
|--------|---------|
| `x86_64-unknown-linux-gnu` | `tsql-x86_64-unknown-linux-gnu.tar.gz` |
| `x86_64-apple-darwin` | `tsql-x86_64-apple-darwin.tar.gz` |
| `aarch64-apple-darwin` | `tsql-aarch64-apple-darwin.tar.gz` |
| `x86_64-pc-windows-msvc` | `tsql-x86_64-pc-windows-msvc.zip` |

### Release Job

- Downloads all build artifacts
- Generates SHA256 checksums
- Creates a GitHub Release with auto-generated notes
- Uploads all binaries and checksums

### Publish Job

- Publishes `tui-syntax` to crates.io
- Waits 30 seconds for crates.io to index
- Publishes `tsql` to crates.io

## Post-Release Tasks

### Update Homebrew Tap

After a successful release, update the Homebrew formula:

1. Download the new release tarballs
2. Calculate SHA256 checksums
3. Update `Formula/tsql.rb` in the `tap` repository
4. See [HOMEBREW.md](HOMEBREW.md) for detailed instructions

## Pre-release Versions

For testing the release process, you can create pre-release tags:

```bash
git tag -a v0.2.0-rc1 -m "Release candidate 1"
git push origin v0.2.0-rc1
```

Pre-release tags (containing `-rc`, `-beta`, or `-alpha`) are marked as pre-releases on GitHub.

## Troubleshooting

### Build Failures

**Problem:** Build fails on a specific platform

**Solution:** 
1. Check the Actions log for the specific error
2. Common issues: missing system dependencies, Rust version mismatch
3. Fix the issue and push a new tag (e.g., `v0.2.1`)

### crates.io Publish Fails

**Problem:** `cargo publish` fails with authentication error

**Solution:**
1. Verify `CARGO_REGISTRY_TOKEN` secret is set correctly
2. Check that the token hasn't expired
3. Ensure the token has `publish-update` scope

**Problem:** `cargo publish` fails with "crate already exists"

**Solution:**
- This usually means you're trying to republish the same version
- Increment the version number and create a new tag

**Problem:** `tsql` publish fails because `tui-syntax` isn't available yet

**Solution:**
- The workflow waits 30 seconds between publishes
- If it's still failing, manually trigger the workflow again or wait longer

### Release Not Appearing

**Problem:** GitHub Release not created

**Solution:**
1. Check that the tag matches `v*.*.*` pattern
2. Verify the release workflow completed successfully
3. Check Actions logs for errors

## Version Numbering

We follow [Semantic Versioning](https://semver.org/):

- **MAJOR** (1.0.0): Breaking changes
- **MINOR** (0.1.0): New features, backwards compatible
- **PATCH** (0.0.1): Bug fixes, backwards compatible

## Rolling Back a Release

If you need to roll back:

1. Delete the GitHub Release (does not delete the tag)
2. Yank the crates.io version: `cargo yank --version 0.2.0 tsql`
3. Create a new fixed release with an incremented version
