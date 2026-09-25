# Configuration

Every setting git-sc reads, from the provider fallback chain to the optional integrations. git-sc works without any configuration file; the [README](../README.md#configuration) has a short example.

## Initial Setup

Initialize configuration with `git-sc init`, or create `~/.config/git-sc/config.toml` manually:

```bash
git-sc init
```

This creates a configuration file with default settings at `~/.config/git-sc/config.toml`.

Use `--force` to overwrite an existing configuration:

```bash
git-sc init --force
```

## Hierarchical Configuration

git-sc supports hierarchical configuration with project-level overrides:

| File | Scope | Description |
|------|-------|-------------|
| `~/.config/git-sc/config.toml` | Global | User-wide default settings |
| `.git-sc` | Project | Repository-specific overrides (in repo root) |

Project settings override global settings. Fields not specified in project config inherit from global config. You can specify only the fields you want to override — partial `[models]` sections are supported.

## Example Configuration

```toml
# AI provider priority
# "antigravity" is the successor of the legacy Gemini CLI (`agy` command).
# Writing "gemini" instead is still accepted as a backward-compatible alias.
providers = ["opencode", "grok", "antigravity", "codex", "claude", "apple-intelligence"]

# Commit message language
language = "Japanese"

# Commit prefix format (optional)
# Values: conventional, bracket, colon, emoji, plain, none
prefix_type = "conventional"

# Auto-push after commit (optional)
auto_push = true

# Codex reasoning effort passed via `-c model_reasoning_effort=<value>`
# Values: "low" (default), "medium", "high", "xhigh", or "" to omit and use codex default
codex_reasoning_effort = "low"

# Model configuration
# Antigravity CLI (`agy`) supports `--model`: the `antigravity` value is passed straight
# to `agy --model "<name>"`. Either spelling works — the display name
# (e.g. "GPT-OSS 120B (Medium)", "Gemini 3.5 Flash (Low)") or the slug
# (e.g. "gpt-oss-120b-medium", "gemini-3.5-flash-low"). Which one `agy models` prints
# varies by agy version (1.0.x: display names, 1.1.10: slugs). An unknown name is
# rejected with a non-zero exit rather than silently falling back, so a typo just
# fails the step. An empty string omits `--model` and lets agy pick its own default. A legacy `gemini = "..."` key is still
# accepted as an input alias and is promoted to `antigravity` (an explicit `antigravity`
# value wins if both are present).
# Grok CLI supports `-m`: pass a model ID from `grok models` (e.g. "grok-4.5").
# An empty string omits `-m` and lets grok pick its own default.
[models]
antigravity = "GPT-OSS 120B (Medium)"
codex = "gpt-5.6-luna"
claude = "haiku"
opencode = ""
grok = ""

# Provider cooldown (minutes)
provider_cooldown_minutes = 60

# Provider timeout (seconds) per call
provider_timeout_seconds = 60
```

## Configuration Options

| Option | Description | Default |
|--------|-------------|---------|
| `providers` | Provider fallback chain — each entry is a provider name string **or** a `{provider, model, command, env, name}` table (see "Advanced: Provider Fallback Chain" below; `antigravity` recommended, `gemini` accepted as an alias) | `["opencode", "grok", "antigravity", "codex", "claude", "apple-intelligence"]` (`apple-intelligence` only on macOS builds with the `apple-ai` feature) |
| `language` | Commit message language | `"Japanese"` |
| `prefix_type` | Commit prefix format | Auto-detect |
| `auto_push` | Auto-push after commit | `false` |
| `codex_reasoning_effort` | Codex `-c model_reasoning_effort` value (`low`, `medium`, `high`, `xhigh`, or `""` to omit) | `"low"` |
| `models.*` | Model for each provider | See config |
| `provider_cooldown_minutes` | Failed provider cooldown; extremely large values are treated as effectively indefinite | `60` |
| `provider_timeout_seconds` | Provider call timeout | `60` |
| `prefix_rules` | URL-based prefix format | `[]` |
| `prefix_scripts` | External prefix scripts | `[]` |
| `ai_usage` | Residual quota gate via the `ai-usage` CLI (see "Residual Quota Gate") | disabled |
| `dev_log` | Developer generation log (global config only; see "Developer Generation Log") | disabled |

Existing global config files are not rewritten automatically. The current Codex default is `gpt-5.6-luna`; to use it in an existing setup, update `models.codex` in `~/.config/git-sc/config.toml`. **Check this after any Codex CLI update.** The previous default, `gpt-5.4-mini`, has since been removed from Codex, and naming a removed model does not fall back to anything — it returns HTTP 400, which git-sc counts as a provider failure and puts the step into cooldown, so an outdated `models.codex` silently drops Codex out of the fallback chain on every run. This default was reselected on September 17, 2026 (JST) by comparing `input_tokens` for Codex models that are API-visible, listed, and support `medium` reasoning, using `Reply ok.` in an empty directory with `--ignore-user-config --ignore-rules --ephemeral --sandbox read-only` and `model_reasoning_effort='medium'`: `gpt-5.6-luna` = 19609, `gpt-5.5` = 20181, `gpt-5.6-sol` = 21174, `gpt-5.6-terra` = 21174, `gpt-6-astra` = 22035. All runs produced `ok` with no tool calls, and a second round reproduced every figure exactly.

The default Antigravity (`agy`) model is `GPT-OSS 120B (Medium)`, chosen by measurement. `agy` 1.1.10 added `--output-format json` to print mode, which reports a per-request `usage` object, so the same `input_tokens` comparison used for Codex is now possible (earlier agy releases had no machine-readable usage output, and this default originally rested on published pricing instead). Measured August 4, 2026 (JST) with `agy` 1.1.10 using the fixed prompt `Reply ok.` in an empty directory: `gpt-oss-120b-medium` = 13680, `gemini-3.5-flash-medium` = 16994, `gemini-3.5-flash-low` = 16998, `gemini-3.1-pro-low` = 17684, `gemini-3.6-flash-low` = 18175, `gemini-3.6-flash-medium` = 18176, `claude-sonnet-4-6` = 19346. All runs succeeded in a single turn, and `gpt-oss-120b-medium` is the minimum by roughly 19%, so it remains the default. This measures minimal per-request overhead only and says nothing about real-workload quality. To use it in an existing setup, add or update `models.antigravity` in `~/.config/git-sc/config.toml`, or set it to `""` to defer to agy's own default.

Provider cooldown state normalizes legacy aliases before reordering providers, so `gemini`/`agy` cooldown entries still apply to `antigravity`, and legacy `apple-ai` / `apple_intelligence` entries still apply to `apple-intelligence`. Running with `--debug` also prints a one-time notice when a legacy `gemini` provider alias is found in your config, reminding you that it is normalized to `antigravity`.

## Advanced: Provider Fallback Chain (model / account / command)

Each entry in `providers` can be either a plain string (provider name only) **or** a table that also specifies `model`, `command`, and `env`. This lets you build a fallback chain where the *same* provider appears multiple times with different models or accounts — useful when one provider splits its quota per model family or per account/contract.

```toml
providers = [
  # Same provider, different accounts (switch via env: CODEX_HOME / CLAUDE_CONFIG_DIR).
  { provider = "codex", model = "gpt-5.6-luna", env = { CODEX_HOME = "~/.codex" } },       # account 1
  { provider = "codex", model = "gpt-5.6-luna", env = { CODEX_HOME = "~/.codex-work" } },  # account 2
  # Same provider, different model families (separate quotas).
  { provider = "antigravity", model = "Gemini 3.5 Flash (Low)" },
  { provider = "antigravity", model = "GPT-OSS 120B (Medium)" },
  # A plain string is still accepted (provider name only).
  "claude",
]
```

Per-step fields:

| Field | Description |
|-------|-------------|
| `provider` | Required. Provider type that decides the CLI argument convention (`codex`, `antigravity`, `claude`, `opencode`, `grok`, `apple-intelligence`; `gemini`/`agy` accepted as aliases). |
| `model` | Optional. Model for this step. If omitted, falls back to `[models].<provider>`, then the CLI's own default. |
| `command` | Optional. Executable (and fixed args) to run instead of the provider's default binary — e.g. a wrapper script. `~` is expanded. The provider's standard arguments (`--disable hooks` etc. for codex) are still applied. |
| `env` | Optional. Environment variables set explicitly via `Command::env()` when launching this step. `~` in values is expanded; the key must be a valid POSIX name. Dynamic-loader / interpreter pre-load keys (`LD_PRELOAD`, `DYLD_INSERT_LIBRARIES`, `NODE_OPTIONS`, `PYTHONPATH` etc.) are rejected case-insensitively with a config error to prevent code-injection via project-level `.git-sc`. |
| `name` | Optional. Identifier used for the cooldown key and log label. If omitted, it is derived deterministically from provider + model + env + command. |

**Account switching (recommended: `env`).** Codex and Claude Code pick their account/credentials from `CODEX_HOME` / `CLAUDE_CONFIG_DIR`. Setting these per step via `env` lets you fall back across accounts, each with its own quota. git-sc applies them with an explicit `Command::env()` override, so the launched CLI is **not** affected by whatever `CODEX_HOME` / `CLAUDE_CONFIG_DIR` happens to be exported in the shell that runs git-sc. (A wrapper script via `command` works too, but `env` is preferred because it is explicit and shown in `--debug`.)

**Independent cooldown.** The cooldown key includes provider + model + env (+ command, or an explicit `name`), so each step is demoted independently: if `codex` on account 1 hits a rate limit, `codex` on account 2 — and `antigravity` on a different model — stay available.

## prefix_type Values

| Value | Example | Description |
|-------|---------|-------------|
| `conventional` | `feat: add new feature` | Conventional Commits format |
| `bracket` | `[Add] new feature` | Bracket-style prefix |
| `colon` | `Add: new feature` | Simple colon prefix |
| `emoji` | `✨ add new feature` | Emoji prefix |
| `plain` | `Add new feature` | No prefix |
| `none` | `Add new feature` | No prefix (same as `plain`) |

## Prefix Rules

Specify commit format by remote URL:

```toml
[[prefix_rules]]
url_pattern = "github\\.com[:/]myorg/"
prefix_type = "conventional"  # conventional, bracket, colon, emoji, plain, none
```

`prefix_type` in a matching rule must be one of the valid values above. Invalid matching rules are skipped with a warning, so later prefix rules, configured `prefix_type`, or auto detection can still apply.

## Prefix Scripts

Custom prefix generation via external scripts:

```toml
[[prefix_scripts]]
url_pattern = "^https://gitlab\\.example\\.com/"
script = "/path/to/prefix-generate.py"
```

If a prefix script returns a valid `prefix_type` name (e.g. `conventional`, `bracket`, `emoji`, etc.) instead of a literal prefix string, git-sc interprets it as a Rule mode. This allows scripts to dynamically select the commit format based on branch name or remote URL.

For literal prefix strings, trailing line endings (`\n`/`\r\n`) from common script output such as `echo` are removed, while intentional trailing spaces are preserved.

If a prefix script returns empty output (exit `0` with no text), git-sc keeps the generated message as-is, except it removes a leading Conventional Commit type prefix (for example `feat:`, `fix(scope):`, `feat!:`) when present.

If a prefix script exits with code `1`, git-sc keeps the AI-generated message without adding a prefix. Other non-zero exit codes are treated as script execution failures, so git-sc falls back to the next matching prefix script, prefix rule, configured `prefix_type`, or auto detection.

For project-level `.git-sc`, relative `script` paths are resolved from the Git repository root, and the script runs with the Git root as its working directory.

```bash
#!/bin/bash
# Example: return "conventional" to use Conventional Commits format
echo "conventional"
```

## Auto Push

Enable auto-push in your config file:

```toml
# In ~/.config/git-sc/config.toml or .git-sc
auto_push = true
```

When enabled, `git-sc` will run `git push` after a successful commit or squash.

## Residual Quota Gate (`ai-usage`)

If you have the `ai-usage` CLI installed, git-sc can drop providers whose account is nearly out of quota before spending a call on them. Disabled by default.

```toml
[ai_usage]
enabled = true
command = ["ai-usage", "--json"]  # optional (`~` in the executable path is expanded)
threshold_percent = 95            # skip a step at or above this usage
window = "nearest"                # weekly | five_hour | nearest (the higher of the two)
timeout_seconds = 10
```

git-sc runs the command once at startup and checks each step in the fallback chain against the matching account. Steps at or above `threshold_percent` are removed **for that run only** — the provider cooldown state is untouched.

Each provider step can say which account it belongs to:

```toml
[[providers]]
provider = "codex"
ai_usage_profile = "Work"          # exact, case-sensitive match on the ai-usage `profile`
env = { CODEX_HOME = "~/.codex-work" }

[[providers]]
provider = "antigravity"
ai_usage_group = "Claude&GPT"      # case-insensitive match on `group_label`
```

`ai_usage_group` exists because one account's quota can be split per model family — Antigravity reports separate `Gemini` and `Claude&GPT` pools that run out independently. Without `ai_usage_profile`, git-sc judges the step against the least-used account for that provider; note this only affects the *decision*, since the account a step actually runs as is decided by its `env`. Set both if you want the gate and the execution to agree.

Failure handling is deliberately asymmetric: if the command cannot be run, times out, or returns unparseable output, the chain is left as-is and the commit proceeds, because a broken helper must never block a commit. But if the usage data is read successfully and *every* step is over the threshold, git-sc stops with an error rather than falling back to the default chain — falling back would call the very providers the gate just refused.

A project-level `.git-sc` can override individual fields; anything it does not mention keeps the global value.

## Developer Generation Log

Records what prompt was sent and what came back, so prompt changes can be evaluated against real data instead of guesswork. Disabled by default.

```toml
# ~/.config/git-sc/config.toml only — a project .git-sc cannot enable this
[dev_log]
enabled = true
content = "metadata"   # metadata | full
retention_days = 14
max_total_mb = 500
```

Each run writes one JSON file to `~/.config/git-sc/logs/YYYY-MM-DD/`, containing the prompt digest and diff statistics, every provider attempt (raw response before cleanup, model, duration, quality findings, and whether it was accepted, retried, or fell through), and the outcome — including the commit hash when one was made. Files are written to a temporary name and renamed into place, so concurrent `git-sc` runs never interleave, and partially written records never appear as finished ones.

Analyze them by streaming the files into JSONL:

```bash
find ~/.config/git-sc/logs -name '*.json' | sort | xargs jq -c .

# e.g. how often each provider produced a defective subject
find ~/.config/git-sc/logs -name '*.json' | xargs jq -r \
  '.attempts[] | select(.findings | length > 0) | "\(.provider)\t\(.findings[0])"' | sort | uniq -c
```

**Privacy.** `content = "metadata"` (the default) keeps prompt statistics and a digest but not the prompt itself, and drops provider stderr as well — Codex echoes the prompt there, so keeping it would put your diff in the log by another route. `content = "full"` stores the exact prompt, which means your staged diff is written to disk in plain text. Raw provider responses are kept at both levels, since a cleaned-up message alone is not enough to diagnose a bad generation. Environment overrides are recorded by name only, never by value. Logs are created mode `0600` inside `0700` directories, are removed after `retention_days`, and are trimmed oldest-first once they exceed `max_total_mb`. Cleanup runs at most once a day. If a log cannot be written, `git-sc` prints one warning (suppressed under `--quiet`) and commits anyway.

This setting is global-only on purpose: a cloned repository's `.git-sc` must not be able to turn on logging or choose where your code gets written. A project-level `[dev_log]` is ignored with a warning.
