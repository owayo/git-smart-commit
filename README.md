<p align="center">
  <img src="docs/images/app.png" width="128" alt="git-sc">
</p>

<h1 align="center">git-sc</h1>

<p align="center">
  AI-powered smart commit message generator for coding agents
</p>

<!-- standard:badges:start -->
<h3 align="center">Supported Platforms</h3>

<p align="center">
  <img src="https://img.shields.io/badge/Linux-FCC624?logo=linux&amp;logoColor=black" alt="Linux">
  <img src="https://img.shields.io/badge/macOS-000000?logo=apple&amp;logoColor=white" alt="macOS">
  <img src="https://img.shields.io/badge/Windows-0078D6" alt="Windows">
</p>

<p align="center">
  <a href="https://github.com/owayo/git-smart-commit/actions/workflows/ci.yml"><img src="https://github.com/owayo/git-smart-commit/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
  <a href="https://github.com/owayo/git-smart-commit/releases/latest"><img src="https://img.shields.io/github/v/release/owayo/git-smart-commit" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/owayo/git-smart-commit" alt="License"></a>
</p>

<p align="center">
  <a href="README.md">English</a> |
  <a href="README.ja.md">日本語</a>
</p>
<!-- standard:badges:end -->

---

git-sc reads your staged diff, detects the commit format the repository already uses, and asks an AI you already have access to (Antigravity CLI, Codex CLI, Claude Code, opencode, Grok CLI, or on-device Apple Intelligence) to write the message. When a provider fails or runs out of quota, the next step of your fallback chain takes over.

It is built to run unattended from a coding agent's stop hook (`git-sc --all --yes --quiet`), so the work an agent leaves behind is committed with a message that describes it. git-sc holds no API keys of its own; each CLI provider uses its own login.

## Features

