<p align="center">
  <img src="assets/nuvek-agent-sync.svg" alt="agent-sync icon" width="128" height="128">
</p>

# nuvek-agent-sync

[![CI](https://github.com/jean1880/nuvek-agent-sync/actions/workflows/ci.yml/badge.svg?branch=master)](https://github.com/jean1880/nuvek-agent-sync/actions/workflows/ci.yml)
[![Licence: MIT](https://img.shields.io/badge/licence-MIT-blue.svg)](LICENSE)

`agent-sync` keeps the coding agents you use aligned to one authored source. You write your rules,
skills, agents, MCP servers, permissions and hooks once, in an agent-config checkout, and
`agent-sync` renders them into each tool's own configuration: Claude Code, Antigravity (`agy`) and
Codex. Start your source from the template at
[nuvek-agent-config](https://github.com/jean1880/nuvek-agent-config).

## Highlights

- **One source, three tools.** Rules, skills, agents, MCP servers, permissions, hooks and model tags.
- **`check` gates drift.** Read-only; exits 6 when any tool differs from the source.
- **Private overlays.** Layer a private checkout over a public base ([docs](docs/overlays.md)).
- **Careful, scrubbed writes.** Atomic, backed up, confined to the tool directories, secret-scanned, and everything printed or logged is redacted.
- **Merges, never clobbers.** Files a tool co-owns are merged into its existing config, preserving tool-owned settings.

## Requirements

Linux. Claude Code (`claude`) must be installed for `apply` to register its MCP servers.

## Install

From a release (static x86_64 Linux binary, or a `.deb` that installs `/usr/bin/agent-sync`):

```bash
curl -LO https://github.com/jean1880/nuvek-agent-sync/releases/latest/download/SHA256SUMS
# download the binary or .deb listed there, then:
sha256sum --check --ignore-missing SHA256SUMS
```

From source (Rust 1.90 or newer):

```bash
cargo install --locked --git https://github.com/jean1880/nuvek-agent-sync
```

## Quick start

```bash
git clone https://github.com/jean1880/nuvek-agent-config.git ~/git/agent-config
agent-sync check     # exit 6: nothing is rendered yet on a fresh machine
agent-sync apply     # write every projection
agent-sync check     # exit 0
```

Start a new session in each tool to load the result. After that, edit the source, never the
projections, and run `agent-sync apply` again.

## What it renders where

| Source (in the checkout) | Claude Code | Antigravity | Codex |
|---|---|---|---|
| `rules/core.md` + `rules/<tool>.md` | `~/.claude/CLAUDE.md` (two `@` imports) | `~/.gemini/AGENTS.md`, `~/.gemini/GEMINI.md` (links) | `~/.codex/AGENTS.md` (generated) |
| `rules/memory-pointer.md` | | `~/.gemini/config/rules/memory-pointer.md` (link) | |
| `.claude-plugin/plugin.json`, `skills/` | `~/.claude/plugins/<name>`, `~/.claude/skills/<name>` (links) | `~/.gemini/config/skills` (link) | `~/.codex/skills/<skill>` (links) |
| `sync/agents/*.md` | `~/.claude/agents/*.md` | `~/.gemini/config/agents/*.md` | |
| `sync/commands/` | `~/.claude/commands` (link) | | |
| `sync/mcp.toml` | `~/.claude.json` (through `claude mcp`) | `~/.gemini/config/mcp_config.json` | `[mcp_servers]` in `~/.codex/config.toml` |
| `sync/permissions.toml` | `permissions` in `~/.claude/settings.json` | `permissions` in `~/.gemini/antigravity-cli/settings.json` | |
| `sync/hooks.toml` | `hooks` in `~/.claude/settings.json` | `~/.gemini/config/hooks.json` | `~/.codex/hooks.json` |
| `sync/models.toml` (optional) | | | `$XDG_CONFIG_HOME/agent-terminal/model-tags.json` |
| A repository `~/git/<repo>` with a git-ignored `.claude/AGENTS.md` | | `<repo>/.agents/AGENTS.md` (a link, kept out of git through `.git/info/exclude`) | |

`<name>` is the `name` in your checkout's `.claude-plugin/plugin.json`. `sync/map.toml` is the
translation table: model tiers, tool names, and the placeholder names that may be expanded. Files
that a tool co-owns are merged, not replaced.

## Commands

| Command | What it does |
|---|---|
| `check [--verbose]` | Read-only. Compares every projection with the source; prints what is out of sync. |
| `apply [--force] [--prune]` | Writes every projection, then re-checks. `--force` writes Antigravity's settings even while `agy` runs. `--prune` also unregisters MCP servers that are not in `mcp.toml`. |
| `status` | Every surface and its state, including the ones in sync. |
| `promote [--list \| --add <entry>… \| --drop <entry>…]` | Reviews approvals that a tool appended on its own: moves them into `permissions.toml`, or drops them. |
| `import [--force]` | Seeds `sync/` from an existing Claude Code configuration. |
| `validate` | Checks every agent's front matter for each tool. |
| `selftest [--guard <script>] [--probe-cli] [--deep]` | End-to-end probes of the rendered agents, links, MCP files and CLIs. |
| `session last \| tail \| status` | Reads recent Claude Code or Antigravity transcripts, with secrets redacted. |

Exit codes:

| Code | Meaning |
|---|---|
| 0 | In sync (or the command succeeded) |
| 2 | Usage error, or a writing command refused with an overlay |
| 4 | The source's `sync/` directory, or the overlay checkout, was not found |
| 5 | Refused: `agy` is running and `--force` was not given |
| 6 | Out of sync (`check`), an `apply` that could not converge, or another error |
| 7 | The secret-surface configuration could not be loaded |

## Configuration

| Flag / variable | Default | What it sets |
|---|---|---|
| `--dir` / `$AGENT_CONFIG_DIR` | `~/git/agent-config` | The source checkout (the base, when an overlay is set) |
| `--overlay` / `$AGENT_SYNC_OVERLAY` | unset | A private overlay checkout layered over the source |
| `$XDG_STATE_HOME` | `~/.local/state` | State: `agent-sync/lock`, `agent-sync/managed.json`, and with an overlay `agent-sync/resolved` |
| `$AGENT_SYNC_ENV_FILE` | unset | A shell-style env file read for placeholder values not in the environment |
| `$AGENT_SYNC_CLAUDE_BIN` | `claude` | The Claude Code CLI used to register MCP servers |
| `--guard` / `$AGENT_SYNC_GUARD` | unset | The pre-tool guard hook `selftest` probes |
| `$XDG_CONFIG_HOME` | `~/.config` | Where `model-tags.json` goes |
| `$NUVEK_SECRET_SURFACES_FILE` | `$XDG_CONFIG_HOME/nuvek/secret-surfaces.toml` | Extra secret surfaces; a set but unusable file stops the run with exit 7 |
| `$NUVEK_LOG_SINK` | `auto` | `stderr` keeps the audit log off the systemd journal |

Every home-relative path comes from `$HOME`. A `${NAME}` in `mcp.toml` is expanded only when `NAME`
is listed in `map.toml` `placeholders`; every other `${...}` is written verbatim, so a secret
reference is never inlined. Every change `apply` makes is logged to the journal as `agent-sync`
(`journalctl --user -t agent-sync`) and to stderr.

More: [overlays](docs/overlays.md) and [operations](docs/operations.md) (how writes work, paging on
drift from systemd, recovery).

## Security

Writes are atomic, confined to the managed tool directories, never pass through a symlink, and
refuse secret-shaped content. Only what `managed.json` records is ever deleted. Output is scrubbed.
Vulnerabilities go through [SECURITY.md](SECURITY.md).

## Troubleshooting

- **`check` exits 6 on a fresh machine:** a tool file is `missing`. Run `agent-sync apply`.
- **Exit 5:** `agy` is running. Close it, or pass `--force`.
- **Exit 7:** the file named by `NUVEK_SECRET_SURFACES_FILE` is missing or invalid.
- **A destination is a symlink:** `apply` refuses to write through links; see
  [operations](docs/operations.md).

## Related projects

- [nuvek-agent-config](https://github.com/jean1880/nuvek-agent-config): the template source that `agent-sync` renders
- [nuvek-core](https://github.com/jean1880/nuvek-core): the library behind its secret scrubbing and logging
- [nuvek-cross-agent](https://github.com/jean1880/nuvek-cross-agent) and [nuvek-agent-terminal](https://github.com/jean1880/nuvek-agent-terminal): cross-harness delegation; `models.toml` feeds their model tags
- [nuvek-hive-board](https://github.com/jean1880/nuvek-hive-board): a board for coordinating agent teams

## Support

If agent-sync saves you time, you can support its development:

- [Ko-fi](https://ko-fi.com/jean1880)

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Security reports go through [SECURITY.md](SECURITY.md).

## Licence

Released under the [MIT Licence](LICENSE). Copyright (c) 2026 Jean-Luc Desroches.
