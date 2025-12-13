# feat: Docker-Based Test Isolation for Local Development

**Created**: 2025-12-13
**Status**: Draft
**Type**: Enhancement

## Overview

Add Docker-based test isolation for running integration tests on developer machines, providing complete environment isolation without requiring manual PostgreSQL installation.

## Problem Statement

Currently, running integration tests requires:
1. Manual PostgreSQL installation on the developer's machine
2. Configuration via `TEST_DATABASE_URL` environment variable
3. User must have `CREATEDB` privilege

This creates friction for:
- New contributors (installation barrier)
- Cross-platform consistency (different PostgreSQL versions/configs)
- Reproducibility (works on my machine issues)

The CI pipeline already uses Docker (PostgreSQL service containers in GitHub Actions), but local development lacks this convenience.

## Proposed Solution

**Recommended Approach: Hybrid (Docker Compose + Existing TestDatabase)**

After analyzing three approaches (testcontainers-rs, Docker Compose, Hybrid), the **Hybrid approach** provides the best balance:

| Aspect | testcontainers-rs | Docker Compose | **Hybrid (Recommended)** |
|--------|------------------|----------------|--------------------------|
| Startup overhead | 3-5s per test run | Manual `docker compose up` | Near-zero after first start |
| TDD workflow | Poor (slow) | Good (persistent) | Excellent |
| Learning curve | Low | Medium | Low |
| Complexity | Simple | Medium | Low |
| CI alignment | Different from CI | Can match CI | Matches CI |
| Backward compat | Breaking | Compatible | Compatible |

### How It Works

1. **Developer starts PostgreSQL once**: `docker compose up -d postgres-test`
2. **Tests run normally**: `cargo test` (no changes needed)
3. **Existing TestDatabase helper**: Continues working unchanged
4. **Auto-detection**: Tests gracefully skip if no database available

### Key Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Container lifecycle | **Persistent** (manual start/stop) | Optimizes for TDD workflow (50-100 test runs/hour) |
| Docker requirement | **Optional** with fallback | Maintains backward compatibility with `TEST_DATABASE_URL` |
| Port management | **Static 5433** | Avoids conflict with local PostgreSQL on 5432 |
| CI strategy | **Keep existing** | Service containers work well, no change needed |
| API compatibility | **Non-breaking** | TestDatabase works unchanged |

## Technical Approach

### Files to Create

#### 1. `docker-compose.test.yml`

```yaml
# Docker Compose configuration for local test environment
# Usage: docker compose -f docker-compose.test.yml up -d

services:
  postgres-test:
    image: postgres:16-alpine
    container_name: tsql-test-postgres
    environment:
      POSTGRES_USER: postgres
      POSTGRES_PASSWORD: postgres
      POSTGRES_DB: postgres
    ports:
      - "5433:5432"  # Use 5433 to avoid conflict with local PostgreSQL
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U postgres"]
      interval: 2s
      timeout: 2s
      retries: 10
    # Use tmpfs for faster test execution (data not persisted)
    tmpfs:
      - /var/lib/postgresql/data
```

#### 2. `scripts/test-db.sh`

```bash
#!/bin/bash
# Helper script for managing test database container

set -e

COMPOSE_FILE="docker-compose.test.yml"
SERVICE_NAME="postgres-test"

case "${1:-help}" in
  start)
    echo "Starting test PostgreSQL container..."
    docker compose -f "$COMPOSE_FILE" up -d "$SERVICE_NAME"
    echo "Waiting for PostgreSQL to be ready..."
    docker compose -f "$COMPOSE_FILE" exec -T "$SERVICE_NAME" \
      sh -c 'until pg_isready -U postgres; do sleep 1; done'
    echo "Test database ready at localhost:5433"
    echo ""
    echo "Run tests with:"
    echo "  TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5433/postgres cargo test"
    ;;
  stop)
    echo "Stopping test PostgreSQL container..."
    docker compose -f "$COMPOSE_FILE" down
    ;;
  status)
    docker compose -f "$COMPOSE_FILE" ps
    ;;
  logs)
    docker compose -f "$COMPOSE_FILE" logs -f "$SERVICE_NAME"
    ;;
  clean)
    echo "Removing test container and volumes..."
    docker compose -f "$COMPOSE_FILE" down -v
    ;;
  help|*)
    echo "Usage: $0 {start|stop|status|logs|clean}"
    echo ""
    echo "Commands:"
    echo "  start   Start the test PostgreSQL container"
    echo "  stop    Stop the test PostgreSQL container"
    echo "  status  Show container status"
    echo "  logs    Follow container logs"
    echo "  clean   Remove container and all data"
    ;;
esac
```

