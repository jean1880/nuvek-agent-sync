---
trigger: model_decision
description: "Read before infrastructure, harness or project work: durable lessons, gotchas and user feedback live in Claude's native memory (a MEMORY.md index plus one file per fact)."
---
# Native memory (read-only for AGY)

Durable, hard-won knowledge lives in Claude Code's native memory:

- Index: `~/.claude/projects/<project>/memory/MEMORY.md`, one line per memory.
<!-- customise: replace <project> with your project directory name under ~/.claude/projects/. -->
- Each entry links a file in the same directory holding one fact (`feedback`, `project`, `reference`, `user`).

How to use it:
1. Before infrastructure, harness or project work, `view_file` the index and then the few entries relevant to the task.
2. Treat entries as background that was true when written. Verify any named file, flag or address against live state before relying on it.
3. Never edit these files. Claude is the single writer; if you learn something durable, say so in your reply.
