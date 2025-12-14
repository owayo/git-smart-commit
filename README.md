# git-sc

Smart commit message generator using AI CLI (Gemini, Codex, or Claude).

`git-sc` analyzes your staged changes and past commit history to generate contextually appropriate commit messages using AI CLI tools with automatic fallback support.

## Features

- **Multi-Provider Support**: Supports Gemini, Codex, and Claude CLI with automatic fallback
- **Configurable**: Customize provider priority, language, and models via `~/.git-sc`
- **Format Detection**: Automatically detects your commit message format from recent commits:
  - Conventional Commits (`feat:`, `fix:`, `docs:`, etc.)
  - Bracket prefix (`[Add]`, `[Fix]`, `[Update]`, etc.)
  - Colon prefix (`Add:`, `Fix:`, `Update:`, etc.)
  - Emoji prefix
  - Plain format
- **Interactive**: Prompts for confirmation before committing (can be skipped with `-y`)
- **Dry Run**: Preview generated messages without committing
- **Amend Support**: Regenerate message for the last commit with `--amend`

## Prerequisites

At least one of the following AI CLI tools must be installed:

- **Gemini CLI**: `npm install -g @google/gemini-cli`
- **Codex CLI**: `npm install -g @openai/codex`
- **Claude CLI**: `npm install -g @anthropic-ai/claude-code`

## Installation

### From GitHub Releases (Recommended)

