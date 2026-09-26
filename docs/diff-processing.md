# Diff Processing and Security

What git-sc sends to the AI provider, how it trims the staged diff, and what it does to keep files and data you did not mean to share out of the prompt. Settings mentioned here are described in [configuration.md](configuration.md).

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

## Diff Processing

- Whitespace-only changes excluded
- Binary files replaced with a one-line summary (e.g. `[Binary] modified: <path>`) instead of their contents
- Lockfile contents replaced with a path and change kind (added, modified, deleted, renamed, or copied), e.g. `[Lockfile] modified: Cargo.lock`. Renames and copies retain both paths; control characters such as newlines in paths are escaped.
- Quoted diff headers with spaces or non-ASCII file paths are handled correctly
- `.git-sc-ignore` patterns applied
- Truncated at 10,000 characters

Lockfile summaries need no configuration and apply to normal commits, amend, squash, reword, and `--generate-for`. Matching filenames in any directory are `*.lock`, `*.lockb`, `*.lockfile`, `package-lock.json`, `npm-shrinkwrap.json`, `pnpm-lock.yaml`, `go.sum`, and `Package.resolved`. The literal filenames `.lock`, `.lockb`, and `.lockfile` also match. The suffix rules cover files such as `Cargo.lock`, `yarn.lock`, `Gemfile.lock`, `composer.lock`, `uv.lock`, and `bun.lock`.

Summaries are produced before the character limit is applied, so large lockfile contents do not crowd out other changes. Lockfile-only changes can still generate a message. Matching `.git-sc-ignore` patterns exclude the entire summary, including its path.

## Security Notes

- AI prompts may contain staged diff content. When git-sc needs a temporary prompt file for providers such as opencode, or a final-output file for Codex, it creates the file with no group/other permissions on Unix/macOS and removes it automatically after use.
- Reword message temporary files use the same private-file behavior.
- The provider cooldown state file (`~/.config/git-sc/.providers-state`) is also created with no group/other permissions, because its cooldown keys embed each step's `env` values verbatim.
- **`.git-sc-ignore` failures stop the run.** If the file exists but cannot be read or parsed, git-sc exits with an error instead of continuing without exclusions. A malformed ignore file would otherwise silently send the very files you meant to withhold to the AI provider.
- **`.git-sc-ignore` is not affected by your diff-formatting Git config.** Exclusion works by reading file paths out of the `diff --git a/… b/…` header, so settings that reshape that line — `diff.noprefix`, `diff.mnemonicPrefix`, `diff.srcPrefix` / `diff.dstPrefix`, `color.ui = always`, `diff.external` — would otherwise make every pattern silently stop matching. git-sc requests the diff with the prefixes, colors, and path base pinned, so your patterns apply the same way regardless of those settings. This also covers `diff.relative`, which additionally would have hidden any change outside the directory you ran git-sc from — with it pinned, the message is always written from the full staged diff no matter which subdirectory you are in. The submodule settings are pinned too: `diff.submodule = log` would reshape a submodule's block header so patterns stopped matching it, and `diff.ignoreSubmodules = all` — including when it comes from a repository's committed `.gitmodules` rather than your own config — would drop staged submodule pointer changes from the diff and from the staged-changes check, so `--squash` could fold in a change the AI never saw.
- **A project-level `.git-sc` can run code.** `providers[].command`, `prefix_scripts[].script`, and `ai_usage.command` name executables that git-sc launches, and a repository-local `.git-sc` is merged in like any other config. Cloning an untrusted repository and running git-sc in it — including automatically, via an agent stop hook — therefore executes whatever those fields point at. `env` keys are validated and dynamic-loader / interpreter pre-load names are rejected, but that does not constrain these three fields. Review a repository's `.git-sc` before running git-sc inside it, the same way you would review a `Makefile` or a git hook.

## .git-sc-ignore

Place `.git-sc-ignore` at the root of the Git repository and write gitignore-style patterns in it. Patterns are matched against the decoded Git path, so quoted diff headers such as Japanese filenames escaped by Git are excluded correctly as well. Rename diffs are checked against both the source path and destination path, so moving a file into an ignored directory is excluded consistently too. Filenames containing spaces are also supported: Git does not quote space-only filenames in `diff --git` headers, but `git-sc` still extracts the correct path so that ignore patterns apply consistently. This also covers rename headers where the source and destination paths differ and both paths contain spaces, as well as mixed headers where only one side is quoted (e.g. renaming `old name.txt` to a non-ASCII filename that Git quotes).

```gitignore
package-lock.json
yarn.lock
Cargo.lock
*.generated.ts
```
