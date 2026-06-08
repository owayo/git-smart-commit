# git-sc (Git Smart Commit)

AI-powered smart commit message generator CLI tool written in Rust.

## Project Overview

This CLI tool generates commit messages using AI coding agents (opencode, Antigravity CLI (`agy`, the successor of Gemini CLI), Codex CLI, Claude Code, Apple Intelligence) with automatic provider fallback and format detection.

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
git-sc -p claude    # Use specific AI provider (antigravity, codex, claude, opencode, apple-intelligence); legacy "gemini" name is accepted as alias
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
| `AiService` | Multi-provider AI with fallback (opencode → antigravity → codex → claude → apple-intelligence). `AiProvider::from_str("gemini")` resolves to `Antigravity` for backward compatibility. |
| `GitService` | Git operations (diff, commit, amend, squash, reword) |
| `Config` | Hierarchical config: global (~/.config/git-sc/config.toml) + project (.git-sc), via `PartialConfig` merge |
| `ProviderState` | Tracks failed providers with 1-hour cooldown |

Recent-commit detection note:
- `GitService::get_recent_commits()` checks whether `HEAD` exists before calling `git log`, so empty repositories work regardless of Git locale or localized stderr text.

Repository detection note:
- `GitService::verify_repository()` asks Git whether the current directory is inside a work tree instead of trusting a `.git` path. A plain `.git` file or directory is not treated as a valid repository by itself.

Operation mode safety note:
- `--amend`, `--squash`, `--reword`, and `--generate-for` are mutually exclusive at CLI argument parsing time, so invalid combinations fail instead of silently choosing one workflow.
- `--squash` rejects pre-existing staged changes before `git reset --soft` so unrelated staged files are not folded into the squash commit.

Prefix script behavior note:
- Literal prefix script output has only trailing line endings (`\n`/`\r\n`) removed before application, so common `echo` output does not split the commit subject while intentional trailing spaces remain intact.
- If a prefix script returns empty output, `App` preserves the generated message and removes only a leading Conventional Commits type prefix (`feat:`, `fix(scope):`, `feat!:` etc.) when present.
- Prefix script exit code `1` is the explicit "use AI-generated message without prefix" signal. Other non-zero exit codes are treated as execution failures, so prefix selection can fall through to later scripts, rules, config, or auto detection.
- Relative `script` paths in project-level `.git-sc` are resolved from the Git repository root, and prefix scripts run with the Git root as their working directory.
- If the current `HEAD` is detached (`get_current_branch()` returns `None`), prefix scripts that already matched their `url_pattern` are skipped with an explicit `branch name unavailable (detached HEAD?), skipping script` notice instead of silently falling through. This avoids the confusing UX of printing "Running prefix script for..." and then doing nothing visible.

Commit hash validation note:
- `verify_commit_hash()` uses `^{commit}` suffix to constrain to commit objects only. Tree, blob, and other non-commit objects are rejected with `InvalidCommitHash` error.

Reword safety note:
- `GitService` validates that a `--reword` target hash is in the current `HEAD` history before merge-range checks and position calculation.
- If the hash exists but is outside the current history (e.g., another branch), reword fails with an error.
- If the target hash itself is a merge commit, reword also fails instead of silently treating it as a normal commit. This check is enforced inside `reword_commit()` itself (including the `n == 1` amend-path), not only in the calling layer, so the guarantee holds even when `reword_commit_by_hash()` is invoked directly without the `app.rs` pre-check.
- Rewording the oldest commit in the current branch is supported by switching to `git rebase -i --root` when needed.
- Rewording `HEAD` uses `git commit --amend --only` so unrelated staged changes remain staged instead of being included in the rewritten commit.
- The temporary message file used during reword is created with a unique name and cleaned up automatically to avoid collisions between concurrent runs.
- `GIT_EDITOR` passes the message file path via `GIT_SC_MSG_FILE` environment variable (not shell string interpolation) to prevent injection attacks from paths containing special characters.
- The display-only short hash is computed via `chars().take(7)` so multibyte input (e.g. accidental non-ASCII argument) does not cause a UTF-8 boundary panic before validation runs.
- When the underlying `git rebase -i` fails for any reason (CONFLICT, rejection by `commit-msg`/`pre-commit` hooks, editor errors, etc.), `GitService::reword_commit()` unconditionally runs `git rebase --abort` before returning the error. This prevents the repository from being left in an "interrupted rebase" state that would block all subsequent git operations.

