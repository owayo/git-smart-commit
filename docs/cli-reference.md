# CLI Reference

Every command and option of git-sc, and what the history-rewriting modes check before they touch anything. For installation and typical examples, see the [README](../README.md).

## Commands

| Command | Description |
|---------|-------------|
| `git-sc` | Generate message for staged changes |
| `git-sc init` | Initialize configuration file (`--force` overwrites an existing one) |
| `git-sc -a` | Stage all changes and generate message |
| `git-sc --amend` | Regenerate message for last commit |
| `git-sc --squash <BASE>` | Squash all commits into one |
| `git-sc --reword <HASH>` | Regenerate message for specific commit |
| `git-sc -g <HASH>` | Generate from existing commit (output only) |

Concurrency guards worth knowing about:

- `--reword` refuses to run while a rebase is in progress. It ends every failed rebase with `git rebase --abort`, so starting one on top of yours would discard your in-progress conflict resolution. Finish or abort your rebase first.
- Committing aborts if the staged content changed while the message was being generated (git-sc compares the index tree before and after). The generated message describes the old content, so committing the new one would be wrong. Just re-run. `--squash` re-checks for newly staged changes the same way, right before it resets.
- Committing, `--amend`, and `--squash` also abort if `HEAD` moved during generation. What `--squash` folds and `--amend` rewrites comes from the history rather than the index, so a commit made in another terminal leaves the index clean and slips past the staged-changes check — a `--squash` would then fold in a commit the AI never saw, and an `--amend` would overwrite a different commit than the one it described.

## Options

### Basic Options

| Option | Short | Description |
|--------|-------|-------------|
| `--yes` | `-y` | Skip confirmation prompt |
| `--dry-run` | `-n` | Show message without committing |
| `--all` | `-a` | Stage all changes |
| `--body` | `-b` | Generate with body (bullet points) |

### Operation Modes

| Option | Short | Description |
|--------|-------|-------------|
| `--amend` | | Regenerate message for last commit |
| `--squash` | | Squash all commits into one |
| `--reword` | | Regenerate message for specific commit |
| `--generate-for` | `-g` | Generate from commit diff (output only) |

Operation modes (`--amend`, `--squash`, `--reword`, `--generate-for`) are mutually exclusive. Combining them fails during argument parsing instead of silently choosing one mode.

`--amend` note:
- Also works when the current `HEAD` is the root commit.
- Preserves unrelated staged changes instead of folding them into the amended commit.
- Refuses when `HEAD` is the boundary commit of a shallow clone (for example right after `git clone --depth 1`). Git has not fetched that commit's parent, so the amended commit would lose it, and a force push would cut the branch history. Run `git fetch --unshallow` first.

`--reword` note:
- Relative refs such as `HEAD~1` and tag names are resolved to a commit ID before generation. Moving a tag during generation does not change the target.
- If `HEAD` moves during generation or confirmation, reword aborts before rewriting history. Re-run it against the updated history.
- Only accepts commits reachable from the current `HEAD` history.
- Passing a hash from another branch (not in current history) fails with an error.
- Passing a merge commit hash also fails, even when the merge commit itself is the reword target.
- Rewording a commit that has a merge commit between it and `HEAD` is rejected with a clear "cannot reword across merge commits" error (merge-spanning reword is unsupported), rather than a confusing low-level git error such as `fatal: ambiguous argument`.
- Rewording the oldest commit in current history is also supported (internally uses `git rebase -i --root` when required).
- Rewording `HEAD` preserves unrelated staged changes instead of folding them into the rewritten commit.
- Lines starting with `#` in the generated message (such as a `#123 ` prefix from a prefix script) are kept for every target commit, the same as when rewording `HEAD`.
- Refuses to rewrite the boundary commit of a shallow clone — the oldest commit you have after cloning with `--depth` — including `HEAD` itself when it is that commit. Rewriting it would drop its parent and cut the history. Run `git fetch --unshallow` first. Commits newer than the boundary, and true root commits, can be reworded as usual.
- The internal rebase runs with `--no-autosquash`, so a user-level `rebase.autoSquash = true` config cannot silently fold `fixup!`/`squash!` commits into the reworded commit.
- The internal rebase also overrides `rebase.rebaseMerges` and `rebase.abbreviateCommands`, so these settings cannot redirect the todo editor or make reword silently do nothing.

`--squash` note:
- Fails before rewriting history when unrelated staged changes already exist. Commit, unstage, or stash them first.
- If the squash commit itself fails (e.g. rejected by a `pre-commit`/`commit-msg` hook or a GPG signing error), the branch is automatically restored to its original `HEAD` instead of being left rewound at the merge-base.

### Settings

| Option | Short | Description |
|--------|-------|-------------|
| `--provider` | `-p` | Use specific AI provider (antigravity, codex, claude, opencode, grok, apple-intelligence). The legacy name `gemini` is accepted as a backward-compatible alias for `antigravity`. |
| `--lang` | `-l` | Override commit message language |

### Debug & Info

| Option | Short | Description |
|--------|-------|-------------|
| `--quiet` | `-q` | Suppress progress messages |
| `--debug` | `-d` | Show prompts sent to AI |
| `--help` | `-h` | Print help |
| `--version` | `-V` | Print version |

`--yes` behavior:
- Required for unattended runs. If the confirmation prompt reaches end-of-file on stdin (for example when git-sc is invoked from a script or hook with stdin closed), the run aborts with an error instead of taking the `[Y/n]` default. An empty line typed by a user still means yes; "no input at all" does not, because the same prompt guards `--amend`, `--squash`, and `--reword`

`--quiet` behavior:
- Suppresses progress output, the formatted preview, and dry-run/cancel messages in normal/amend/squash/reword flows
- Still prints the generated message as one `[provider] message` line and the outcome as one word — `Committed`, `Amended`, `Reworded` or `Squashed`, followed by `Pushed` when auto-push ran — so hooks and scripts can read it
- Keeps error output visible
- In `--generate-for` mode, still prints only the generated commit message (for piping/scripting)

`--debug` behavior:
- In `--generate-for` mode, all debug output (config settings, AI prompt, provider command, streaming output) goes to stderr, so stdout remains the generated message only and stays safe to pipe
- In every other mode debug output goes to stdout as a single block, including when combined with `--quiet`. `--quiet` suppresses progress messages; it does not move debug output to stderr
