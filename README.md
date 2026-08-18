# gh-plugin-rm-generated-by

A [GitHub CLI](https://cli.github.com/) extension that strips AI-generated
trailers — such as `🤖 Generated with [Claude Code](...)` and
`Co-authored-by: Claude <noreply@anthropic.com>` — from a pull request's
**description**.

It can run on demand, or as a small middleware that fires automatically right
after every `gh pr create`.

The extension is written in Rust and distributed as a precompiled binary.

## Install

```sh
gh extension install tomMoulard/gh-plugin-rm-generated-by
```

This downloads the prebuilt binary for your OS/arch from the latest
[release](https://github.com/tomMoulard/gh-plugin-rm-generated-by/releases).

### Build from source (local development)

`gh` only auto-builds Go extensions, so build the binary first, then install the
directory:

```sh
cargo build --release
cp target/release/gh-plugin-rm-generated-by .   # gh runs ./gh-<name>
gh extension install .
```

Run the tests with `cargo test`.

## Usage

```sh
# Clean the PR of the current branch
gh plugin-rm-generated-by

# Clean a specific PR (number, URL, or branch name)
gh plugin-rm-generated-by 123
gh plugin-rm-generated-by https://github.com/owner/repo/pull/123

# Preview the result without editing the PR
gh plugin-rm-generated-by --dry-run

# Clean every open PR you authored, across all repositories
gh plugin-rm-generated-by clean-all
gh plugin-rm-generated-by clean-all --dry-run          # preview only
gh plugin-rm-generated-by clean-all --limit 300        # raise the search cap (default 100)

# Pipe a body through the filter (no API calls) — handy for scripts/CI
gh pr view 123 --json body -q .body | gh plugin-rm-generated-by filter
```

`clean-all` finds your open PRs with `gh search prs --author=@me --state=open`
and cleans each one (reporting the PR URL, since numbers repeat across repos).

## Automatic mode: wrap `gh pr create`

`gh` cannot transparently hook into its own `pr create` command, so this
extension ships a tiny shell function that wraps `gh`: it runs the real
`gh pr create`, then cleans the newly created PR.

Install it into your shell rc (auto-detects `~/.zshrc` / `~/.bashrc`):

```sh
gh plugin-rm-generated-by install
# then restart your shell, or: source ~/.zshrc
```

Or wire it up yourself:

```sh
# in ~/.zshrc or ~/.bashrc
eval "$(gh plugin-rm-generated-by shell-init)"
```

From then on, `gh pr create ...` opens the PR and immediately removes the
trailer from its description. Everything else about `gh` is untouched.

Prefer not to override `gh`? Use the explicit wrapper instead:

```sh
gh plugin-rm-generated-by create --fill --base main
```

## Automatic mode: Claude Code hook

The shell wrapper only exists in interactive shells. Claude Code's Bash tool
runs commands non-interactively, so its `gh pr create` calls bypass the
wrapper entirely and the PR keeps the trailer. Cover that path with a Claude
Code [PostToolUse hook](https://code.claude.com/docs/en/hooks):

```sh
gh plugin-rm-generated-by install claude
```

This registers the hook in `~/.claude/settings.json`:

```json
{
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Bash",
        "hooks": [
          { "type": "command", "command": "gh plugin-rm-generated-by claude-hook" }
        ]
      }
    ]
  }
}
```

After every Bash tool call whose command ran `gh pr create` (or `gh pr edit`),
`claude-hook` pulls the PR URL out of the tool output and strips the trailers.
When no URL is visible (redirected output), it falls back to the current
branch's PR in the directory the command ran in. All other Bash calls exit
immediately, and the hook always exits 0 so it never disrupts the session.

Tip: you can also stop Claude Code from generating the text in the first place
by setting `"attribution": { "commit": "", "pr": "" }` in
`~/.claude/settings.json` — the hook then acts as a safety net.

## What gets removed

A line is dropped when it is any of:

- `🤖 Generated with ...` (emoji + "generated with")
- a line containing both "generated with" and "claude code" (case-insensitive)
- `Co-authored-by: ...` referencing Claude or Anthropic

After removal, trailing blank lines and a dangling horizontal rule
(`---`, `***`, `___`) left behind by the trailer are also trimmed. If no
trailer is found, the PR is left completely unchanged (no edit is made).

## Server-side enforcement (optional)

The local middleware only helps people who install it. To strip the trailer no
matter who (or what) opens the PR, add a GitHub Actions workflow that reuses the
same `filter`:

```yaml
# .github/workflows/rm-generated-by.yml
name: Strip AI trailer from PR body
on:
  pull_request:
    types: [opened, edited]
permissions:
  pull-requests: write
jobs:
  strip:
    runs-on: ubuntu-latest
    steps:
      - env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          GH_REPO: ${{ github.repository }}
          PR: ${{ github.event.pull_request.number }}
        run: |
          gh extension install tomMoulard/gh-plugin-rm-generated-by
          gh plugin-rm-generated-by "$PR"
```

## License

MIT
