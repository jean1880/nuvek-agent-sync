# Python: where it lives, and the bar it meets

Python is not the default for new reusable tools; those are Rust crates (`rust-craftsman`). It survives
where a project already chose it. Edit those projects in their own idiom, and do not port them as a
side effect of another change.

## Standards
- **Match the neighbours.** Read one or two sibling modules first, and mirror their imports, logging,
  naming, error idiom and density.
- **Types:** full hints; `X | None`; `pathlib`; `dataclass` or `TypedDict` for records. `Any` only at the
  dynamic edge (`json.loads`, untyped libraries), commented, and pushed into a typed model straight away.
  For a genuinely unknown value, use `object` with `isinstance` narrowing.
- **Trust boundaries** (configs, API payloads, LLM output): validate at the boundary with whatever the
  project already uses (pydantic where it is a dependency). Fail closed: a failed validation or scan
  excludes the input, never includes it.
- **Errors:** narrow `except`s. Log why and the offending input. Fatal means a non-zero exit, so the
  service manager marks the unit failed. No silent `pass`.
- **Logging:** the project's logger, not `print`. Lazy arguments in log calls. Any appended file is size-capped.
- **Long jobs:** bounded batches, incremental state saves, atomic writes (`tmp` + `os.replace`). Re-running
  is safe (hash guards, upserts, orphan cleanup with an empty-set guard).
- **Secrets:** from `os.environ`, never hard-coded. Scrub text before it leaves the process.
- **Docs:** module and function docstrings in the present tense. Say what it does and why, not its history.

## Gate
`python3 -m py_compile <files>`, then `ruff` and `mypy --strict` where the project configures them, then
run it or its tests. Paste the evidence. Review against the list above before calling it done.
