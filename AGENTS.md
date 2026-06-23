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
│   ├── mod.rs              # Module wiring (apple is cfg-gated here)
│   ├── service.rs          # AiProvider/AiService, fallback orchestration
│   ├── prompt.rs           # Prompt construction & message cleanup
│   ├── provider_command.rs # Per-provider Command construction & debug display
│   ├── process.rs          # Subprocess execution (timeout, concurrent I/O), TempFile
│   └── apple.rs            # Apple Intelligence native call (macOS + apple-ai only)
└── git/
    ├── mod.rs
    └── service.rs  # GitService for git operations

```

AI module layout note:
- `ai/` is split by responsibility: `service.rs` keeps the `AiProvider`/`AiService` types and the provider-fallback orchestration; prompt format contracts live in `prompt.rs`, per-provider CLI argument knowledge in `provider_command.rs`, subprocess lifecycle (the deadlock-avoidance threading in `run_process_with_timeout`) and output interpretation in `process.rs`, and the fm-rs native path in `apple.rs`. Cross-module items use `pub(super)` visibility. The unit tests for all of these currently remain in `ai/service.rs`'s `#[cfg(test)]` module (they exercise the same `AiService` associated functions regardless of which file defines them); relocating them next to their units is optional follow-up work, not a behavioral concern.

### Key Components

| Module | Responsibility |
|--------|----------------|
| `App` | Orchestrates workflow: verify env → load config → get diff → detect format → generate → commit |
| `AiService` | Multi-provider AI with fallback over a chain of `ProviderStep`s (provider + model + command + env). Each step resolves its provider via `AiProvider::from_str` (`gemini`/`agy` → `Antigravity`); the same provider may appear multiple times with different models/accounts. Default chain: opencode → antigravity → codex → claude → apple-intelligence. |
| `GitService` | Git operations (diff, commit, amend, squash, reword) |
| `Config` | Hierarchical config: global (~/.config/git-sc/config.toml) + project (.git-sc), via `PartialConfig` merge. `providers` is a `Vec<ProviderStep>` accepting either a string (provider name) or a `{provider, model, command, env, name}` table. |
| `ProviderState` | Tracks failed steps with a composite cooldown key (provider + model + env + command, or an explicit `name`), so the same provider on a different model/account is demoted independently. Default 1-hour cooldown. |

Recent-commit detection note:
- `GitService::get_recent_commits()` checks whether `HEAD` exists before calling `git log`, so empty repositories work regardless of Git locale or localized stderr text.

Repository detection note:
- `GitService::verify_repository()` asks Git whether the current directory is inside a work tree instead of trusting a `.git` path. A plain `.git` file or directory is not treated as a valid repository by itself.

Operation mode safety note:
- `--amend`, `--squash`, `--reword`, and `--generate-for` are mutually exclusive at CLI argument parsing time, so invalid combinations fail instead of silently choosing one workflow.
- `--squash` rejects pre-existing staged changes before `git reset --soft` so unrelated staged files are not folded into the squash commit.
- `--squash` records the original `HEAD` (via `GitService::get_head_hash()`) before `git reset --soft`. If the subsequent `git commit` fails (pre-commit/commit-msg hook rejection, GPG signing failure, etc.), the branch is restored to the original `HEAD` with another soft reset before the error is returned. Without this, the branch stays rewound at the merge-base and the original commits survive only in the reflog. Soft resets do not touch the index/worktree, so the recovery restores the exact pre-squash state.
- `--generate-for` keeps stdout reserved for the generated message only. With `--debug`, every debug block (config settings in `App::new`, AI prompt, provider command, streaming output, exit code) is routed to stderr in this mode; in other modes debug output stays on stdout as before. The routing is threaded as the `silent` flag through `generate_with_prefix` → `AiService::call_provider` → `print_debug_command` / `run_process_with_timeout` (`emit_debug_line`), and as `to_stderr` into `App::print_config_debug`.

