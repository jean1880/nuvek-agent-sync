---
name: delegation
description: "Fan work out to subagents, coordinate a team, deliberate before building, and delegate to another harness with cross-agent (parallel subagents, Maker-Checker review, rate limit exit 75, delegate this sweep)."
---

# Delegation

> **Triggers & Scope:** Use when fanning work out to subagents, choosing a model tier per agent, briefing or re-reviewing an agent, coordinating a team of agents, or handing work to another harness with `cross-agent`.

The rules PARALLEL-FIRST, SUBAGENTS, CROSS-HARNESS and TEAM-COORDINATION live in `rules/core.md`; this skill is the how-to. Two integrations are optional and named where they apply: `cross-agent` and Agent Terminal (https://github.com/jean1880/nuvek-agent-terminal) for cross-harness spawns, and `hive-board` (https://github.com/jean1880/nuvek-hive-board) for a shared board. Without them, native subagents still work, and every finding goes to an external plan file (kept outside project repositories) instead of the board.

## FAN-OUT
- Split along file or repository boundaries so two agents never edit the same file. Independent pieces run in parallel; a step that needs another's output runs after it.
- The coordinator keeps shared-library changes, cross-cutting decisions, commits and deploys, and integrates and verifies every result itself.
- Brief each agent so it stands alone: goal, files it owns, constraints, working directory, required tool capabilities, environment prerequisites, how to verify, and what to report. Keep full build gates serial per workspace; independent reads and focused tests can overlap.
- Pass the model explicitly on every spawn; never let an agent inherit the session model by accident. Say which model each got and why when reporting the fan-out.
- A standing request to fan out beats a harness default of "spawn only when asked". It does not justify a spawn for work one agent finishes in a few calls: each spawn starts cold and re-reads its context.
- Isolation: two or more agents EDITING the same repository at once each get their own git worktree (`git -C <repo> worktree add`, or the harness's worktree isolation), and the coordinator merges their branches. Read-only agents and single-writer fan-outs stay in the main checkout. Never put a worktree inside a tool's configuration directory, where it would be loaded as configuration.

## TEAM COORDINATION (optional: hive-board)
Subagents can work as a team through a shared board. `hive-board` provides one over MCP. When the board is unconfigured or unreachable, post the same typed findings (identity, kind, severity, subjects, evidence) to an external plan file instead, and treat that file as the board for everything below.
- **Identity.** Every agent states its identity (`agent_type`, `model`, `harness`) on each post. The orchestrator names them in every brief.
- **Topics and threads.** One topic per feature, incident or investigation. Agents read the brief and their peers' posts BEFORE posting.
- **Typed findings.** Post findings as they land, typed `finding`, `decision`, `gotcha`, `verdict` or `question`, with a severity (`BLOCKER`, `SHOULD`, `NICE`). Name real repositories or packages as subjects and give `file:line` or a command as evidence.
- **Statuses belong to the orchestrator.** It marks each acted-on finding `confirmed`, `fixed`, `false_positive` or `wontfix`. Authors never set their own findings' statuses.
- **Close the loop.** Resolve the topic with an outcome. Use a needs-human state when a person must act.
- **Resilience.** If the board is unreachable, the tool errors and nothing is written to it. Record the finding in the external plan file at once so it is not lost, and copy it to the board when it returns.

## DELIBERATION (non-trivial tasks)
How a team decides HOW before doing. Trivial tasks skip it.
1. **Open a topic** (kind `design`) whose description states the goal, the constraints, the chosen path and the PREMISES it rests on, listed as claims to verify, not facts. Every brief names the topic and the agent's identity.
2. **Round 1, independent.** Each agent proposes how to do its part and challenges the premises with evidence BEFORE reading peers; a later reader anchors on the first post. For designs, schemas and security-relevant work, include one peer-harness voice from the review rotation (see CROSS-AGENT).
3. **Round 2, cross-examination.** Continue with the same workers when their backend supports it; otherwise send a cold follow-up carrying prior findings, the delta and the same review ID. Each agent attacks the two weakest arguments of the others and concedes explicitly where beaten.
4. **Decide and record.** The orchestrator posts a `decision`: what was chosen, what was rejected and why, who moved. A split the team could not settle is decided by the orchestrator and named as such.
5. **Challenge the user once.** Disproved premises, missed details and better paths go to the user in ONE batched question round before building. The answer closes the question; new contrary evidence later is raised once.
6. **Build and verify.** For schemas, security and live-system designs, a fresh red team with no stake in any position attacks the decision before it ships.

Scale to the task: a two-agent check for medium work; all rounds plus a fresh red team for schemas, security and live systems. Cost is turns times context: resume agents for later rounds and keep posts typed and short.

### The panel
Four read-only seats argue over ideas. Each argues one lens deeply and hands outside concerns to the seat that owns it. Seat two or three per task, never all four by default. Define the seats as agents in `sync/agents/` (this repository ships none of them; write your own from the lenses below).

| Seat | Lens | Tier |
|---|---|---|
| architect | Shape: boundaries, ownership, contracts, coupling, blast radius; brings 2-3 options, one dropping a premise | deep |
| experience designer | The operator and the agents using it: journeys, tool descriptions, errors, defaults, UI | standard |
| senior engineer | Build it simply: reuse, smallest first slice, the proof, the release path, what to cut | standard |
| red team | Break it: trust boundaries, secrets, injection, failure modes, rollback, detection; the fresh-eyes final round | deep |

| Task | Seats | Plus |
|---|---|---|
| New service, schema or data model | architect + red team (+ senior engineer) | one peer |
| Feature in existing code, new crate or CLI | senior engineer + architect | |
| UI, CLI, tool, skill, alert or workflow | experience designer + senior engineer | |
| Security, auth, exposure, live systems | red team + architect | both peers |
| Refactor | senior engineer + architect | |

A peer voice runs a seat's brief on another harness, preferably a different model family: `cross-agent agy -a <seat>`, or `cross-agent codex "<seat lens + brief>"` (Codex has no personas, so paste the lens into the brief). A fresh red team, not one that argued in rounds 1 and 2, is step 6's red team.

## SWARM PLAYBOOK
For a build spanning several repositories or crates and several agents (after DELIBERATION):
1. **Contract first.** Write the plan with the shared contract (schema, routes, field names, payload types) BEFORE any builder starts. Run a plan reviewer on it, fold its findings into a binding addendum, and send each builder only the delta that touches its lane.
2. **One ledger.** Track each job: owner, the files it owns, dependencies, worktree, branch and head, review state, exact verification, next action. `hive-board` jobs are that ledger when you use them; otherwise use the plan. Do not mirror it into several documents. A job's `owns` list must not overlap with any other claimed job.
3. **Claim and fan out.** Brief one subagent per ready job. Match the agent type to the capability: read-only jobs go to reviewer or research agents, local-build jobs to builders. A live-system change is never a delegated job; it stays a dry-run plan the human approves.
4. **Keep context where supported.** A failed verification keeps the claim: send fixes to the same builder and reviewer when they support continuation. Cold CLI follow-ups carry prior findings and the delta under the same review ID.
5. **Verify with a real review.** The reviewer differs from the worker; for security-relevant jobs the harness differs too, and the model family where available. Say plainly when an exception applied. When parallel builders share an API, audit the server against every client.
6. **Restart recovery.** After a session restart, list the jobs you hold, reclaim any you can no longer complete, and claim them again.
7. **The coordinator owns shared surfaces.** Lockfiles, the git index, commits, tags, deploys and shared-library changes stay with the orchestrator. Commit with explicit pathspecs, never `commit -a`, when other work may be uncommitted in the tree.
8. **Close out.** Set finding statuses, resolve the topic with an outcome, and record harness quirks you met as `gotcha` findings.

Jobs earn their keep only when they pay for themselves. For work under about three calls, an in-session task list is enough.

Harness quirk: headless `agy` auto-denies shell commands, so give it explicit file paths and ask it to read only.

## TIERS
- `fast`: mechanical work, search, fan-out, status polls.
- `standard`: code review, research, multi-step builds.
- `deep`: hard reasoning only (incident root cause, architecture, security review, ambiguous debugging).
- Short, simple code challenges and unit tests can go to a local coder model when you run one.
- Custom agents keep their frontmatter tier unless the task warrants an override. The tier-to-model mapping is in `sync/map.toml` and each tool overlay.
- Checker agents for live-system changes get a deep-tier model.

## COST
- Cost is turns times context. Brief reviewers with the diff stat, the intent and what is already verified.
- Re-review by messaging the SAME worker with the delta when continuation is supported. A CLI invocation starts a cold context: carry the review ID, original findings, fixes, base and head commits and verification results forward, and do not call it a resume.
- Split a multi-crate or 1k+-line review into one reviewer per crate.
- Review plans with the `plan-reviewer` agent.
- Headless subagents cannot answer permission prompts: grant their tools first.

## UNATTENDED RUNS
Before the user leaves, list every step that will need approval or a classifier allowance (deploy, apply, rebuild, restart) and request scoped grants up front. Report each residual gated step as a hand-off script: numbered exact commands, expected output and rollback.

## CROSS-AGENT
`cross-agent agy|claude [-a <agent>] "<prompt>"` or `cross-agent codex [-m <model>] "<prompt>"` runs a task in a peer harness with a wall-clock timeout (`-t`). Install it from the `cross-agent` project; Agent Terminal (https://github.com/jean1880/nuvek-agent-terminal) offers the same bridge as `cross_agent_spawn`.
- **Pick before you spawn.** The goal is even load across every account, so the spawning agent's own harness is not the default. `cross-agent status --json` (or the Agent Terminal `cross_agent_capabilities` tool) lists each model with its `tags`, `tier` and `best_for`, plus each agent's `usage.headroom` and `usage.load`.
  1. Keep models whose tags and tier fit the task. Use `high-effort` models only where the work needs them.
  2. Drop agents whose headroom is `low` or `exhausted`. If it is `unknown`, refresh once; if it is still unknown, rank that agent after every agent with a reading.
  3. Pick the lowest `load`; on a tie, prefer a harness other than yours. `cross-agent select --tag review --tier standard --from codex --json` filters the cached catalogue and explains the choice, but does not refresh usage or check live tool grants.
  4. Set effort from the policy in `models.toml` (`[effort]`), clamped to what the model offers; `null` means the model has no effort control, so omit it. Reserved efforts (`xhigh`, `max`, `ultra`) are for the hardest work. Escalate effort, then model, only after a failed attempt.
  5. Spawn through the bridge. Use a native subagent only when the pick is your own harness, the work needs your live context or a persona only your harness has, or the bridge is down.
  6. Report agent, model, effort and the reason (tags, load).
  The tags are authored in `sync/models.toml` (see `sync/models.toml.example`).
- Use it when you are near session or rate limits, to offload high-token sweeps and keep the orchestrator lean, and for a heterogeneous second opinion.
- **Review rotation.** Rotate the checker across harnesses other than the maker's, and check the model family too: AGY can run Claude models, so a different harness alone does not establish model diversity. Security, schema and live-system reviews go to both peers independently when available; report an unavailable independent check explicitly.
- **Codex is a checker only.** `cross-agent codex` runs a read-only sandbox with no session files. It may run read-only shell commands (`git diff`, `rg`) to gather evidence, but it has no personas, so the brief carries the lens.
- Exit 75 means rate-limited: do not retry that peer. Hand the review to the other peer, else to a native deep-tier subagent.
- A timeout is a failure of that call, not a result: report it and narrow the prompt. Classify startup, permission, bridge, quota and timeout failures before choosing one fallback, and do not respawn a known-broken route or weaken permissions to make it run.
- Use `--prompt-file /absolute/brief.md` (or `--prompt-file -` for stdin) to avoid shell escaping. `--task-id`, `--review-id` and `--json` give a correlated receipt. A configured model or effort is not proof of what the provider used. Store the brief and result outside project repositories.
- Scrub secrets from the prompt: the peer is a separate context and sees only the prompt.
- Maker-Checker procedure: `references/maker-checker.md`.