#### 3. `.env.test` (example file)

```bash
# Test database configuration
# Copy to .env or export these variables before running tests

# Docker-based test database (recommended)
TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5433/postgres

# Alternative: Use your own PostgreSQL installation
# TEST_DATABASE_URL=postgres://your_user:your_password@localhost:5432/your_db
```

### Files to Update

#### 4. Update `CONTRIBUTING.md`

Add section after "Prerequisites":

```markdown
### Running Tests

#### Quick Start (Docker - Recommended)

1. Start the test database:
   ```bash
   ./scripts/test-db.sh start
   ```

2. Run tests:
   ```bash
   TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5433/postgres cargo test
   ```

3. When done, stop the container:
   ```bash
   ./scripts/test-db.sh stop
   ```

#### Alternative: Local PostgreSQL

If you prefer using your own PostgreSQL installation:

1. Ensure PostgreSQL is running and you have `CREATEDB` privilege
2. Set the connection URL:
   ```bash
   export TEST_DATABASE_URL=postgres://user:password@localhost:5432/postgres
   ```
3. Run tests:
   ```bash
   cargo test
   ```

#### Unit Tests Only (No Database)

To run only unit tests without any database:
```bash
cargo test --lib --bins
```
```

#### 5. Update `README.md`

Add to "Development" or "Contributing" section:

```markdown
### Running Tests

```bash
# Start test database (one-time)
./scripts/test-db.sh start

# Run all tests
TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5433/postgres cargo test

