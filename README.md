<p align="center">
  <img src="docs/images/app.png" width="128" alt="git-sc">
</p>

<h1 align="center">git-sc</h1>

<p align="center">
  AI-powered smart commit message generator for coding agents
</p>

<p align="center">
  <a href="https://github.com/owayo/git-smart-commit/actions/workflows/release.yml">
    <img alt="Release" src="https://github.com/owayo/git-smart-commit/actions/workflows/release.yml/badge.svg?branch=main">
  </a>
  <a href="https://github.com/owayo/git-smart-commit/actions/workflows/ci.yml">
    <img alt="CI" src="https://github.com/owayo/git-smart-commit/actions/workflows/ci.yml/badge.svg?branch=main">
  </a>
  <a href="https://github.com/owayo/git-smart-commit/releases/latest">
    <img alt="Version" src="https://img.shields.io/github/v/release/owayo/git-smart-commit">
  </a>
  <a href="LICENSE">
    <img alt="License" src="https://img.shields.io/github/license/owayo/git-smart-commit">
  </a>
</p>

<p align="center">
  <a href="README.md">English</a> |
  <a href="README.ja.md">日本語</a>
</p>

---

## Features

- **Multi-Provider Support**: Supports Gemini CLI, Codex CLI, Claude Code, opencode, and Apple Intelligence with automatic fallback
- **Smart Cooldown**: Automatically demotes failed providers for 1 hour (configurable)
- **Format Detection**: Detects commit format from recent commits (Conventional, Bracket, Emoji, etc.)
- **Interactive**: Prompts for confirmation before committing (skip with `-y`)
- **Dry Run**: Preview generated messages without committing (`-n`)
- **Quiet Mode**: Suppresses progress output for hook/scripting use (`-q`)
- **Body Support**: Generate detailed commit messages with bullet points (`-b`)
- **Amend/Squash/Reword**: Regenerate messages for existing commits
- **Agent Context**: Integrates with [claw-hooks](https://github.com/owayo/claw-hooks) to generate context-aware messages reflecting the agent's intent

## Requirements

- **OS**: macOS, Linux, Windows
- **Git**: Required
- **AI Provider** (at least one):
  - Gemini CLI: `npm install -g @google/gemini-cli`
  - Codex CLI: `npm install -g @openai/codex`
  - Claude Code: `curl -fsSL https://claude.ai/install.sh | bash`
  - opencode: `curl -fsSL https://opencode.ai/install | bash`
  - Apple Intelligence: Built-in on macOS (macOS 26+ with Apple Silicon required)

## Installation

### Homebrew (macOS/Linux)

```bash
brew install owayo/git-sc/git-sc
```

### From Source

```bash
git clone https://github.com/owayo/git-smart-commit.git
cd git-smart-commit
make install
```

### From GitHub Releases

Download the latest binary from [Releases](https://github.com/owayo/git-smart-commit/releases).

#### macOS (Apple Silicon)

```bash
curl -L https://github.com/owayo/git-smart-commit/releases/latest/download/git-sc-aarch64-apple-darwin.tar.gz | tar xz
sudo mv git-sc /usr/local/bin/
```

#### macOS (Intel)

```bash
curl -L https://github.com/owayo/git-smart-commit/releases/latest/download/git-sc-x86_64-apple-darwin.tar.gz | tar xz
sudo mv git-sc /usr/local/bin/
```

#### Linux (x86_64)

```bash
curl -L https://github.com/owayo/git-smart-commit/releases/latest/download/git-sc-x86_64-unknown-linux-gnu.tar.gz | tar xz
sudo mv git-sc /usr/local/bin/
```

#### Linux (ARM64)

```bash
curl -L https://github.com/owayo/git-smart-commit/releases/latest/download/git-sc-aarch64-unknown-linux-gnu.tar.gz | tar xz
sudo mv git-sc /usr/local/bin/
```

#### Windows

Download `git-sc-x86_64-pc-windows-msvc.zip` from [Releases](https://github.com/owayo/git-smart-commit/releases), extract, and add to PATH.

## Quickstart

```bash
# Generate commit message for staged changes
git-sc

# Stage all and commit without confirmation
git-sc -a -y

# Preview message (dry run)
git-sc -n
```

## Usage

### Commands

| Command | Description |
|---------|-------------|
| `git-sc` | Generate message for staged changes |
| `git-sc init` | Initialize configuration file |
| `git-sc -a` | Stage all changes and generate message |
| `git-sc --amend` | Regenerate message for last commit |
| `git-sc --squash <BASE>` | Squash all commits into one |
| `git-sc --reword <HASH>` | Regenerate message for specific commit |
| `git-sc -g <HASH>` | Generate from existing commit (output only) |

### Options

#### Basic Options

| Option | Short | Description |
|--------|-------|-------------|
| `--yes` | `-y` | Skip confirmation prompt |
| `--dry-run` | `-n` | Show message without committing |
| `--all` | `-a` | Stage all changes |
| `--body` | `-b` | Generate with body (bullet points) |

#### Operation Modes

| Option | Short | Description |
|--------|-------|-------------|
| `--amend` | | Regenerate message for last commit |
| `--squash` | | Squash all commits into one |
| `--reword` | | Regenerate message for specific commit |
| `--generate-for` | `-g` | Generate from commit diff (output only) |

#### Settings

| Option | Short | Description |
|--------|-------|-------------|
| `--lang` | `-l` | Override commit message language |

#### Debug & Info

| Option | Short | Description |
|--------|-------|-------------|
| `--quiet` | `-q` | Suppress progress messages |
| `--debug` | `-d` | Show prompts sent to AI |
| `--help` | `-h` | Print help |
| `--version` | `-V` | Print version |

`--quiet` behavior:
- Suppresses progress, preview, and success/cancel messages in normal/amend/squash/reword flows
- Keeps error output visible
- In `--generate-for` mode, still prints only the generated commit message (for piping/scripting)

### Examples

```bash
# Basic usage
git-sc                      # Generate for staged changes
git-sc -a -y                # Stage all and commit directly

# Preview and body
git-sc -n                   # Dry run (preview only)
git-sc -b                   # Include detailed body

# Amend and squash
git-sc --amend              # Regenerate last commit message
git-sc --squash origin/main # Squash feature branch commits

# Generate from existing commits
git-sc -g abc1234           # Generate from commit diff
git-sc -g abc1234 -b        # With detailed body
```

## Configuration

### Initial Setup

Initialize configuration with `git-sc init`, or create `~/.config/git-sc/config.toml` manually:

```bash
git-sc init
```

This creates a configuration file with default settings at `~/.config/git-sc/config.toml`.

Use `--force` to overwrite an existing configuration:

```bash
git-sc init --force
```

### Hierarchical Configuration

git-sc supports hierarchical configuration with project-level overrides:

| File | Scope | Description |
|------|-------|-------------|
| `~/.config/git-sc/config.toml` | Global | User-wide default settings |
| `.git-sc` | Project | Repository-specific overrides (in repo root) |

Project settings override global settings. Fields not specified in project config inherit from global config.

### Example Configuration

```toml
# AI provider priority
providers = ["opencode", "gemini", "codex", "claude", "apple-intelligence"]

# Commit message language
language = "Japanese"

# Commit prefix format (optional)
# Values: conventional, bracket, colon, emoji, plain, none
prefix_type = "conventional"

# Auto-push after commit (optional)
auto_push = true

# Model configuration
[models]
gemini = "gemini-2.5-flash-lite"
codex = "gpt-5.1-codex-mini"
claude = "haiku"
opencode = "opencode/minimax-m2.1-free"

# Provider cooldown (minutes)
provider_cooldown_minutes = 60

# Provider timeout (seconds) per call
provider_timeout_seconds = 30
```

### Configuration Options

| Option | Description | Default |
|--------|-------------|---------|
| `providers` | AI provider priority | `["opencode", "gemini", "codex", "claude", "apple-intelligence"]` |
| `language` | Commit message language | `"Japanese"` |
| `prefix_type` | Commit prefix format | Auto-detect |
| `auto_push` | Auto-push after commit | `false` |
| `models.*` | Model for each provider | See config |
| `provider_cooldown_minutes` | Failed provider cooldown | `60` |
| `provider_timeout_seconds` | Provider call timeout | `30` |
| `prefix_rules` | URL-based prefix format | `[]` |
| `prefix_scripts` | External prefix scripts | `[]` |

### prefix_type Values

| Value | Example | Description |
|-------|---------|-------------|
| `conventional` | `feat: add feature` | Conventional Commits format |
| `bracket` | `[feat] add feature` | Bracket-style prefix |
| `colon` | `feat: add feature` | Simple colon prefix |
| `emoji` | `:sparkles: add feature` | Emoji prefix |
| `plain` | `Add feature` | No prefix |
| `none` | `add feature` | No prefix, lowercase |

### Prefix Rules

Specify commit format by remote URL:

```toml
[[prefix_rules]]
url_pattern = "github\\.com[:/]myorg/"
prefix_type = "conventional"  # conventional, bracket, colon, emoji, plain
```

### Prefix Scripts

Custom prefix generation via external scripts:

```toml
[[prefix_scripts]]
url_pattern = "^https://gitlab\\.example\\.com/"
script = "/path/to/prefix-generate.py"
```

If a prefix script returns a valid `prefix_type` name (e.g. `conventional`, `bracket`, `emoji`, etc.) instead of a literal prefix string, git-sc interprets it as a Rule mode. This allows scripts to dynamically select the commit format based on branch name or remote URL.

If a prefix script returns empty output (exit `0` with no text), git-sc keeps the generated message as-is, except it removes a leading Conventional Commit type prefix (for example `feat:`, `fix(scope):`, `feat!:`) when present.

```bash
#!/bin/bash
# Example: return "conventional" to use Conventional Commits format
echo "conventional"
```

## Diff Processing

- Whitespace-only changes excluded
- Binary files excluded
- `.git-sc-ignore` patterns applied
- Truncated at 10,000 characters

### .git-sc-ignore

```gitignore
package-lock.json
yarn.lock
Cargo.lock
*.generated.ts
```

### Auto Push

Enable auto-push in your config file:

```toml
# In ~/.config/git-sc/config.toml or .git-sc
auto_push = true
```

When enabled, `git-sc` will run `git push` after a successful commit or squash.

## VS Code Extension

**[Git-SC (Smart Commit)](https://marketplace.visualstudio.com/items?itemName=owayo.vscode-git-smart-commit)** - Available on VS Code Marketplace

## Claude Code Integration

Add to `~/.claude/settings.json`:

```json
{
  "hooks": {
    "Stop": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "git-sc --all --yes --quiet"
          }
        ]
      }
    ]
  }
}
```

## Agent Context (claw-hooks Integration)

When git-sc is used with [claw-hooks](https://github.com/owayo/claw-hooks), it automatically receives context about what the coding agent was working on via the `CLAW_HOOKS_AGENT_MESSAGE` environment variable. This context is included in the AI prompt, enabling the generated commit message to reflect the high-level intent rather than just describing the raw diff.

**claw-hooks** is a companion tool that manages Claude Code's hook lifecycle. When its stop hook fires, it sets `CLAW_HOOKS_AGENT_MESSAGE` with the agent's last activity summary before invoking git-sc.

```bash
# Automatically set by claw-hooks stop hook
# CLAW_HOOKS_AGENT_MESSAGE="Refactored authentication to use JWT tokens"
git-sc -a -y -q
```

When the environment variable is set, the prompt includes an "Agent Context" section that guides the AI to prioritize the developer's intent.
This applies to standard commit generation as well as `--amend`, `--reword`, `--squash`, and `--generate-for`.

## How It Works

```mermaid
flowchart LR
    A[Stage Changes] --> B[Get Diff]
    B --> C[Detect Format]
    C --> D[Generate via AI]
    D --> E[Confirm & Commit]
```

1. **Verify**: Check git repo and AI agent availability
2. **Config**: Load `~/.config/git-sc/config.toml` settings
3. **Diff**: Get staged changes (with exclusions)
4. **Format**: Detect from recent commits or rules
5. **Generate**: Send to AI with fallback
6. **Commit**: Confirm and create commit

## Apple Intelligence

Apple Intelligence provider uses [fm-rs](https://github.com/blacktop/fm-rs) (Rust bindings for Apple's [Foundation Models](https://developer.apple.com/documentation/foundationmodels) framework) for fully on-device inference. No API key or network connection is required.

- **Requirements**: macOS 26 (Tahoe) or later, Apple Silicon, Apple Intelligence enabled in System Settings
- **How it works**: With Apple Intelligence enabled (default on macOS), git-sc calls Foundation Models directly via fm-rs. A `LanguageModelSession` is created with commit-message-specific instructions for each generation.
- **Build**: `cargo build --features apple-ai` (automatic with `make build` / `make install` on macOS)
- **Cross-platform**: On Linux/Windows, Apple Intelligence is not available and is automatically skipped

## Build Commands

| Command | Description |
|---------|-------------|
| `make build` | Build debug version |
| `make release` | Build release version |
| `make install` | Build and install to /usr/local/bin |
| `make test` | Run tests |
| `make fmt` | Format code |
| `make check` | Run clippy and check |
| `make clean` | Clean build artifacts |

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## Changelog

See [Releases](https://github.com/owayo/git-smart-commit/releases) for version history.

## License

[MIT](LICENSE)
