---
name: rust-craftsman
description: "Author and refactor Rust: crates, CLI tools, MCP servers, async services, and the fmt, clippy and test gate (write a crate, add a CLI or MCP tool, refactor Rust, fix a Rust bug). Review is the rust-reviewer agent; multi-language scaffolding is polyglot-architect."
---

# Rust Craftsman

> **Triggers & Scope:** Use when writing or substantially editing Rust so it lands idiomatic, gated and installed the first time. Review is the `rust-reviewer` agent; multi-language scaffolding is `polyglot-architect`.

<!-- customise: add a CONTEXT line naming your workspace layout, your gate script and the shared crates people should reuse. -->

If the repository has its own `AGENTS.md`, it adds the local specifics (which crate to reuse for what, the layering rule, per-binary configuration). This skill is the craft that applies in every repository.

## BEFORE WRITING
- **Reuse before writing.** Check the workspace's shared crates and the dependencies already declared before adding a helper or a dependency. A private helper that fits should be exposed, not rewritten.
- **Blast radius of a shared crate.** Before changing a public item in a library others consume, list its consumers (a code graph if you have one, otherwise a workspace-wide search). Removing or renaming an export breaks consumers that only rebuild later.
- **Match the neighbours:** error type, logging macro, module layout, test style.

## WRITING
- **Errors:** `Result` with `thiserror` in libraries and `anyhow` in binaries. No `unwrap`, `expect` or `panic!` outside tests. A security check that cannot evaluate returns "unsafe", never "safe".
- **Strings are char-safe:** truncate with `.chars().take(n)`, never a byte slice. A byte index panics on a multi-byte boundary. Guard every truncation with a multi-byte test.
- **Async:** tokio. Every external call gets its own `.timeout(...)`. Blocking work (SSH, `std::thread::sleep`, synchronous file walks) runs outside the runtime or in `spawn_blocking`, never on a worker.
- **Config:** endpoint and secret NAMES in code, values from the environment at runtime (treat a blank value as unset). Nothing host-specific is compiled into a library, or into anything headed for a public repository.
- **Output channels:** an MCP server's stdout carries only JSON-RPC frames, so logs go to stderr. A context-enrichment hook (session or prompt context) fails open: it exits 0 on every error, because a blocking exit code can erase the user's prompt, and it prints nothing on its skip path. A security or approval guard keeps its blocking exit code, since blocking is its job.
- **Logs:** send them through the project's logging layer (structured, to the system journal or stderr). Never write a log file under an agent's configuration directory; that directory is harness config, not a log location.
- **A corner cut on purpose** (global lock, O(n²) scan, heuristic) gets a comment naming its ceiling and the upgrade path.
- Every non-trivial change leaves one test behind: the smallest one that fails if the logic breaks. Real captured fixtures beat hand-written ones. If you run a local coder model, hand it the isolated unit tests and simple scaffolding to conserve cloud context.

## DONE MEANS
1. Gate green: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` in the crate, or the repository's own gate script when it has one. Fix a red gate before anything else.
2. `rust-reviewer` returns PASS. Re-review by messaging the same agent with the delta, not a fresh spawn. For core crates or complex logic, add a cross-harness check on a harness other than the author's, rotating per the `delegation` skill (`cross-agent claude -a rust-reviewer`, `cross-agent agy`, `cross-agent codex`), to remove single-model confirmation bias.
3. **Installed:** the gate builds debug, but timers, hooks and MCP servers often run the release binary. Finish with `cargo build --release -p <crate>` where that applies. An MCP server then needs a reconnect and a timer picks the binary up on its next run. A stale release binary is a live bug: it can keep running old behaviour that the new code deliberately removed.
4. Verified live where the tool has live behaviour: a dry-run mode, a handshake or a smoke test against a throwaway environment.