# Run unit tests only (no database needed)
cargo test --lib --bins
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for detailed setup instructions.
```

#### 6. Update `.env.example`

Add:

```bash
# Test database URL (for integration tests)
# Start test database with: ./scripts/test-db.sh start
TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5433/postgres
```

### No Changes Required

- **`tests/common/mod.rs`**: TestDatabase helper works unchanged
- **`tests/integration_tests.rs`**: All tests work unchanged
- **`.github/workflows/ci.yml`**: CI continues using service containers
- **`Cargo.toml`**: No new dependencies

## Acceptance Criteria

### Functional Requirements

- [ ] `docker-compose.test.yml` creates PostgreSQL 16 container on port 5433
- [ ] `scripts/test-db.sh start` starts container and waits for readiness
- [ ] `scripts/test-db.sh stop` stops container cleanly
- [ ] All 9 integration tests pass with Docker database
- [ ] Tests skip gracefully when `TEST_DATABASE_URL` not set
- [ ] Existing `TEST_DATABASE_URL` approach continues working

### Non-Functional Requirements

- [ ] Container starts in < 5 seconds (after image cached)
- [ ] Tests run at same speed as with local PostgreSQL
- [ ] Works on macOS, Linux, and Windows (with Docker Desktop)
- [ ] Clear error messages when Docker not available

### Documentation

- [ ] CONTRIBUTING.md updated with Docker instructions
- [ ] README.md updated with quick start
- [ ] .env.example includes TEST_DATABASE_URL

## Test Plan

### Manual Testing

1. **Fresh clone experience**
   - Clone repo on new machine
   - Follow CONTRIBUTING.md instructions
   - Verify tests pass

2. **Existing developer migration**
   - Pull changes on existing setup
   - Verify old TEST_DATABASE_URL still works
   - Try Docker approach

3. **Docker not installed**
   - Run tests without Docker
   - Verify graceful skip with clear message

4. **Port conflict (5433 in use)**
   - Start something on port 5433
   - Run `./scripts/test-db.sh start`
   - Verify clear error message

5. **Cross-platform**
   - Test on macOS (Intel + Apple Silicon)
   - Test on Linux (Ubuntu)
   - Test on Windows (Docker Desktop + WSL2)

### Automated Testing

- CI continues using service containers (no changes)
- Add optional CI job to validate Docker Compose approach (future enhancement)

## Implementation Phases

### Phase 1: Core Infrastructure (MVP)

1. Create `docker-compose.test.yml`
2. Create `scripts/test-db.sh`
3. Create `.env.test` example
4. Update CONTRIBUTING.md
5. Update README.md
6. Test on macOS/Linux

**Estimated effort**: 1-2 hours

### Phase 2: Polish (Optional)

1. Add Makefile targets (`make test-db-start`, `make test`)
2. Add Windows batch script (`scripts/test-db.bat`)
3. Add troubleshooting guide
4. Add pre-commit hook to check Docker status

### Phase 3: Future Enhancements (Not in Scope)

1. testcontainers-rs for fully automated containers
2. Multi-version PostgreSQL testing matrix
3. CI migration to Docker Compose

## Alternatives Considered

### Alternative 1: testcontainers-rs

**Pros:**
- Fully automatic (no manual `docker compose up`)
- Each test run gets fresh container
- No lifecycle management

**Cons:**
- 3-5 second startup per test run (bad for TDD)
- New dependency (testcontainers crate)
- Different approach than CI

**Why rejected:** TDD workflow optimization is critical. 50-100 test runs/hour with 5s overhead each = 250-500s wasted daily.

### Alternative 2: Docker Compose Only (No Helper Script)

**Pros:**
- Simpler (fewer files)

**Cons:**
- Developers must remember docker compose commands
- No readiness wait
- Less discoverable

**Why rejected:** Developer experience matters. Helper script provides clear commands.

### Alternative 3: Migrate CI to Docker Compose

**Pros:**
- Consistency between local and CI

**Cons:**
- Current CI works well
- Windows CI runners don't support Docker easily
- Adds complexity for marginal benefit

**Why rejected:** If it ain't broke, don't fix it.

## Edge Cases & Error Handling

| Scenario | Handling |
|----------|----------|
| Docker not installed | Tests skip with message: "Skipping: TEST_DATABASE_URL not set" |
| Docker not running | `test-db.sh start` fails with Docker error |
| Port 5433 in use | Docker Compose fails with port binding error |
| Image not cached | First `start` pulls image (~30s on fast connection) |
| Container crashes | Tests fail, `test-db.sh start` restarts it |
| Orphaned test databases | tmpfs storage means no persistence between restarts |

## Security Considerations

- Default password `postgres` is acceptable for local testing
- Container only binds to localhost (not exposed externally)
- No persistent data (tmpfs) - no cleanup concerns
- Same credentials as CI (transparent, documented)

## References

### Internal References
- Current test setup: `crates/tsql/tests/common/mod.rs`
- Integration tests: `crates/tsql/tests/integration_tests.rs`
- CI configuration: `.github/workflows/ci.yml:62-92`
- Contributing guide: `CONTRIBUTING.md`

### External References
- [Docker Compose documentation](https://docs.docker.com/compose/)
- [PostgreSQL Docker image](https://hub.docker.com/_/postgres)
- [testcontainers-rs](https://github.com/testcontainers/testcontainers-rs) (alternative approach)

### Related Work
- CI uses PostgreSQL 16 service container (matches this proposal)
- TestDatabase helper already supports UUID-based isolation
