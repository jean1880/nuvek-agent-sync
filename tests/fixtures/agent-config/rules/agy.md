# ANTIGRAVITY OVERLAY (only AGY reads this, as ~/.gemini/GEMINI.md; shared rules arrive via ~/.gemini/AGENTS.md → rules/core.md)
- TIERS: `fast` = `flash` · `standard` = `pro` · `deep` = `pro`. `inherit` uses the parent model. Checker subagents for live systems use `pro`.
- ASK tool: `ask_question`. Re-review by sending the delta to the same subagent (`manage_subagents`), not a fresh `invoke_subagent`.
- TOOLS: there is no dedicated list/search tool. Directory listing and text search go through `run_command` (`rg`, `fdfind`, `ls`), one command per call.
- MCP-SCHEMAS: tool schemas live in `~/.gemini/antigravity-cli/mcp/<server>/*.json`; read the schema before calling `call_mcp_tool`.
- NO-CHAINING: `&&`, `||`, `;`, `|` and multi-line commands are rejected. Run one command per `run_command` call, put multi-step work in a reusable script, and poll a condition instead of `sleep ... ; check`. Example: not `cd X && git status`, but `git -C X status`.
- SUBAGENT-PERMISSIONS: headless reviewer subagents cannot answer permission prompts, so grant their tools before invoking them.
- REPO-VISIBILITY: new repositories default to private.
- EDIT-SAFETY: always `view_file` the exact lines before `replace_file_content`. Prefer `write_to_file` for whole small files (under 100 lines) to avoid indentation mismatches.
- PATH-SAFETY: verify a path exists (`fdfind` or `rg --files`) before `view_file`.
- MEMORY: Claude's native memory is read-only for you (see the memory-pointer rule). Put new durable lessons in your reply so the user or Claude can record them.
- SOURCE: skills, rules and agents come from the agent-config checkout; files under `~/.gemini/config/agents`, `mcp_config.json` and `hooks.json` are generated. Edit the repository, never the projection.
- CROSS-HARNESS: `cross-agent claude [-a <agent>] "<prompt>"` or `cross-agent codex "<prompt>"` (read-only, no personas) for adversarial review or second opinions, alternating between the two; exit 75 (rate-limited) → the other peer, then native AGY subagents. Detail is in the `delegation` skill.
<!-- customise: add AGY-only rules here (allowed browser domains, account names). -->
