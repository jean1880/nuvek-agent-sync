---
name: polyglot-architect
description: "Multi-language scaffolding and standards: Rust, TypeScript/Vue, Python, shell, C#/.NET, C++ (start a new project, code standards, build setup, which gate and reviewer apply). For hands-on Rust use rust-craftsman."
---

# Polyglot Architect

> **Triggers & Scope:** Use for multi-language development standards, scaffolding and review loops across Rust, TypeScript/Vue, Python, shell, C#/.NET and C++: project structure, tooling, idioms, and which gate and reviewer apply. For hands-on Rust use `rust-craftsman`.

<!-- customise: set the default language per kind of project (tool, service, UI, glue) for your team. -->
LANGS: [Rust → `rust-craftsman` | TypeScript/Vue 3 + Tailwind v4 | Shell glue | Python → `references/python.md` | C#/.NET → `references/dotnet.md` | C++]

## CHOOSING A LANGUAGE
- A new reusable tool, service, CLI, hook or MCP server defaults to Rust.
- Shell is thin glue only: service wrappers, hooks, orchestration and globbing.
- A web UI is Vue 3 + Tailwind v4 on the project's shared tokens and components. A gap in the shared library is fixed in the library, not worked around in the app.
- Python only where a project already uses it. The standards are in `references/python.md`.

## PROJECT HYGIENE
- Detect the project from its manifest (`Cargo.toml`, `package.json`, `CMakeLists.txt`, `pyproject.toml`) and follow its local config (`rustfmt.toml`, eslint, ruff). Read its `AGENTS.md` (or a legacy `CLAUDE.md`) first.
- Search with `rg` and `fd`, and locate the project root before acting rather than assuming paths.
- Endpoints and secrets come from the environment at runtime, never compiled in or committed.
- A public repository never names a private deployment: no hosts, addresses, internal paths or personal identifiers.
- If you run a local coder model, delegate short, simple code challenges, single-function drafts and unit-test authoring to it to conserve cloud tokens and session context.

## REVIEW LOOP (not done without it)
- GATE, always:
  - Rust: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, or the repository's own gate script.
  - UI: the project's build, typecheck, lint and test scripts.
  - Shell: `shellcheck -x`.
  - Python: see the reference file.
  - C#/.NET: `dotnet build` then `dotnet test` from the repository root (details in `references/dotnet.md`).
  A red gate means not done.
- REVIEW, as a separate pass from writing: Rust → `rust-reviewer`; a shared UI library → `shared-library-review`; shell → `script-reviewer`. Reviewers never edit; the author fixes and re-reviews by messaging the same agent. For heterogeneous Maker-Checker review, invoke a harness other than the author's, rotating per the `delegation` skill. Say which review ran. An unstated review cannot be told apart from none.
- Scale it to the change: a logic change gets the full loop; a comment or doc-only edit gets the gate and a note.
