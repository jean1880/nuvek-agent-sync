# Overlays

Keep the shared part of your source public and the private part (hosts, addresses, personal
rules, private skills) in a second checkout. Point agent-sync at both:

```bash
agent-sync --dir ~/git/nuvek-agent-config --overlay ~/git/my-private-config apply
```

or set `AGENT_CONFIG_DIR` and `AGENT_SYNC_OVERLAY` once. The overlay is a flag rather than a
setting in the base, because where it lives is a fact about one machine, and a public base should
never name a private path.

## How the two are combined

agent-sync builds a resolved source tree at `$XDG_STATE_HOME/agent-sync/resolved` and points every
link into it. Only `.claude-plugin/`, `rules/`, `skills/` and `sync/` are resolved.

- **Files:** for each relative path, the overlay's file wins over the base's. The tree holds
  symlinks to the winning files, so `@` imports, plugin discovery and skill links all reach the
  real file, and a generated agent's header names the file to edit (`~/…/sync/agents/<name>.md`
  in whichever layer it came from).
- **Skills:** `skills/<name>/` is one unit: an overlay skill replaces the base skill's whole
  directory, and an overlay-only skill is added.
- **Removing base files:** list them in `overlay.toml` at the overlay's root:

  ```toml
  remove = ["skills/package-release", "sync/agents/plan-reviewer.md"]
  ```

- **TOML sources:** when the overlay has one of the files below, it is merged with the base's copy
  by key. When only the base has it, it is used as it is. `[remove]` in the overlay's copy deletes
  base entries; naming an entry the base does not have does nothing.

  | File | Merge rule | Delete with |
  |---|---|---|
  | `mcp.toml` | `[servers.<name>]` replaces the base entry whole | `[remove] servers = ["name"]` |
  | `hooks.toml` | `[[hook]]` keyed by `name`: replaced in place, new hooks appended | `[remove] hooks = ["name"]` |
  | `permissions.toml` | each slot list is unioned (base order, then new overlay entries) | `[remove.allow]` / `[remove.deny]` with slot lists of exact entries |
  | `models.toml` | `[vocabulary]` by key, `[[model]]` by `match` + `driver`, `[effort]` replaced | `[remove] vocabulary = [...]`, `models = ["<match>"]` |
  | `map.toml` | tables merge, arrays union (an overlay adds `placeholders`), values replace | not supported: override the value |

  ```toml
  # overlay sync/mcp.toml
  [remove]
  servers = ["filesystem"]

  [servers.my-private-api]
  url = "http://${API_HOST}:8080/mcp"
  agents = ["claude", "agy", "codex"]
  ```

## check and apply

`check` resolves into a temporary directory and compares the installed tree with it, so it stays
read-only and reports a stale tree as drift.

`apply` builds a new generation of the tree under `generations/`. It then loads and renders every
projection from that candidate, and only if all of that succeeds does it swap the candidate in
with one atomic rename. A candidate that fails to build or render is discarded, and the live tree
stays as it was. The live generation and the one before it are kept. Only directories that
agent-sync marked as its own generations are ever removed.

Links to items that left both layers are pruned. So are links left behind when you rename the
plugin or stop using an overlay, provided `managed.json` lists them.

`import` and `promote --add` write into the source, so they refuse to run with an overlay (exit
2). Edit the overlay's `sync/` files instead.

Without `--overlay` the resolver never runs and every link points at the source checkout.
