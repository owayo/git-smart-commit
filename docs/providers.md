# Provider Notes

How git-sc launches the providers that need special handling, and what changes per platform. Which providers run, in what order, and with which model or account is set in [configuration.md](configuration.md).

## Grok CLI

The Grok provider drives the Grok Build TUI (`grok`, xAI). That CLI is a full coding agent — plan mode, cross-session memory, web search, and tool execution are all on by default — so git-sc constrains it to behave as a single-turn pure function:

| Flag | Why |
|------|-----|
| `--output-format plain` | Headless text output instead of the interactive TUI |
| `--sandbox read-only` | Forbids filesystem writes and network, like Codex's sandbox |
| `--no-plan` / `--no-memory` | Blocks plan mode and cross-session memory (both default on) |
| `--disable-web-search` | Cuts web fetch/search |
| `--max-turns 1` | Stops any tool loop after one turn |
| `--verbatim` | Prevents the CLI from rewriting the prompt |
| `--prompt-file <temp file>` | Avoids `ARG_MAX` limits and cmd.exe metacharacter issues for large diffs |

- **Model**: resolved as `model` on the step > `[models].grok` > empty (defer to grok's own default). When non-empty, the ID from `grok models` (currently only `grok-4.5`) is passed as `-m "<id>"`. The shipped default for `[models].grok` is left empty so a cheaper model added later is picked up without a git-sc release.
- **Availability**: the Grok CLI ships bundled with [cmux](https://github.com/manaflow-ai/cmux) at `/Applications/cmux.app/Contents/Resources/bin/grok`. If `grok` is not on `PATH`, this step is skipped and the chain moves on to the next provider.

## Apple Intelligence

Apple Intelligence provider uses [fm-rs](https://github.com/blacktop/fm-rs) (Rust bindings for Apple's [Foundation Models](https://developer.apple.com/documentation/foundationmodels) framework) for fully on-device inference. No API key or network connection is required.

- **Requirements**: macOS 26 (Tahoe) or later, Apple Silicon, Apple Intelligence enabled in System Settings
- **How it works**: With Apple Intelligence enabled (default on macOS), git-sc calls Foundation Models directly via fm-rs. A `LanguageModelSession` is created with commit-message-specific instructions for each generation. The instructions are built from the resolved prefix type, so `prefix_type = "none"` / `"bracket"` / `"emoji"` and auto-detection from recent commits are respected instead of always forcing Conventional Commits.
- **Context window**: the on-device model holds **4096 tokens**, which is far smaller than every other provider. git-sc measures the prompt before generating and, when it does not fit, rebuilds it from a compacted diff that keeps the full list of changed files and truncates the bodies. A warning is printed when this happens, because the resulting message was written from a partial view of the change. If even the compacted prompt does not fit, the run moves on to the next provider instead of failing the commit.
- **Timeout**: bounded by `provider_timeout_seconds` (default 60), the same setting the CLI providers use.
- **Failures**: a failure caused by the prompt itself (context size, safety guardrail, refusal, unsupported language) does not put the provider into cooldown — only failures that mean the model is currently unusable (assets not downloaded, rate limited, timed out) do. Typed failure classification requires building against the macOS 27 SDK or later; on macOS 26 all failures are treated as provider failures.
- **Build**: `cargo build --features apple-ai` (automatic with `make build` / `make install` on macOS). Building on macOS 27 or later additionally enables Foundation Models 27 features (exact token counts, typed errors, per-response token usage); macOS 26 remains supported.
- **Cross-platform**: On Linux/Windows, Apple Intelligence is not available and is automatically skipped

## Platform Notes

- **Windows**: the Antigravity CLI (`agy`) provider is skipped with an explicit error. All providers launch through `cmd /C` on Windows (to support npm-installed `.cmd` shims), but cmd.exe cannot safely receive a multi-line diff prompt as a command-line argument, so passing it would corrupt the command (and is a known command-injection vector class, CVE-2024-24576). The fallback chain simply moves on to the next provider. Providers that read the prompt from stdin or a temp file (codex, claude, opencode, grok) are unaffected.
