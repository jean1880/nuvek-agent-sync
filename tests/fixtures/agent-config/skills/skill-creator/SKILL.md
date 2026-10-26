---
name: skill-creator
description: "Author and refactor skills: SKILL.md files in this repository's skills directory, shared by Claude Code, Antigravity and Codex (create a skill, edit the X skill, split a skill that got too big, add a capability)."
---

# Skill Creator

> **Triggers & Scope:** Use when authoring or editing a skill. It writes and refactors `SKILL.md` files in this repository's `skills/` directory, shared by Claude Code, Antigravity and Codex.

A skill encodes an opinion, a gotcha or a workflow that is specific to YOUR environment and that the agent
would otherwise get wrong. It is a guide to be consulted, not a rulebook to be obeyed. Write for a capable
reader who needs the local facts, not for one who needs to be fenced in.

## THE THREE TIERS
Context loads in stages. Put each fact in the cheapest tier that still surfaces it in time.
1. **`description` (always in context).** The only thing that decides WHEN the skill loads. It is the highest-leverage field, so write it first.
2. **SKILL.md body (loads on trigger).** What is needed on essentially every invocation of the skill.
3. **`references/*.md` (loads on demand).** Deep procedures, long tables, recovery runbooks, per-branch detail. Link them from the body by path.

## AUTHORING
- DESCRIPTION-FIRST: `description` says WHAT the skill does and WHEN, including the literal phrases the user would type (in parentheses) and the scope boundary. At most 1024 characters, no angle brackets (`<` `>`). A vague description ("helps with X") under-triggers. Where two skills overlap, name the sibling in the description ("release tagging goes to package-release"); delegation declared there beats a routing table elsewhere.
- `name` is the folder name: kebab-case, no spaces, capitals or underscores.
- CONTEXT HEADER: group the stable facts a reader needs immediately (platform, versions, hosts, auth path, key paths) near the top.
- SIGNAL: spend the body on what is non-obvious HERE: failure modes, constraints, the trap that looks like it should work. Cut anything derivable from reading the filesystem, the config or general knowledge.
- JUDGEMENT-OVER-RULES: state the situation and the consequence and let the reader decide. "Editing a single-file bind mount with `sed -i` orphans the inode, so the container sees `Stale file handle`" beats a bare "NEVER use sed". Reserve hard MUST and NEVER for the genuinely dangerous: destructive operations, secrets, live systems, irreversible state.
- NO-DUPLICATION: do not restate the global mandates from `rules/core.md`, and do not restate one rule twice inside a skill under two names. If a global mandate has a skill-specific sharpening, write only the sharpening and note that the global still applies.
- ONE SKILL, ONE JOB: a skill that covers two unrelated jobs becomes two skills. Overlap is resolved in the descriptions and in the routing section of `rules/core.md`.
- HOW-NOT-WHAT: a skill teaches HOW to do something: procedures, mechanisms, failure modes. It never stores facts that change on their own (inventories, identifier tables, label values, addresses and ports, versions, file lists of a config directory). Those live where they are kept current: a queryable index or graph for live topology, the owning repository file for configuration. The skill names the query or the owning file and how to change it safely; it never copies the result.
- PROSE IS FINE: complete sentences over telegraphic notation. Compression that costs clarity is a bad trade. Structure with plain headings and bullets; use an arrow chain only when the ORDER is the point.
- ASSETS: give scripts, configs and reference docs by path, relative to the skill folder or absolute.

## DELEGATING SKILL WRITING
A subagent that writes or rewrites a `SKILL.md` starts cold and has not read this file. Its brief must say
"follow `skills/skill-creator/SKILL.md`" and name the rules that matter for the task (HOW-NOT-WHAT,
PRESENT-STATE, TOOL-REFS, PORTABILITY). Without that, the first draft tends to be a fact inventory or a
changelog that has to be rewritten. The same brief names the files the agent owns, so two agents never edit one skill.

## PROGRESSIVE DISCLOSURE
- Split when the body exceeds about 1500 words, or as soon as a section is relevant to only one branch of the work, whichever comes first.
- Move it to `references/<topic>.md` and leave one line in the body saying what the file covers and when to open it.
- Good candidates: recovery runbooks, forensics procedures, long reference tables, worked templates, elicitation prompt banks.
- Exemplar: `delegation` keeps the rules in the body and the Maker-Checker procedure in `references/maker-checker.md`.

## PRESENT-STATE
- Document the NOW. No history and no changelog: no dates, and no "retired", "deprecated", "was", "used to", "now changed" or "superseded". State the rule, the failure mode and the fix, not when or how it changed.
- NO-GHOSTS: something that no longer exists gets no mention at all, not even a warning. Delete "never re-add X" and "not the old Y". Keep a prohibition only when the wrong path is still reachable with something that currently exists. Keep the rule and the live failure mode from a past incident; cut the incident.

## TOOL-REFS
Reference a CLI exactly as your permission allowlist grants it. Permission matchers compare the command
string literally and never expand `~`, so a near miss prompts on every call.
- Use the bare command name for tools on `PATH`.
- Use an absolute path only for scripts that are not on `PATH`, and invoke them directly rather than through `bash <path>`, which is a different string.
- Never reference build output directories such as `target/release/` from a skill or agent.

## PORTABILITY (every skill is loaded by Claude Code, Antigravity and Codex)
- STRICT-YAML: frontmatter must parse as strict YAML. Quote any `description` containing `: `. Claude tolerates the slip, but AGY rejects the whole skill and it silently disappears there. `agent-sync check` reports it as drift on the named skill.
- NEUTRAL-WORDING: name MCP tools as server → tool (`hive-board` → `board_post_message`), not `mcp__hive-board__board_post_message`. Describe actions as goals ("read the file", "find files matching", "ask the user") rather than one tool's function names. AGY has no dedicated list or search tool, so a skill must not depend on Grep or Glob existing.
- TOOL-SPECIFIC: a passage that only makes sense in one tool goes inside `<!-- only:claude -->` … `<!-- /only -->` (or `only:agy`). A whole skill that is tool-specific declares `agents: [claude]` in its frontmatter.
- PRIVATE-FACTS: nothing that names your hosts, addresses, domains, usernames or paths belongs in a skill that others will read. Use a placeholder or a `<!-- customise: ... -->` marker.

## DEPLOY
- Path: `skills/<name>/SKILL.md` in this repository is the only authored copy.
- Claude reaches it through the plugin link that `agent-sync apply` creates under `~/.claude`, AGY through `~/.gemini/config/skills`, and Codex through per-skill links under `~/.codex/skills`.
- Commit skill edits to this repository, then run `agent-sync check` (it must exit 0) before pushing.