Amend safety note:
- `GitService` reads the last-commit diff via `git show HEAD`, so `--amend` also works when the current `HEAD` is the root commit.
- `GitService::amend_commit()` uses `git commit --amend --only` so unrelated staged changes remain staged instead of being included in the amended commit.

Notification safety note:
- On macOS the notification path calls `CFStringCreateWithCString` for both the notification name and body. Each return value is null-checked before use, and any already-allocated CFString is `CFRelease`d before bailing out. `CFRelease(NULL)` is undefined behaviour, so this guard avoids a crash if CoreFoundation fails to allocate (e.g., under memory pressure).

### AI Provider Implementation

Each provider is called via CLI subprocess:
- **opencode**: Uses temp file with `-f` flag to avoid command line length limits
- **antigravity** (`agy`, the successor of the Gemini CLI as of 2026-05): Uses `-p` flag for prompt input. The CLI exposes no `-m`/`--model` or `--debug` flag, so model/debug related options are intentionally omitted from the command line. Before launching, `AiService::check_arg_size_limit` rejects prompts larger than 512 KiB with an explicit error to avoid hitting OS-level `ARG_MAX`. The legacy `gemini` provider name remains accepted as an alias both in `from_str` and in the state-file cooldown key (auto-migrated to `antigravity` in memory on load).
- **codex/claude**: Uses stdin for prompt input; Codex also uses `-o`/`--output-last-message` to read only the final agent message instead of the execution transcript
- **apple-intelligence**: fm-rs (Rust FFI) via Foundation Models on-device (macOS 26+, Apple Intelligence enabled)

Temp file safety note:
- `TempFile` and `TempRewordMessageFile` use RAII (Drop) for automatic cleanup.
- On Unix/macOS, temp files are created with mode `0600` so AI prompts, Codex final-output files, and reword messages are not readable by group or other users while they exist.
- On write/sync failure, the file is explicitly deleted before returning the error to prevent orphaned temp files.

Subprocess timeout note:
- `AiService::run_process_with_timeout()` writes the prompt to the child's stdin on a dedicated thread that runs **concurrently** with the stdout/stderr reader threads. Writing the full prompt synchronously before starting to read stdout (the previous approach) could deadlock: if a stdin-using provider (codex/claude) emits more than a pipe buffer's worth of stdout before consuming all of stdin, both pipes fill and the parent blocks in `write_all` while the child blocks on its own `write`. Because the timeout loop is never reached while `write_all` is blocked, the hang is unbounded. This is reachable in practice because `agent_context` (`CLAW_HOOKS_AGENT_MESSAGE`) is not length-limited. Running the writer concurrently with the readers removes the deadlock.
- `AiService::run_process_with_timeout()` joins the stdin writer thread and both stdout/stderr reader threads on every exit path (success, timeout, and `try_wait` error). After `child.kill()` and `child.wait()` close the pipes, all three threads receive EOF/EPIPE and exit cleanly, so no detached threads leak when a provider call times out. `std::process::Child::drop()` is a no-op, so this explicit join + the timeout loop's `kill()`/`wait()` are what prevent zombie children and leaked pipe FDs across the provider fallback chain.
- If the stdin write fails (e.g., the AI CLI exits immediately and the pipe receives EPIPE) **and** the child still reports success (exit 0), `run_process_with_timeout()` returns a provider error instead of treating the possibly-truncated prompt's output as a valid commit message. When the child exits non-zero, that exit status takes precedence and is handled by `process_provider_output()`.

When a provider fails, it enters cooldown (default: 60 minutes) and the next provider is tried.

Provider state file note:
- `State::save()` writes to `~/.config/git-sc/.providers-state.tmp` first and then `rename(2)`s it onto the final path so concurrent `git-sc` invocations never read a half-written TOML file. On rename failure the temporary file is deleted before the error is returned.
- The temporary file suffix combines PID, monotonic nanosecond timestamp, and a process-local `AtomicU64` counter, so multiple threads (or rapid consecutive saves) that happen to observe the same wall-clock nanosecond never share a tmp path. Without the counter, two concurrent threads could write to the same `*.tmp.PID.NANOS` file and the slower thread's `rename(2)` would fail with `ENOENT` after the faster thread already moved it.
- `provider_cooldown_minutes` is converted to seconds with saturating arithmetic, so extremely large user-provided values do not panic in debug builds or wrap in release builds; they are treated as effectively indefinite cooldowns.
- Cooldown reordering canonicalizes provider aliases before comparing state keys with configured providers, so `gemini`/`agy` remain tied to `antigravity` and legacy `apple-ai`/`apple_intelligence` keys remain tied to `apple-intelligence`.

