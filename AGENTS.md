# git-sc (Git Smart Commit)

AI-powered smart commit message generator CLI tool written in Rust.

## Project Overview

This CLI tool generates commit messages using AI coding agents (opencode, Gemini CLI, Codex CLI, Claude Code, Apple Intelligence) with automatic provider fallback and format detection.

## Quick Reference

### Build Commands
```bash
make build      # Debug build
make release    # Release build (optimized)
make install    # Build and install to /usr/local/bin (Apple Intelligence enabled on macOS)
make test       # Run tests
make fmt        # Format code
make check      # Run clippy and cargo check
make clean      # Clean build artifacts
```

### Common Usage
```bash
git-sc              # Generate message for staged changes
git-sc -a -y        # Stage all and commit without confirmation
git-sc -n           # Dry run (preview only)
git-sc -a -y -q     # Quiet mode (suppress progress logs for hooks/scripts)
git-sc --debug      # Show AI prompt and command being executed
git-sc -p claude    # Use specific AI provider (gemini, codex, claude, opencode, apple-intelligence)
git-sc --amend      # Regenerate last commit message
git-sc --squash main # Squash commits since main branch
```

## Architecture

```
src/
├── main.rs      # Entry point, CLI dispatch
├── cli.rs       # clap-based argument parsing
├── app.rs       # Application orchestrator (main workflow)
├── config.rs    # Hierarchical TOML configuration
├── error.rs     # AppError enum with thiserror
├── init.rs      # `git-sc init` subcommand
├── state.rs     # Provider cooldown state management
├── notify.rs    # NanoBuddy notification (macOS DistributedNotificationCenter)
├── ai/
│   ├── mod.rs
│   └── service.rs  # AiService with multi-provider fallback
└── git/
    ├── mod.rs
    └── service.rs  # GitService for git operations

```

### Key Components

| Module | Responsibility |
|--------|----------------|
| `App` | Orchestrates workflow: verify env → load config → get diff → detect format → generate → commit |
| `AiService` | Multi-provider AI with fallback (opencode → gemini → codex → claude → apple-intelligence) |
| `GitService` | Git operations (diff, commit, amend, squash, reword) |
| `Config` | Hierarchical config: global (~/.config/git-sc/config.toml) + project (.git-sc) |
| `ProviderState` | Tracks failed providers with 1-hour cooldown |

Prefix script behavior note:
- If a prefix script returns empty output, `App` preserves the generated message and removes only a leading Conventional Commits type prefix (`feat:`, `fix(scope):`, `feat!:` etc.) when present.

Reword safety note:
- `GitService` validates that a `--reword` target hash is in the current `HEAD` history before merge-range checks and position calculation.
- If the hash exists but is outside the current history (e.g., another branch), reword fails with an error.
- Rewording the oldest commit in the current branch is supported by switching to `git rebase -i --root` when needed.

### AI Provider Implementation

Each provider is called via CLI subprocess:
- **opencode**: Uses temp file with `-f` flag to avoid command line length limits
- **gemini**: Uses `-p` flag for prompt input
- **codex/claude**: Uses stdin for prompt input
- **apple-intelligence**: fm-rs (Rust FFI) via Foundation Models on-device (macOS 26+, Apple Intelligence enabled)

When a provider fails, it enters cooldown (default: 60 minutes) and the next provider is tried.

### Agent Context

When invoked from a coding agent, `App::run()` reads the `CLAW_HOOKS_AGENT_MESSAGE` environment variable and passes it to `AiService::build_prompt()` as `agent_context`. This context is injected into the AI prompt before the diff section, guiding the AI to reflect the developer's high-level intent in the commit message. The context is applied across standard generation and `--amend` / `--reword` / `--squash` / `--generate-for` workflows.

## Configuration Files

| File | Scope |
|------|-------|
| `~/.config/git-sc/config.toml` | Global user settings |
| `.git-sc` | Project-level overrides (repo root) |
| `.git-sc-ignore` | Patterns to exclude from diff |

## Testing

```bash
cargo test                    # Run all tests
cargo test test_name          # Run specific test
cargo test -- --nocapture     # Show println! output
```

- Unit tests: `#[cfg(test)]` modules in each source file
- Integration tests: `tests/cli_integration.rs` (CLI behavior via assert_cmd)

## Dependencies

### Runtime
- **clap**: CLI argument parsing with derive macros
- **anyhow/thiserror**: Error handling
- **serde/toml**: Configuration parsing
- **colored**: Terminal output styling
- **regex**: Commit format detection
- **ignore**: Gitignore-style pattern matching
- **dirs**: Platform-specific directory paths
- **fm-rs** (optional, macOS): Apple Intelligence Foundation Models FFI

### Dev
- **rstest**: Parameterized test framework
- **pretty_assertions**: Readable diff output for test assertions
- **tempfile**: Temporary file/directory management for tests
- **assert_cmd**: CLI integration testing
- **predicates**: Assertion matchers for assert_cmd
