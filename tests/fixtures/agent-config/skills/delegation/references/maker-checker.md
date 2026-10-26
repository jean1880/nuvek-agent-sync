# Maker-Checker across harnesses

Use a different model family for the check when available. Harness and model family are separate:
AGY can run Claude models. Three harnesses are in the rotation: Claude (`cross-agent claude -a <reviewer>`),
AGY (`cross-agent agy`) and Codex (`cross-agent codex`, read-only sandbox, no personas).

1. The maker drafts the change (plan, playbook, script, diff) and writes it to a file.
2. The checker runs on a harness other than the maker's, rotating between the two peers from one review to the next:

   | Maker | Checkers (alternate) |
   |---|---|
   | Claude | `cross-agent agy` · `cross-agent codex` |
   | AGY | `cross-agent claude -a <reviewer>` · `cross-agent codex` |
   | Codex | `cross-agent claude -a <reviewer>` · `cross-agent agy` |

   Security, schema and live-system changes go to both peers, independently (neither sees the other's verdict).
3. Write a review brief outside the project repository. Include task and review IDs, the repository or worktree, base and head commits, changed files, intent, previous findings and the current delta, exact checks with results, assumptions marked VERIFIED or UNVERIFIED, and the required verdict form (findings by severity plus APPROVED or CHANGES-REQUIRED). Pass it with `--prompt-file`; use `--task-id`, `--review-id` and `--json` to correlate the receipt. For Codex, paste the reviewer's lens, since it has no personas. Headless AGY cannot run shell, so give it exact file paths and a readable diff file; Codex can run read-only commands itself.
4. The checker is read-only: it returns findings and never edits.
5. The maker applies fixes, then sends the delta to the same supervised checker if continuation is supported. CLI calls are cold reviews on every harness: repeat the relevant findings and the delta under the same review ID, and keep the `cold_review` indication. Never claim context continuity from a reused persona name.
6. Exit 75 from a peer: hand the review to the other peer. If both are rate-limited, fall back to a native subagent of a deep-tier model with the same brief.
7. The coordinator decides. A checker's approval is evidence, not a deploy.
8. Before merging, validate the proposed merge tree against the final head and base pair. A retarget or a moved commit invalidates earlier evidence. Verify the required checks exist on that pair; a green source branch alone does not prove the merged result. Use the delivery tool's expected-head and base fences and exact verification argv.