Prefix script behavior note:
- Literal prefix script output has only trailing line endings (`\n`/`\r\n`) removed before application, so common `echo` output does not split the commit subject while intentional trailing spaces remain intact.
- If a prefix script returns empty output, `App` preserves the generated message and removes only a leading Conventional Commits type prefix (`feat:`, `fix(scope):`, `feat!:` etc.) when present.
- Prefix script exit code `1` is the explicit "use AI-generated message without prefix" signal. Other non-zero exit codes are treated as execution failures, so prefix selection can fall through to later scripts, rules, config, or auto detection.
- Relative `script` paths in project-level `.git-sc` are resolved from the Git repository root, and prefix scripts run with the Git root as their working directory.
- If the current `HEAD` is detached (`get_current_branch()` returns `None`), prefix scripts that already matched their `url_pattern` are skipped with an explicit `branch name unavailable (detached HEAD?), skipping script` notice instead of silently falling through. This avoids the confusing UX of printing "Running prefix script for..." and then doing nothing visible.
- Prefix mode is resolved in priority order: (1) `prefix_scripts` and (2) `prefix_rules` are evaluated only when a remote URL is available (both match their `url_pattern` against it), then (3) the config `prefix_type` and (4) automatic detection from recent commits are evaluated unconditionally. Steps 3 and 4 are remote-URL-independent, so a local-only repository without `remote.origin.url` still honors a configured `prefix_type` instead of silently falling back to Auto. Steps 1 and 2 are factored into the `try_prefix_scripts()` / `try_prefix_rules()` helpers to keep `get_prefix_mode_internal()` flat, but the priority and fall-through semantics above are the contract.

Commit hash validation note:
- `verify_commit_hash()` uses `^{commit}` suffix to constrain to commit objects only. Tree, blob, and other non-commit objects are rejected with `InvalidCommitHash` error.

Reword safety note:
- `GitService` validates that a `--reword` target hash is in the current `HEAD` history before merge-range checks and position calculation.
- If the hash exists but is outside the current history (e.g., another branch), reword fails with an error.
- If the target hash itself is a merge commit, reword also fails instead of silently treating it as a normal commit. This check is enforced inside `reword_commit()` itself (including the `n == 1` amend-path), not only in the calling layer, so the guarantee holds even when `reword_commit_by_hash()` is invoked directly without the `app.rs` pre-check.
- Rewording the oldest commit in the current branch is supported by switching to `git rebase -i --root` when needed.
- The reword rebase always passes `--no-autosquash` to isolate the user's `rebase.autoSquash=true` config. Without it, `fixup!`/`squash!` commits inside the rebase range get reordered in the todo and silently folded into their targets (history modification beyond the requested reword), and a `squash` line would additionally overwrite the folded commit's message with the reword message because `GIT_EDITOR` unconditionally copies the message file.
- The reword position `n` is always consumed as `HEAD~n` (a first-parent depth), so it is counted along the **first-parent** path: `get_commit_position_by_hash()` uses `git rev-list --count --first-parent <hash>..HEAD`, and the out-of-range / "oldest commit" checks use the first-parent depth (`git rev-list --count --first-parent HEAD`) too. Counting all ancestors topologically (the previous behavior) over-counts when the history contains merges, making `n` exceed the real first-parent depth so that `HEAD~n` resolves past the target — which surfaced a cryptic `fatal: ambiguous argument 'HEAD~n..HEAD'` instead of the intended outcome. With first-parent counting, position, root detection (`--root`), and merge detection stay consistent: rewording a first-parent ancestor that has a merge between it and `HEAD` cleanly fails with `HasMergeCommits` (merge-spanning reword is unsupported), while rewording across a merge-free range still works.
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

