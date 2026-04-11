---
name: rust-worker
description: Worker for Rust implementation tasks — building features, writing tests, porting code
---

# Rust Worker

NOTE: Startup and cleanup are handled by `worker-base`. This skill defines the WORK PROCEDURE.

## When to Use This Skill

Use this worker for implementing Rust code features, writing tests, porting code from ClawHip, building CLI commands, HTTP endpoints, event sources, renderers, sinks, and configuration systems. This worker handles the core implementation work for the Hermip project.

## Required Skills

None — this worker uses standard tools (code editing, shell commands, Rust tooling).

## Work Procedure

1. **Read AGENTS.md and mission.md** — Understand project conventions, boundaries, and architecture.

2. **Read `.factory/library/architecture.md`** — Understand the system architecture, component relationships, and key invariants.

3. **Write tests first (RED)** — Before implementing any feature, write failing tests that describe the expected behavior. Place unit tests in `#[cfg(test)]` modules and integration tests in `tests/`. Cover:
   - Happy path behavior
   - Error cases (invalid input, missing fields, edge cases)
   - Backward compatibility (if applicable)

4. **Implement to make tests pass (GREEN)** — Write the minimum code needed to make all tests pass. Follow existing patterns in the codebase (Source trait, Sink trait, etc.).

5. **Run test suite** — Execute `cargo test` and verify all tests pass. Fix any failures.

6. **Run linter and type checker** — Execute `cargo clippy -- -D warnings` and `cargo fmt --check`. Fix any issues.

7. **Build release binary** — Execute `cargo build --release` and verify it succeeds.

8. **Manual verification** — For features with CLI or HTTP API surfaces, manually verify:
   - CLI commands produce expected output
   - HTTP endpoints return expected status codes
   - Error cases produce helpful messages
   - Record each manual check in the handoff

9. **Commit your work** — Stage and commit all changes with a descriptive message.

## Example Handoff

```json
{
  "salientSummary": "Implemented Hermip daemon HTTP API with POST /api/event and GET /health endpoints. All tests pass, clippy clean, release build succeeds.",
  "whatWasImplemented": "Added axum HTTP server to hermip daemon with /health and /api/event endpoints. Health endpoint returns {\"status\":\"ok\"}. Event endpoint validates JSON payloads (returns 202 for valid, 400 for invalid, 415 for wrong content-type). Unknown paths return 404. SIGTERM handler for graceful shutdown.",
  "whatWasLeftUndone": "",
  "verification": {
    "commandsRun": [
      {"command": "cargo test", "exitCode": 0, "observation": "47 tests passed, 0 failed"},
      {"command": "cargo clippy -- -D warnings", "exitCode": 0, "observation": "No warnings"},
      {"command": "cargo fmt --check", "exitCode": 0, "observation": "All formatted"},
      {"command": "cargo build --release", "exitCode": 0, "observation": "Binary at target/release/hermip"},
      {"command": "curl -sf http://localhost:25294/health", "exitCode": 0, "observation": "{\"status\":\"ok\"}"},
      {"command": "curl -X POST -H 'Content-Type: application/json' -d '{\"source\":\"droid\",\"event_type\":\"mission.started\"}' http://localhost:25294/api/event", "exitCode": 0, "observation": "HTTP 202"}
    ],
    "interactiveChecks": [
      {"action": "Started daemon and verified health endpoint", "observed": "GET /health returned 200 with {\"status\":\"ok\"}"},
      {"action": "Sent invalid JSON to /api/event", "observed": "Returned 400 with descriptive error message"},
      {"action": "Sent SIGTERM to daemon", "observed": "Process exited cleanly within 5 seconds"}
    ]
  },
  "tests": {
    "added": [
      {"file": "src/daemon.rs", "cases": [{"name": "test_health_endpoint", "verifies": "VAL-DAEMON-001"}, {"name": "test_event_endpoint_valid", "verifies": "VAL-DAEMON-002"}, {"name": "test_event_endpoint_invalid", "verifies": "VAL-DAEMON-003"}]},
      {"file": "tests/integration_daemon.rs", "cases": [{"name": "test_graceful_shutdown", "verifies": "VAL-DAEMON-006"}, {"name": "test_unknown_routes_404", "verifies": "VAL-DAEMON-012"}]}
    ]
  },
  "discoveredIssues": []
}
```

## When to Return to Orchestrator

- Feature depends on an API endpoint or data model that doesn't exist yet (cross-feature dependency)
- Requirements are ambiguous or contradictory
- Existing bugs in the codebase block progress
- Cannot complete work within mission boundaries (port range, resources, etc.)