Download the latest binary for your platform from [Releases](https://github.com/owa/git-smart-commit/releases).

#### macOS (Apple Silicon)
```bash
curl -L https://github.com/owa/git-smart-commit/releases/latest/download/git-sc-aarch64-apple-darwin.tar.gz | tar xz
sudo mv git-sc /usr/local/bin/
```

#### macOS (Intel)
```bash
curl -L https://github.com/owa/git-smart-commit/releases/latest/download/git-sc-x86_64-apple-darwin.tar.gz | tar xz
sudo mv git-sc /usr/local/bin/
```

#### Linux (x86_64)
```bash
curl -L https://github.com/owa/git-smart-commit/releases/latest/download/git-sc-x86_64-unknown-linux-gnu.tar.gz | tar xz
sudo mv git-sc /usr/local/bin/
```

#### Linux (ARM64)
```bash
curl -L https://github.com/owa/git-smart-commit/releases/latest/download/git-sc-aarch64-unknown-linux-gnu.tar.gz | tar xz
sudo mv git-sc /usr/local/bin/
```

#### Windows

Download `git-sc-x86_64-pc-windows-msvc.zip` from [Releases](https://github.com/owa/git-smart-commit/releases), extract, and add to PATH.

### From Source

```bash
# Clone the repository
git clone https://github.com/owa/git-smart-commit.git
cd git-smart-commit

# Build and install
make install
```

## Configuration

On first run, `git-sc` creates a configuration file at `~/.git-sc`:

```toml
# AI provider priority order (first available will be used)
providers = [
    "gemini",
    "codex",
    "claude",
]

# Language for commit messages
language = "Japanese"

# Model configuration for each provider
[models]
gemini = "flash"
codex = "gpt-5.1-codex-mini"
claude = "haiku"

# Prefix script configuration (optional)
# Execute external scripts to generate commit message prefix based on remote URL
[[prefix_scripts]]
url_pattern = "https://gitlab.example.com/"
script = "/path/to/prefix-generate.py"

[[prefix_scripts]]
url_pattern = "git@gitlab.example.com:"
script = "/path/to/prefix-generate.py"

# Prefix rules configuration (optional)
# Specify prefix format based on remote URL
[[prefix_rules]]
url_pattern = "https://github.com/myorg/"
prefix_type = "conventional"  # conventional, bracket, colon, emoji, plain, none, or custom format

[[prefix_rules]]
url_pattern = "git@github.com:myorg/"
prefix_type = "conventional"
```

### Configuration Options

| Option | Description | Default |
|--------|-------------|---------|
| `providers` | Priority order of AI providers | `["gemini", "codex", "claude"]` |
| `language` | Language for commit messages | `"Japanese"` |
| `models.gemini` | Model for Gemini CLI | `"flash"` |
| `models.codex` | Model for Codex CLI | `"gpt-5.1-codex-mini"` |
| `models.claude` | Model for Claude CLI | `"haiku"` |
| `prefix_scripts` | External scripts for prefix generation | `[]` |
| `prefix_rules` | URL-based prefix format configuration | `[]` |

### Prefix Priority Order

1. **prefix_scripts** (highest priority): Execute script if `url_pattern` matches (prefix match)
2. **prefix_rules**: Use specified `prefix_type` if `url_pattern` matches (prefix match)
3. **Auto** (default): Auto-detect format from last 5 commits

### Prefix Rules

You can specify prefix format based on the remote URL.

| prefix_type | Behavior |
|-------------|----------|
| `conventional` | Conventional Commits format (feat:, fix:, docs:, etc.) |
| `bracket` | Bracket prefix format ([Add], [Fix], [Update], etc.) |
| `colon` | Colon prefix format (Add:, Fix:, Update:, etc.) |
| `emoji` | Emoji prefix format (✨, 🐛, 📝, etc.) |
| `plain` | No prefix (message body only) |
| `none` | No prefix (message body only) |
| other | Use as custom format |

### Prefix Scripts

You can configure external scripts to generate commit message prefixes based on the remote URL. When the remote URL starts with the specified `url_pattern`, the script is executed with the remote URL and branch name as arguments.

**Script Exit Code Behavior:**

| Exit Code | Output | Behavior |
|-----------|--------|----------|
| `0` | Has content | Use the output as custom prefix |
| `0` | Empty | No prefix (message body only) |
| `1` | - | Use AI-generated message as-is |

Example script call:
```bash
/path/to/prefix-generate.py "git@example.com:org/repo.git" "feature/my-branch"
```

Example script (pseudo-code):
```bash
#!/bin/bash
# Generate prefix based on branch name or external API
PREFIX=$(generate_prefix "$1" "$2")
if [ -n "$PREFIX" ]; then
    echo -n "$PREFIX"
    exit 0
else
    exit 1  # Use AI-generated format
fi
```

## Build Commands

| Command | Description |
|---------|-------------|
| `make build` | Build debug version (no version bump) |
| `make release` | Build release version (no version bump) |
| `make release-patch` | Bump patch version and build (0.1.0 → 0.1.1) |
| `make release-minor` | Bump minor version and build (0.1.0 → 0.2.0) |
| `make release-major` | Bump major version and build (0.1.0 → 1.0.0) |
| `make install` | Build release and install to /usr/local/bin |
| `make install-release` | Bump patch, build, and install |
| `make tag-release` | Create a git tag for GitHub Actions release |
| `make tag-release-push` | Create and push tag to trigger release |
| `make test` | Run tests |
| `make fmt` | Format code |
| `make check` | Run clippy and check |
| `make clean` | Clean build artifacts |
| `make help` | Show all available commands |

## Usage

```bash
# Generate commit message for staged changes
git-sc

# Stage all changes (including unstaged) and generate commit message
git-sc -a

# Generate message without confirmation prompt
git-sc -y

# Preview message without committing (dry run)
git-sc -n

# Regenerate message for the last commit (amend)
git-sc --amend

# Override language setting
git-sc -l English

# Combine options
git-sc -a -y           # Stage all and commit without confirmation
git-sc -a -n           # Stage all and preview message
git-sc --amend -y      # Amend last commit without confirmation
```

## Options

| Option | Short | Description |
|--------|-------|-------------|
| `--yes` | `-y` | Skip confirmation prompt and commit directly |
| `--dry-run` | `-n` | Show generated message without actually committing |
| `--all` | `-a` | Stage all changes (including unstaged) and commit |
| `--amend` | | Regenerate message for the last commit |
| `--lang` | `-l` | Override language setting from config |
| `--help` | `-h` | Print help information |
| `--version` | `-V` | Print version information |

## How It Works

1. **Verify Environment**: Checks for git repository and AI CLI installation
2. **Load Config**: Reads settings from `~/.git-sc` (creates default if not exists)
3. **Stage Changes**: Optionally stages all changes with `-a` flag
4. **Get Diff**: Retrieves the staged diff content
5. **Detect Format**: Analyzes recent commits to detect your preferred format
6. **Generate Message**: Sends diff and format instructions to AI CLI (with fallback)
7. **Confirm & Commit**: Shows the message and prompts for confirmation

## Examples

### With Conventional Commits

If your recent commits are:
```
feat: add user authentication
fix(api): resolve rate limiting issue
```

`git-sc` will generate messages like:
```
feat(auth): implement password reset flow
```

### With Bracket Prefix

If your recent commits are:
```
[Add] new feature
[Fix] bug in auth
```

`git-sc` will generate messages like:
```
[Update] refactor user service
```

### Provider Fallback

If Gemini CLI fails or is not installed, `git-sc` automatically tries the next provider:
```
Using Gemini...
⚠ Gemini failed: API Error
Using Codex...
✓ Commit created successfully!
```

## Integration with Claude Code

You can use Claude Code's Hooks feature to automatically generate commit messages when a session ends.

Add the following to `~/.claude/settings.json`:

```json
{
  "hooks": {
    "Stop": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "git-sc --all --yes"
          }
        ]
      }
    ]
  }
}
```

This will automatically commit changes when a Claude Code session ends. If there are no changes, it exits gracefully.

## License

MIT