Provider fallback chain note:
- `Config.providers` is a `Vec<ProviderStep>` (`config.rs`). Each entry deserializes from **either** a plain string (provider name only) **or** a table `{ provider, model, command, env, name }`. This uses a hand-written `Deserialize` (string-or-struct via `deserialize_any`), deliberately **not** `#[serde(untagged)]`: under toml 1.x untagged collapses malformed input to a "did not match any variant" message, whereas the hand-written visitor surfaces the real missing-field error (e.g. a missing `provider`). The same provider may appear multiple times with different models or accounts — e.g. `codex` on two `CODEX_HOME`s, or `antigravity` on Gemini- vs GPT-OSS-family models, which have separate quotas.
- `AiService` holds the chain as `steps: Vec<ProviderStep>`. `from_config` keeps each raw provider string (alias canonicalization happens only at cooldown-key/comparison time) and drops steps whose provider does not resolve via `AiProvider::from_str`.
- Model resolution (`AiService::resolve_model`): `step.model` (non-empty) > `[models].<provider>` > empty (defer to the CLI's own default). `[models]` stays the per-provider default for steps that omit `model`.
- Command/binary: `step.command` (first element = binary, rest = fixed args; a wrapper-script path is allowed) overrides the provider's default binary (`provider.command()`); the provider's standard arguments (`--disable hooks`/`exec`/`-o` for codex, `-p` for claude, etc.) are still applied on top, because a wrapper ultimately invokes the same underlying CLI. `command[0]`'s `~` is expanded at `Config::load` time.
- Account switching via `env` (the load-bearing safety property): each step's `env` (`BTreeMap<String,String>`) is applied with an explicit `cmd.env(k, v)` in `build_provider_command`, and `env_clear()` is **not** called (PATH/HOME must stay inherited). An explicit `Command::env()` override beats whatever `CODEX_HOME`/`CLAUDE_CONFIG_DIR` is exported in the shell that launched git-sc, which prevents the class of bug where a step that omits the env silently inherits the parent shell's account and burns the wrong quota. env values are `~`-expanded and keys are validated against POSIX `[A-Za-z_][A-Za-z0-9_]*` (`is_valid_env_key`) at load time; an invalid key is a hard `ConfigError` (fail-fast, so a typo cannot silently redirect the run to the wrong account). Additionally, dynamic-loader / interpreter pre-load keys (`is_dangerous_env_key`: `LD_PRELOAD`, `LD_LIBRARY_PATH`, `LD_AUDIT`, `DYLD_INSERT_LIBRARIES`, `DYLD_LIBRARY_PATH`, `DYLD_FALLBACK_LIBRARY_PATH`, `DYLD_FRAMEWORK_PATH`, `DYLD_FALLBACK_FRAMEWORK_PATH`, `DYLD_FORCE_FLAT_NAMESPACE`, `DYLD_IMAGE_SUFFIX`, `DYLD_PRINT_LIBRARIES`, `NODE_OPTIONS`, `PYTHONPATH`, `PYTHONSTARTUP`, `PERL5OPT`, `PERL5LIB`, `RUBYOPT`, `RUBYLIB`) are compared case-insensitively and explicitly refused with a `ConfigError`. These keys can redirect a child process's shared-library/interpreter pre-load path, so a malicious project-level `.git-sc` could otherwise inject arbitrary code into the codex/claude/agy subprocess (the legitimate account-switching use case is unaffected because it relies on `CODEX_HOME` / `CLAUDE_CONFIG_DIR` etc., not loader keys). `--debug` prints each step's explicit env overrides and its `cooldown_key`.

Each provider is called via CLI subprocess:
- **opencode**: Uses temp file with `-f` flag to avoid command line length limits
- **antigravity** (`agy`, the successor of the Gemini CLI as of 2026-05): Uses `-p` flag for prompt input. As of `agy` v1.0.x the CLI supports `--model` (changelog: "Added --model to set model when launching CLI"), so git-sc passes the `[models] antigravity` value straight through as `agy --model "<name>"` when it is non-empty; an empty value omits `--model` and defers to agy's own default. The model name is the display name `agy models` prints (e.g. "GPT-OSS 120B (Medium)", "Gemini 3.5 Flash (Low)" — spaces and parentheses included), used verbatim. The CLI still exposes no `--debug` flag, so debug-related options remain intentionally omitted from the command line. Before launching, `AiService::check_arg_size_limit` rejects prompts larger than 512 KiB with an explicit error to avoid hitting OS-level `ARG_MAX`. The legacy `gemini` provider name remains accepted as an alias both in `from_str` and in the state-file cooldown key (auto-migrated to `antigravity` in memory on load). **Windows is unsupported for this provider**: all providers are launched through `cmd /C` there (npm `.cmd` shims), but cmd.exe does not understand Rust's MSVCRT-style argument quoting, so a prompt containing newlines or `"` (always true for a diff) deterministically corrupts the command line and is a CVE-2024-24576-class injection vector. `build_provider_command` returns an explicit provider error on Windows so the fallback chain moves on instead of failing silently.
- **codex/claude**: Uses stdin for prompt input; Codex also uses `-o`/`--output-last-message` to read only the final agent message instead of the execution transcript
- **apple-intelligence**: fm-rs (Rust FFI) via Foundation Models on-device (macOS 26+, Apple Intelligence enabled). The system instructions are built per request by `build_apple_instructions(language, prefix_type, has_recent_commits)` so they always agree with the prompt's `format_section`: explicit `prefix_type` values get a forceful (`CRITICAL FORMAT RULE`) rule for that exact style (`none`/`plain` forbids prefixes instead of forcing `feat:` as the old fixed instructions did), Auto mode with recent commits gets an imitate-their-format rule that deliberately contains **no concrete prefix examples** (the ~3B on-device model parrots example tokens like `[Add]` instead of reading the listed commits), and Auto without recent commits falls back to the Conventional Commits rule to match the prompt. Format-neutral instructions were tried and rejected: the small model becomes unstable (echoes the diff). Known limitation: in Auto mode the on-device model may still localize the prefix word (e.g. `修正:` instead of `feat:`) — a model-capability limit, also tolerated by the live tests (`assert_apple_intelligence_result` only warns).

Temp file safety note:
- `TempFile` and `TempRewordMessageFile` use RAII (Drop) for automatic cleanup.
- On Unix/macOS, temp files are created with mode `0600` so AI prompts, Codex final-output files, and reword messages are not readable by group or other users while they exist.
- On write/sync failure, the file is explicitly deleted before returning the error to prevent orphaned temp files.

Subprocess timeout note:
- `AiService::run_process_with_timeout()` writes the prompt to the child's stdin on a dedicated thread that runs **concurrently** with the stdout/stderr reader threads. Writing the full prompt synchronously before starting to read stdout (the previous approach) could deadlock: if a stdin-using provider (codex/claude) emits more than a pipe buffer's worth of stdout before consuming all of stdin, both pipes fill and the parent blocks in `write_all` while the child blocks on its own `write`. Because the timeout loop is never reached while `write_all` is blocked, the hang is unbounded. This is reachable in practice because `agent_context` (`CLAW_HOOKS_AGENT_MESSAGE`) is not length-limited. Running the writer concurrently with the readers removes the deadlock.
- `AiService::run_process_with_timeout()` joins the stdin writer thread and both stdout/stderr reader threads on every exit path (success, timeout, and `try_wait` error). After `child.kill()` and `child.wait()` close the pipes, all three threads receive EOF/EPIPE and exit cleanly, so no detached threads leak when a provider call times out. `std::process::Child::drop()` is a no-op, so this explicit join + the timeout loop's `kill()`/`wait()` are what prevent zombie children and leaked pipe FDs across the provider fallback chain.
- If the stdin write fails (e.g., the AI CLI exits immediately and the pipe receives EPIPE) **and** the child still reports success (exit 0), `run_process_with_timeout()` returns a provider error instead of treating the possibly-truncated prompt's output as a valid commit message. When the child exits non-zero, that exit status takes precedence and is handled by `process_provider_output()`.

When a step fails, that step enters cooldown (default: 60 minutes, keyed by provider+model+env+command) and the next step in the chain is tried.

Provider state file note:
- `State::save()` writes to `~/.config/git-sc/.providers-state.tmp` first and then `rename(2)`s it onto the final path so concurrent `git-sc` invocations never read a half-written TOML file. On rename failure the temporary file is deleted before the error is returned. The same cleanup also runs when the `fs::write` itself fails after creating the file (ENOSPC etc.); tmp names are unique per call and never reused, so a leftover file would otherwise accumulate forever.
- The temporary file suffix combines PID, monotonic nanosecond timestamp, and a process-local `AtomicU64` counter, so multiple threads (or rapid consecutive saves) that happen to observe the same wall-clock nanosecond never share a tmp path. Without the counter, two concurrent threads could write to the same `*.tmp.PID.NANOS` file and the slower thread's `rename(2)` would fail with `ENOENT` after the faster thread already moved it.
- `provider_cooldown_minutes` is converted to seconds with saturating arithmetic, so extremely large user-provided values do not panic in debug builds or wrap in release builds; they are treated as effectively indefinite cooldowns.
- Cooldown reordering canonicalizes provider aliases before comparing state keys with configured providers, so `gemini`/`agy` remain tied to `antigravity` and legacy `apple-ai`/`apple_intelligence` keys remain tied to `apple-intelligence`.
- Cooldown keys are composite, not provider-name-only: `ProviderStep::cooldown_key()` returns the explicit `name` (lowercased) if set, otherwise a deterministic key derived from canonical provider + model + env + command, joined by US (0x1F) so model names with spaces/parentheses/colons and env values with slashes never collide. This is what makes "the same provider on a different model or account is demoted independently" hold (e.g. `codex` on account A can be in cooldown while `codex` on account B keeps working, and `antigravity` on the Gemini model stays usable when the GPT-OSS step is cooling down). `State.failures` is a `Vec<ProviderFailure { key, provider, failed_at }>` (was a `HashMap<String, _>`); `State::load` migrates an old provider-name-keyed file by mapping each legacy key through the "provider-only step" `cooldown_key`, so existing cooldowns keep applying and `gemini`/`apple-ai` legacy keys still merge into `antigravity`/`apple-intelligence`. The legacy in-memory `migrate_legacy_gemini_key` is gone — `canonical_provider_key` (now in `config.rs`, shared by `ProviderStep::cooldown_key` and the state migration) handles the alias merge.

### Agent Context

When invoked from a coding agent, `App::run()` reads the `CLAW_HOOKS_AGENT_MESSAGE` environment variable and passes it to `AiService::build_prompt()` as `agent_context`. This context is injected into the AI prompt before the diff section, guiding the AI to reflect the developer's high-level intent in the commit message. The context is applied across standard generation and `--amend` / `--reword` / `--squash` / `--generate-for` workflows.

Default Codex model note:
- The default Codex model is `gpt-5.4-mini` (`default_codex_model()` in `config.rs`). It was selected on June 9, 2026 (JST): the candidates were narrowed to models that are API-visible, listed, and support `medium` reasoning — `gpt-5.5`, `gpt-5.4`, and `gpt-5.4-mini`. Each was measured with the fixed prompt `Reply ok.` in an empty directory (`-C <tmp> --skip-git-repo-check --ignore-user-config --ignore-rules --ephemeral --sandbox read-only`), reading `input_tokens` from the `--json` `turn.completed` event: `gpt-5.5` = 17152, `gpt-5.4` = 15770, `gpt-5.4-mini` = 15421. The `gpt-5.4-mini` run produced the required final output `ok` and no tool calls, so it is chosen as the default by the input-token primary metric. This measures minimal response cost only and does not guarantee real-workload (commit/review/refactor) quality; any future change should re-run `codex debug models` and re-measure, since model availability shifts over time. Re-verified on June 12, 2026 (JST): the candidate set was identical (measurement: `gpt-5.5` = 17180, `gpt-5.4` = 15801, `gpt-5.4-mini` = 15446; all produced `ok` with no tool calls — absolute values drift a few tokens between runs as the upstream system prompt evolves, but the ranking is stable), so the ranking and the `gpt-5.4-mini` default are unchanged. Re-measured again on June 15, 2026 (JST): `gpt-5.5` = 17429, `gpt-5.4` = 16044, `gpt-5.4-mini` = 15692. Re-measured on June 16, 2026 (JST) with `codex debug models` showing the same candidate set: `gpt-5.5` = 17653, `gpt-5.4` = 16274, `gpt-5.4-mini` = 15918; all accepted runs produced `ok` with no tool calls. `gpt-5.4-mini` remains the minimum, so the default is still unchanged. Re-measured on June 18, 2026 (JST) with the same candidate set: `gpt-5.5` = 30695, `gpt-5.4` = 29310, `gpt-5.4-mini` = 28962. The absolute values rose because Codex now emits a `Skill descriptions were shortened` system notice that adds to the input even with `--ignore-user-config` (it is appended by Codex's own prompt path, not the user config). Re-measured later the same day (June 18, 2026 JST) with the same candidate set: `gpt-5.5` = 17657, `gpt-5.4` = 16274, `gpt-5.4-mini` = 15922; the notice still appears but absolute values dropped back near the June 16 baseline, showing the Skill-context budget itself fluctuates between runs as the local Skill set changes. Re-measured on June 21, 2026 (JST) with the same candidate set: `gpt-5.5` = 31033, `gpt-5.4` = 29648, `gpt-5.4-mini` = 29296; all accepted runs produced `ok` (no tool calls, only the `Skill descriptions were shortened` system notice). Re-measured on June 23, 2026 (JST): `gpt-5.5` = 18445, `gpt-5.4` = 17060, `gpt-5.4-mini` = 16708; all accepted runs produced `ok` with no tool calls. `gpt-5.4-mini` remains the minimum, so the default is unchanged. Re-measured on June 24, 2026 (JST) with the same candidate set: `gpt-5.5` = 31887, `gpt-5.4` = 30502, `gpt-5.4-mini` = 30150; all accepted runs produced `ok` with no tool calls (only the `Skill descriptions were shortened` system notice). `gpt-5.4-mini` is still the minimum, so the default remains unchanged.

Default Antigravity (`agy`) model note:
- The default Antigravity model is `GPT-OSS 120B (Medium)` (`default_antigravity_model()` in `config.rs`). Unlike Codex, this was **not** chosen by measuring `input_tokens`: `agy` 1.0.x print mode (`-p`) exposes no official `--json` / `--output` option for per-request token usage, so the Codex-style empirical comparison is impossible. Re-verified on June 24, 2026 (JST): `agy models` listed `Gemini 3.5 Flash (Medium/High/Low)`, `Gemini 3.1 Pro (Low/High)`, `Claude Sonnet 4.6 (Thinking)`, `Claude Opus 4.6 (Thinking)`, and `GPT-OSS 120B (Medium)` — the same candidate set as on June 23, 2026. Google Cloud Agent Platform pricing lists `gpt-oss-120b` at $0.09 / 1M input tokens, lower than the listed Gemini and Claude alternatives, so `GPT-OSS 120B (Medium)` remains the lowest input-price default among the CLI-provided models. The value is the display name `agy models` prints and is passed verbatim to `agy --model "<name>"`; an empty string omits `--model` and defers to agy's own default. This is a cost-based heuristic, not a quality measurement, so any future change should re-check `agy models` and current pricing, since model availability and prices shift over time.

`[models]` field note:
- The canonical model key for the Antigravity provider is `antigravity` (the former `gemini` field was removed from `ModelsConfig`). A legacy `[models] gemini = "..."` value is still accepted as an input-only alias by `PartialModelsConfig` and is promoted to `antigravity` on load; if both `antigravity` and `gemini` are present, the explicit `antigravity` value wins. Running with `--debug` prints a one-time notice (`AiService::set_debug`) when a legacy `gemini` provider alias remains in the `providers` list, reminding the user it is normalized to `antigravity`. The `--debug` config dump in `App::print_config_debug` shows `models.antigravity` (rendering an empty value as `(agy default)`).

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
- Paths containing spaces are supported. Git does not quote space-only filenames in `diff --git` headers, so path extraction uses a midpoint split for symmetric `a/PATH b/PATH` headers to avoid misparsing (e.g., `diff --git a/foo bar.txt b/foo bar.txt` → `foo bar.txt`, not `foo` and `bar.txt`). Asymmetric unquoted rename headers split at the last ` b/`, so `diff --git a/old file.txt b/generated/new file.txt` checks both paths. Mixed rename headers are handled in both directions: quoted→unquoted consumes the unquoted side to the end of line, and unquoted(with spaces)→quoted (e.g. `diff --git a/old name.txt "b/new\303\251.txt"` — Git quotes each side independently, and a space alone does not trigger quoting) splits at the first `"`, which is unambiguous because an unquoted side can never contain `"`.

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
- **fm-rs** (optional, macOS, pinned to `0.1.4`): Apple Intelligence Foundation Models FFI. Keep pinned unless a newer release passes `cargo clippy --features apple-ai -- -D warnings`; `0.1.5` was rechecked on June 3, June 12, June 16, and June 24, 2026 (JST) — still the latest release and it still fails against the macOS 26.5 SDK because its Swift token usage API does not compile (`AsyncWaiter` is `private` and `SystemLanguageModel` has no `tokenUsage` member in `src/swift/token_usage_api.swift`). `depup --include-pinned` therefore continues to be reverted to `=0.1.4`.

### Dev
- **rstest**: Parameterized test framework
- **pretty_assertions**: Readable diff output for test assertions
- **tempfile**: Temporary file/directory management for tests
- **assert_cmd**: CLI integration testing
- **predicates**: Assertion matchers for assert_cmd
