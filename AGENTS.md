# git-sc (Git Smart Commit)

AI-powered smart commit message generator CLI tool written in Rust.

## Project Overview

This CLI tool generates commit messages using AI coding agents (opencode, Gemini CLI, Codex CLI, Claude Code) with automatic provider fallback and format detection.

## Quick Reference

### Build Commands
```bash
make build      # Debug build
make release    # Release build (optimized)
make install    # Build and install to /usr/local/bin
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
git-sc --debug      # Show AI prompt and command being executed
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
| `AiService` | Multi-provider AI with fallback (opencode → gemini → codex → claude) |
| `GitService` | Git operations (diff, commit, amend, squash, reword) |
| `Config` | Hierarchical config: global (~/.config/git-sc/config.toml) + project (.git-sc) |
| `ProviderState` | Tracks failed providers with 1-hour cooldown |

### AI Provider Implementation

Each provider is called via CLI subprocess:
- **opencode**: Uses temp file with `-f` flag to avoid command line length limits
- **gemini/codex/claude**: Uses stdin for prompt input

When a provider fails, it enters cooldown (default: 60 minutes) and the next provider is tried.

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

Tests are organized with `#[cfg(test)]` modules in each source file.

## Dependencies

- **clap**: CLI argument parsing with derive macros
- **anyhow/thiserror**: Error handling
- **serde/toml**: Configuration parsing
- **colored**: Terminal output styling
- **regex**: Commit format detection
- **ignore**: Gitignore-style pattern matching

---

# AI-DLC and Spec-Driven Development

Kiro-style Spec Driven Development implementation on AI-DLC (AI Development Life Cycle)

## Project Context

### Paths
- Steering: `.kiro/steering/`
- Specs: `.kiro/specs/`

### Steering vs Specification

**Steering** (`.kiro/steering/`) - Guide AI with project-wide rules and context
**Specs** (`.kiro/specs/`) - Formalize development process for individual features

### Active Specifications
- Check `.kiro/specs/` for active specifications
- Use `/kiro:spec-status [feature-name]` to check progress

## Development Guidelines
- Think in English, generate responses in Japanese. All Markdown content written to project files (e.g., requirements.md, design.md, tasks.md, research.md, validation reports) MUST be written in the target language configured for this specification (see spec.json.language).

## Minimal Workflow
- Phase 0 (optional): `/kiro:steering`, `/kiro:steering-custom`
- Phase 1 (Specification):
  - `/kiro:spec-init "description"`
  - `/kiro:spec-requirements {feature}`
  - `/kiro:validate-gap {feature}` (optional: for existing codebase)
  - `/kiro:spec-design {feature} [-y]`
  - `/kiro:validate-design {feature}` (optional: design review)
  - `/kiro:spec-tasks {feature} [-y]`
- Phase 2 (Implementation): `/kiro:spec-impl {feature} [tasks]`
  - `/kiro:validate-impl {feature}` (optional: after implementation)
- Progress check: `/kiro:spec-status {feature}` (use anytime)

## Development Rules
- 3-phase approval workflow: Requirements → Design → Tasks → Implementation
- Human review required each phase; use `-y` only for intentional fast-track
- Keep steering current and verify alignment with `/kiro:spec-status`
- Follow the user's instructions precisely, and within that scope act autonomously: gather the necessary context and complete the requested work end-to-end in this run, asking questions only when essential information is missing or the instructions are critically ambiguous.

## Steering Configuration
- Load entire `.kiro/steering/` as project memory
- Default files: `product.md`, `tech.md`, `structure.md`
- Custom files are supported (managed via `/kiro:steering-custom`)
