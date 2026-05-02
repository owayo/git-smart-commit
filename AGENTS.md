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
make check      # Run clippy and cargo check (includes apple-ai on macOS)
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
git-sc --reword HEAD # Regenerate a specific commit message
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
| `Config` | Hierarchical config: global (~/.config/git-sc/config.toml) + project (.git-sc), via `PartialConfig` merge |
| `ProviderState` | Tracks failed providers with 1-hour cooldown |

Recent-commit detection note:
- `GitService::get_recent_commits()` checks whether `HEAD` exists before calling `git log`, so empty repositories work regardless of Git locale or localized stderr text.

Repository detection note:
- `GitService::verify_repository()` asks Git whether the current directory is inside a work tree instead of trusting a `.git` path. A plain `.git` file or directory is not treated as a valid repository by itself.

Operation mode safety note:
- `--amend`, `--squash`, `--reword`, and `--generate-for` are mutually exclusive at CLI argument parsing time, so invalid combinations fail instead of silently choosing one workflow.

Prefix script behavior note:
- If a prefix script returns empty output, `App` preserves the generated message and removes only a leading Conventional Commits type prefix (`feat:`, `fix(scope):`, `feat!:` etc.) when present.
- Prefix script exit code `1` is the explicit "use AI-generated message without prefix" signal. Other non-zero exit codes are treated as execution failures, so prefix selection can fall through to later scripts, rules, config, or auto detection.
- Relative `script` paths in project-level `.git-sc` are resolved from the Git repository root, and prefix scripts run with the Git root as their working directory.

Commit hash validation note:
- `verify_commit_hash()` uses `^{commit}` suffix to constrain to commit objects only. Tree, blob, and other non-commit objects are rejected with `InvalidCommitHash` error.

Reword safety note:
- `GitService` validates that a `--reword` target hash is in the current `HEAD` history before merge-range checks and position calculation.
- If the hash exists but is outside the current history (e.g., another branch), reword fails with an error.
- If the target hash itself is a merge commit, reword also fails instead of silently treating it as a normal commit.
- Rewording the oldest commit in the current branch is supported by switching to `git rebase -i --root` when needed.
- The temporary message file used during reword is created with a unique name and cleaned up automatically to avoid collisions between concurrent runs.
- `GIT_EDITOR` passes the message file path via `GIT_SC_MSG_FILE` environment variable (not shell string interpolation) to prevent injection attacks from paths containing special characters.
- The display-only short hash is computed via `chars().take(7)` so multibyte input (e.g. accidental non-ASCII argument) does not cause a UTF-8 boundary panic before validation runs.

Amend safety note:
- `GitService` reads the last-commit diff via `git show HEAD`, so `--amend` also works when the current `HEAD` is the root commit.

### AI Provider Implementation

Each provider is called via CLI subprocess:
- **opencode**: Uses temp file with `-f` flag to avoid command line length limits
- **gemini**: Uses `-p` flag for prompt input
- **codex/claude**: Uses stdin for prompt input
- **apple-intelligence**: fm-rs (Rust FFI) via Foundation Models on-device (macOS 26+, Apple Intelligence enabled)

Temp file safety note:
- `TempFile` and `TempRewordMessageFile` use RAII (Drop) for automatic cleanup.
- On write/sync failure, the file is explicitly deleted before returning the error to prevent orphaned temp files.

Subprocess timeout note:
- `AiService::run_process_with_timeout()` always joins the stdout/stderr reader threads on every exit path (success, timeout, and `try_wait` error). After `child.kill()` and `child.wait()` close the pipes, the reader threads receive EOF and exit cleanly, so no detached threads leak when a provider call times out.

When a provider fails, it enters cooldown (default: 60 minutes) and the next provider is tried.

Provider state file note:
- `State::save()` writes to `~/.config/git-sc/.providers-state.tmp` first and then `rename(2)`s it onto the final path so concurrent `git-sc` invocations never read a half-written TOML file. On rename failure the temporary file is deleted before the error is returned.

### Agent Context

When invoked from a coding agent, `App::run()` reads the `CLAW_HOOKS_AGENT_MESSAGE` environment variable and passes it to `AiService::build_prompt()` as `agent_context`. This context is injected into the AI prompt before the diff section, guiding the AI to reflect the developer's high-level intent in the commit message. The context is applied across standard generation and `--amend` / `--reword` / `--squash` / `--generate-for` workflows.

Default Codex model note:
- As of May 2, 2026, the default Codex model is `gpt-5.4`. It was rechecked locally with `codex debug models` and `echo "Hello" | codex exec -c model_reasoning_effort='medium' -m <model>` across listed Codex models. `gpt-5.4` and `codex-auto-review` tied for the fewest tokens among successful models (`13,813`). `gpt-5.4` is selected because git-sc uses Codex for general commit-message generation, while `codex-auto-review` is review-oriented. Other successful candidates were `gpt-5.3-codex-spark` (`14,304`), `gpt-5.3-codex` (`17,470`), `gpt-5.4-mini` (`18,627`), `gpt-5.5` (`19,295`), and `gpt-5.2` (`19,344`).

## Configuration Files

| File | Scope |
|------|-------|
| `~/.config/git-sc/config.toml` | Global user settings |
| `.git-sc` | Project-level overrides (repo root) |
| `.git-sc-ignore` | Patterns to exclude from diff |

Config merge note:
- Settings are loaded via `PartialConfig` (all `Option<T>` fields) to distinguish "unset" from "explicitly set to default value".
- Project config correctly overrides global config even when the project value equals the default (e.g., `language = "Japanese"` overriding `language = "English"`).
- `PartialConfig::merge_into()` only overwrites fields that are explicitly present in the project config file.

`.git-sc-ignore` note:
- Patterns are matched against decoded Git paths, including quoted diff headers with non-ASCII filenames.
- For rename diffs, ignore matching checks both the pre-rename and post-rename path so moves into ignored directories are excluded consistently.
- Patterns apply to both text and binary files. Ignore filtering runs before binary-to-summary conversion so that binary files matching ignore patterns are fully excluded from the diff.
- `decode_quoted_diff_path` validates that 3-digit octal escape values are within the u8 range (0-377). Values exceeding 255 (e.g., `\400`) are rejected as invalid input.
- Paths containing spaces are supported. Git does not quote space-only filenames in `diff --git` headers, so path extraction uses a midpoint split for symmetric `a/PATH b/PATH` headers to avoid misparsing (e.g., `diff --git a/foo bar.txt b/foo bar.txt` → `foo bar.txt`, not `foo` and `bar.txt`). Mixed quoted/unquoted rename headers are also handled by consuming the unquoted side to the end of line.

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
- **fm-rs** (optional, macOS, pinned to `0.1.4`): Apple Intelligence Foundation Models FFI. Keep pinned unless a newer release passes `cargo clippy --features apple-ai -- -D warnings`; `0.1.5` fails against the macOS 26.4 SDK because its Swift token usage API does not compile.

### Dev
- **rstest**: Parameterized test framework
- **pretty_assertions**: Readable diff output for test assertions
- **tempfile**: Temporary file/directory management for tests
- **assert_cmd**: CLI integration testing
- **predicates**: Assertion matchers for assert_cmd