- **Multi-Provider Support**: Supports Antigravity CLI (`agy`, the successor of Gemini CLI), Codex CLI, Claude Code, opencode, Grok CLI, and Apple Intelligence with automatic fallback. The same provider can appear multiple times with different models or accounts ([provider fallback chain](docs/configuration.md))
- **Smart Cooldown**: Automatically demotes failed steps for 1 hour (configurable), keyed per provider+model+account so one rate-limited account or model does not block the others
- **Format Detection**: Detects commit format from recent commits (Conventional, Bracket, Emoji, etc.)
- **Empty Repo Safe**: Auto format detection falls back cleanly even when the repository has no commits and Git outputs localized messages
- **Interactive**: Prompts for confirmation before committing (skip with `-y`)
- **Dry Run**: Preview generated messages without committing (`-n`)
- **Quiet Mode**: Suppresses progress output for hook/scripting use (`-q`)
- **Body Support**: Generate detailed commit messages with bullet points (`-b`)
- **Amend/Squash/Reword**: Regenerate messages for existing commits
- **Private Temp Files**: AI prompts, Codex final-output files, and reword message files are created without group/other read permissions on Unix/macOS
- **Agent Context**: Integrates with [claw-hooks](https://github.com/owayo/claw-hooks) to generate context-aware messages reflecting the agent's intent

## Requirements

- **Git**: Required
- **AI Provider** (at least one):
  - Antigravity CLI (`agy`, successor of Gemini CLI): see https://antigravity.google/docs/gcli-migration (the legacy Gemini CLI stopped serving requests on 2026-06-18)
  - Codex CLI: `npm install -g @openai/codex`
  - Claude Code: `curl -fsSL https://claude.ai/install.sh | bash`
  - opencode: `curl -fsSL https://opencode.ai/install | bash`
  - Grok CLI (`grok`, xAI): bundled with [cmux](https://github.com/manaflow-ai/cmux) at `/Applications/cmux.app/Contents/Resources/bin/grok`. If `grok` is not on `PATH`, the step is skipped and the chain moves on.
  - Apple Intelligence: macOS 26+ with Apple Silicon, and only in builds from source (see below)

How git-sc launches each provider, and what differs per platform, is in [docs/providers.md](docs/providers.md).

## Installation

<!-- standard:install:start -->
### Homebrew (macOS/Linux)

```bash
brew install owayo/git-sc/git-sc
```

### winget (Windows)

```powershell
winget install owayo.git-sc
```

### Cargo

Requires Rust 1.98 or later.

```bash
cargo install --git https://github.com/owayo/git-smart-commit --locked
```

### From GitHub Releases

Download the archive for your platform from [Releases](https://github.com/owayo/git-smart-commit/releases/latest), extract it, and put `git-sc` on your `PATH`. Each release also includes `SHA256SUMS` for checking the downloads.

| Platform | Archive |
|---|---|
| Linux (x86_64) | `git-sc-x86_64-unknown-linux-gnu.tar.gz` |
| Linux (ARM64) | `git-sc-aarch64-unknown-linux-gnu.tar.gz` |
| macOS (Intel) | `git-sc-x86_64-apple-darwin.tar.gz` |
| macOS (Apple Silicon) | `git-sc-aarch64-apple-darwin.tar.gz` |
| Windows (x86_64) | `git-sc-x86_64-pc-windows-msvc.zip` |

On macOS, if you downloaded the archive with a browser, remove the quarantine attribute before running it: `xattr -d com.apple.quarantine git-sc`.

### From Source

Requires [mise](https://mise.jdx.dev/) (the Rust toolchain is pinned in `mise.toml`).

```bash
git clone https://github.com/owayo/git-smart-commit.git
cd git-smart-commit
make install
```

`make install` installs to `/usr/local/bin`. Set `INSTALL_PATH` to change it (for example `make install INSTALL_PATH="$HOME/.local/bin"`).
<!-- standard:install:end -->

After installing with winget, open a new terminal: the portable package updates your `PATH`, and already-running shells do not pick up the change.

Apple Intelligence is not included in the Homebrew, winget, Cargo, or GitHub Releases builds. On macOS, installing from source enables it: `make install` builds with the `apple-ai` feature, copies it to a temporary file, and then atomically replaces the installed binary (this avoids stale per-inode code-signature validation after reinstalling over an existing command).

## Quickstart

```bash
# Optional: write ~/.config/git-sc/config.toml with the defaults (without it, the defaults apply)
git-sc init

# Preview the message for the staged changes without committing
git-sc -n

# Generate the message, confirm it, and commit
git-sc
```

## Usage

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

When you run it from a script or a hook:

- `--yes` is required for unattended runs. When stdin is closed (a script or hook), the confirmation prompt aborts with an error instead of taking the `[Y/n]` default.
- `--quiet` suppresses progress output but keeps errors. Combined with `--generate-for`, stdout carries only the generated message, so it is safe to pipe.
- `--amend`, `--squash`, and `--reword` rewrite history, and they stop rather than guess when the repository changes under them (a rebase in progress, or `HEAD` moving while the message is generated).

For every command and option, and exactly what each history-rewriting mode checks before it runs, see [docs/cli-reference.md](docs/cli-reference.md).

## Configuration

git-sc works without a configuration file. Settings come from two optional files, and the project file overrides the global one field by field:

| File | Scope | Description |
|------|-------|-------------|
| `~/.config/git-sc/config.toml` | Global | User-wide default settings (`git-sc init` writes it; `--force` overwrites) |
| `.git-sc` | Project | Repository-specific overrides, read from the root of the Git repository |

```toml
# Provider fallback chain, tried in this order
providers = ["opencode", "grok", "antigravity", "codex", "claude", "apple-intelligence"]
language = "Japanese"            # Commit message language
prefix_type = "conventional"     # conventional, bracket, colon, emoji, plain, none (default: auto-detect)
auto_push = true                 # Run git push after a successful commit (default: false)

[models]
antigravity = "GPT-OSS 120B (Medium)"
codex = "gpt-5.6-luna"
claude = "haiku"
opencode = ""                    # "" leaves the choice to the CLI's own default
grok = ""
```

- Existing config files are not rewritten when a default changes. After a Codex CLI update, check that `models.codex` still names a model Codex serves: a removed model fails every call and silently drops Codex out of the chain.
- A project `.git-sc` can name executables for git-sc to run (`providers[].command`, `prefix_scripts[].script`, `ai_usage.command`). Review it before running git-sc in a repository you do not trust.
- Files matching the patterns in `.git-sc-ignore` (gitignore syntax, at the repository root) are left out of the diff sent to the AI.

Every option, and the details of the fallback chain, prefixes, the `ai-usage` quota gate, and the developer generation log, are in [docs/configuration.md](docs/configuration.md). What git-sc sends to the AI, and how it protects that data, is in [docs/diff-processing.md](docs/diff-processing.md).

## Claude Code Integration

Add to `~/.claude/settings.json` to commit whenever Claude Code stops:

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

With [claw-hooks](https://github.com/owayo/claw-hooks), the agent's last activity summary reaches git-sc through `CLAW_HOOKS_AGENT_MESSAGE`, and the message reflects the intent behind the change rather than only the raw diff. The [Git-SC (Smart Commit)](https://marketplace.visualstudio.com/items?itemName=owayo.vscode-git-smart-commit) extension brings git-sc to VS Code. More on both in [docs/integrations.md](docs/integrations.md).

## Development

<!-- standard:dev:start -->
Requires [mise](https://mise.jdx.dev/). Tool versions are pinned in `mise.toml`.

```bash
make setup   # Install the toolchain (mise) and dependencies
make ci      # Run the same checks as CI (no changes)
```

| Command | Description |
|---|---|
| `make setup` | Install the toolchain (mise) and dependencies |
| `make build` | Build a debug binary |
| `make release` | Build a release binary |
| `make test` | Run the tests |
| `make lint` | Run clippy with warnings as errors |
| `make fmt` | Format the code (rewrites files) |
| `make fmt-check` | Check the formatting (no changes) |
| `make check` | Run fmt-check and lint (no changes) |
| `make ci` | Run the same checks as CI (no changes) |
| `make install` | Install the release binary to INSTALL_PATH (default /usr/local/bin) |
| `make uninstall` | Remove the binary from INSTALL_PATH |
| `make clean` | Remove build artifacts |

Run `make` to list every target. Releases are published from GitHub Actions (**Actions → Release → Run workflow**).
<!-- standard:dev:end -->

On macOS, `make build`, `make release`, `make install`, and `make lint` compile the Apple Intelligence provider, which builds Swift code through [fm-rs](https://github.com/blacktop/fm-rs) and needs Xcode with the macOS 26 SDK or later. `make lint` runs clippy both without and with that feature, because the Linux CI and the released binaries build without it.

## License

<!-- standard:license:start -->
[MIT](LICENSE)
<!-- standard:license:end -->