### Agent Context

When invoked from a coding agent, `App::run()` reads the `CLAW_HOOKS_AGENT_MESSAGE` environment variable and passes it to `AiService::build_prompt()` as `agent_context`. This context is injected into the AI prompt before the diff section, guiding the AI to reflect the developer's high-level intent in the commit message. The context is applied across standard generation and `--amend` / `--reword` / `--squash` / `--generate-for` workflows.

Default Codex model note:
- The default Codex model is `gpt-5.4-mini` (`default_codex_model()` in `config.rs`). It was selected on June 9, 2026 (JST): the candidates were narrowed to models that are API-visible, listed, and support `medium` reasoning — `gpt-5.5`, `gpt-5.4`, and `gpt-5.4-mini`. Each was measured with the fixed prompt `Reply ok.` in an empty directory (`-C <tmp> --skip-git-repo-check --ignore-user-config --ignore-rules --ephemeral --sandbox read-only`), reading `input_tokens` from the `--json` `turn.completed` event: `gpt-5.5` = 17152, `gpt-5.4` = 15770, `gpt-5.4-mini` = 15421. The `gpt-5.4-mini` run produced the required final output `ok` and no tool calls, so it is chosen as the default by the input-token primary metric. This measures minimal response cost only and does not guarantee real-workload (commit/review/refactor) quality; any future change should re-run `codex debug models` and re-measure, since model availability shifts over time.

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
- Paths containing spaces are supported. Git does not quote space-only filenames in `diff --git` headers, so path extraction uses a midpoint split for symmetric `a/PATH b/PATH` headers to avoid misparsing (e.g., `diff --git a/foo bar.txt b/foo bar.txt` → `foo bar.txt`, not `foo` and `bar.txt`). Asymmetric unquoted rename headers split at the last ` b/`, so `diff --git a/old file.txt b/generated/new file.txt` checks both paths. Mixed quoted/unquoted rename headers are also handled by consuming the unquoted side to the end of line.

Diff truncation note:
- `truncate_diff()` first compares `diff.len()` (byte length) against `MAX_DIFF_CHARS`. Since UTF-8 always uses at least one byte per character, byte length within the limit guarantees character count is also within the limit, so the common ASCII path returns immediately without scanning the entire diff.
- When byte length exceeds the limit, `char_indices().nth(MAX_DIFF_CHARS)` finds the cutoff after scanning at most `MAX_DIFF_CHARS+1` characters, avoiding the previous full-diff `chars().count()` walk on multi-MB inputs.

## Testing

```bash
cargo test                    # Run all tests
cargo test test_name          # Run specific test
cargo test -- --nocapture     # Show println! output
```

- Unit tests: `#[cfg(test)]` modules in each source file
- Integration tests: `tests/cli_integration.rs` (CLI behavior via assert_cmd)
- Git tests must not assume the initial branch is `master`; use `git branch --show-current` in temporary repositories when switching back to the primary branch.

## Dependencies

### Runtime
- **clap**: CLI argument parsing with derive macros
- **anyhow/thiserror**: Error handling
- **serde/toml**: Configuration parsing
- **colored**: Terminal output styling
- **regex**: Commit format detection
- **ignore**: Gitignore-style pattern matching
- **dirs**: Platform-specific directory paths
- **fm-rs** (optional, macOS, pinned to `0.1.4`): Apple Intelligence Foundation Models FFI. Keep pinned unless a newer release passes `cargo clippy --features apple-ai -- -D warnings`; `0.1.5` was rechecked on June 3, 2026 (JST) and still fails against the macOS 26.5 SDK because its Swift token usage API does not compile (`AsyncWaiter` is `private` and `SystemLanguageModel` has no `tokenUsage` member in `src/swift/token_usage_api.swift`).

### Dev
- **rstest**: Parameterized test framework
- **pretty_assertions**: Readable diff output for test assertions
- **tempfile**: Temporary file/directory management for tests
- **assert_cmd**: CLI integration testing
- **predicates**: Assertion matchers for assert_cmd
