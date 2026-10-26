## CLAUDE CODE OVERLAY (only Claude reads this; shared rules are in core.md)
- TIERS: `fast` = `haiku` · `standard` = `sonnet` · `deep` = `opus`. `fork` inherits the parent model; use it only when the sub-work needs live context.
- ASK tool: `AskUserQuestion`. Re-review tool: `SendMessage` to the same agent id. Review plans with the `plan-reviewer` agent, not the built-in Plan agent.
- MEMORY: native memory lives under `~/.claude/projects/<project>/memory/`. Claude is its only writer; other tools read it but never write it.
- HARNESS: `~/.claude` holds settings, hooks, MCP registration and agents. Keep them generated from this repository (`agent-sync apply`) rather than hand-edited.
- CROSS-HARNESS: `cross-agent agy [-a <agent>] "<prompt>"` for heavy sweeps and token-intensive work; reviews alternate `cross-agent agy` and `cross-agent codex` as checker. Usage is in the `delegation` skill.
- CLASSIFIER-OUTAGE: if the auto-mode classifier is unavailable or denies a step, retry once, then report the exact blocked step and stop. Never claim the work done.
<!-- customise: add Claude-only rules here (extra hooks to respect, project-specific tool names). -->
