# Homebrew Tap Setup and Maintenance

This document explains how to set up and maintain the Homebrew tap for tsql.

## Overview

A Homebrew "tap" is a GitHub repository containing formula files. Users can install tsql with:

```bash
brew tap fcoury/tap
brew install tsql
```

Or in one command:

```bash
brew install fcoury/tap/tsql
```

## Initial Setup

### Step 1: Create the Tap Repository

1. Go to GitHub and create a new repository named `tap`
   - URL: `https://github.com/fcoury/tap`
   - Make it public
   - Initialize with a README (optional)

2. Clone the repository locally:
   ```bash
   git clone git@github.com:fcoury/tap.git
   cd tap
   ```

3. Create the Formula directory:
   ```bash
   mkdir Formula
   ```

### Step 2: Create the Formula File

Create `Formula/tsql.rb` with the following content:

```ruby
class Tsql < Formula
  desc "A modern, keyboard-first PostgreSQL CLI"
  homepage "https://github.com/fcoury/tsql"
  version "0.1.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/fcoury/tsql/releases/download/v#{version}/tsql-aarch64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_AARCH64_MACOS_SHA256"
    else
      url "https://github.com/fcoury/tsql/releases/download/v#{version}/tsql-x86_64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_X86_64_MACOS_SHA256"
    end
  end

  on_linux do
    url "https://github.com/fcoury/tsql/releases/download/v#{version}/tsql-x86_64-unknown-linux-gnu.tar.gz"
    sha256 "REPLACE_WITH_LINUX_SHA256"
  end

  def install
    bin.install "tsql"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/tsql --version")
  end
end
```

### Step 3: Get SHA256 Checksums

After a release is published, download the checksums:

```bash
# Download the SHA256SUMS.txt from the release
curl -LO https://github.com/fcoury/tsql/releases/download/v0.1.0/SHA256SUMS.txt
cat SHA256SUMS.txt
```

Or calculate them manually:

```bash
# Download each tarball and calculate SHA256
curl -LO https://github.com/fcoury/tsql/releases/download/v0.1.0/tsql-aarch64-apple-darwin.tar.gz
shasum -a 256 tsql-aarch64-apple-darwin.tar.gz

curl -LO https://github.com/fcoury/tsql/releases/download/v0.1.0/tsql-x86_64-apple-darwin.tar.gz
shasum -a 256 tsql-x86_64-apple-darwin.tar.gz

curl -LO https://github.com/fcoury/tsql/releases/download/v0.1.0/tsql-x86_64-unknown-linux-gnu.tar.gz
shasum -a 256 tsql-x86_64-unknown-linux-gnu.tar.gz
```

### Step 4: Update the Formula with Real Checksums

Replace the placeholder `sha256` values with the actual checksums from Step 3.

### Step 5: Commit and Push

```bash
git add Formula/tsql.rb
git commit -m "Add tsql formula v0.1.0"
git push origin main
```

### Step 6: Test the Installation

```bash
# Tap the repository (only needed once)
brew tap fcoury/tap

# Install tsql
brew install tsql

# Verify it works
tsql --version
```

## Updating for New Releases

After each new release:

### Step 1: Get New Checksums

```bash
VERSION=0.2.0  # New version

# Download checksums from release
curl -LO https://github.com/fcoury/tsql/releases/download/v${VERSION}/SHA256SUMS.txt
cat SHA256SUMS.txt
```

### Step 2: Update the Formula

Edit `Formula/tsql.rb`:

1. Update the `version` line
2. Update all `sha256` values

```ruby
version "0.2.0"  # Updated version

on_macos do
  if Hardware::CPU.arm?
    url "https://github.com/fcoury/tsql/releases/download/v#{version}/tsql-aarch64-apple-darwin.tar.gz"
    sha256 "new_aarch64_sha256_here"
  else
    url "https://github.com/fcoury/tsql/releases/download/v#{version}/tsql-x86_64-apple-darwin.tar.gz"
    sha256 "new_x86_64_macos_sha256_here"
  end
end

on_linux do
  url "https://github.com/fcoury/tsql/releases/download/v#{version}/tsql-x86_64-unknown-linux-gnu.tar.gz"
  sha256 "new_linux_sha256_here"
end
```

### Step 3: Commit and Push

```bash
git add Formula/tsql.rb
git commit -m "Update tsql to v0.2.0"
git push origin main
```

### Step 4: Verify Update

```bash
# Upgrade tsql
brew upgrade tsql

# Or reinstall if needed
brew reinstall tsql

# Verify version
tsql --version
```

## Testing Locally Before Pushing

You can test the formula locally before pushing:

```bash
# Install from local formula file
brew install --build-from-source ./Formula/tsql.rb

# Or audit the formula for issues
brew audit --strict Formula/tsql.rb
```

## Troubleshooting

### "sha256 mismatch" Error

**Problem:** Homebrew reports SHA256 mismatch during install

**Solution:**
1. Re-download the tarball and recalculate the checksum
2. Ensure you're using the checksum from the correct release version
3. Update the formula with the correct checksum

### Formula Audit Warnings

**Problem:** `brew audit` reports issues

**Solution:**
- Fix any issues reported by `brew audit --strict Formula/tsql.rb`
- Common issues: missing desc, homepage, or license

### Binary Not Working After Install

**Problem:** Installed binary doesn't run

**Solution:**
1. Check that the binary name in the tarball matches what's in `bin.install`
2. Ensure the tarball structure is correct (binary at root level)
3. Verify the binary is compiled for the correct architecture

## Future: Automating Updates

You can automate formula updates using GitHub Actions. Create a workflow in the `tap` repository that:

1. Triggers on releases in the main `tsql` repository
2. Downloads the SHA256SUMS.txt
3. Updates the formula file
4. Commits and pushes

Example workflow (to be added to `tap/.github/workflows/update-formula.yml`):

```yaml
name: Update Formula

on:
  repository_dispatch:
    types: [release]
  workflow_dispatch:
    inputs:
      version:
        description: 'Version to update to (e.g., 0.2.0)'
        required: true

jobs:
  update:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      
      - name: Download checksums
        run: |
          VERSION=${{ github.event.inputs.version || github.event.client_payload.version }}
          curl -LO https://github.com/fcoury/tsql/releases/download/v${VERSION}/SHA256SUMS.txt
          
      - name: Update formula
        run: |
          # Script to update version and checksums in Formula/tsql.rb
          # ... (implementation details)
          
      - name: Commit and push
        run: |
          git config user.name "GitHub Actions"
          git config user.email "actions@github.com"
          git add Formula/tsql.rb
          git commit -m "Update tsql to v${VERSION}"
          git push
```

This automation is optional and can be set up after the initial manual process is working.

## Reference

- [Homebrew Formula Cookbook](https://docs.brew.sh/Formula-Cookbook)
- [Homebrew Taps](https://docs.brew.sh/Taps)
- [Homebrew Acceptable Formulae](https://docs.brew.sh/Acceptable-Formulae)
