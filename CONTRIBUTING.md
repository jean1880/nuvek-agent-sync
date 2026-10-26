# Contributing to nuvek-agent-sync

Thanks for taking the time to help. Bug reports, ideas, documentation fixes and code are all
welcome. This guide says how to get a change from an idea to a merged pull request.

Everyone taking part is expected to follow the [Code of Conduct](CODE_OF_CONDUCT.md).

**Security problems are not reported in issues.** Follow [SECURITY.md](SECURITY.md) and use
GitHub's private vulnerability reporting.

## Reporting a bug

[Open a bug report](https://github.com/jean1880/nuvek-agent-sync/issues/new?template=bug_report.yml).
Search the [open and closed issues](https://github.com/jean1880/nuvek-agent-sync/issues?q=is%3Aissue)
first, and try `master`, where fixes land before a release.

**Review everything you paste for secrets.** `check` output, `settings.json` and `mcp.toml` can
hold tokens, hostnames and home paths. Replace anything sensitive with `****` before you post.

## Before you write code

**Open an issue first for anything bigger than a small fix**: a new projection, a new source
format, a change of behaviour or a new dependency. Typo fixes, small documentation fixes and
obvious bug fixes can go straight to a pull request.

The rules that most often decide a review:

- **agent-sync names no deployment.** A host, a path or a person that differs between machines
  is a flag, an environment variable or a source file, never a constant.
- **`check` is read-only.** It never writes a tool file, and it reports a missing file as drift.
- **Every write goes through `src/fsx.rs`.** It allows writes only under the managed roots, uses
  exclusive temporaries and a rename, keeps the previous copy as `.agent-sync.bak`, and never
  writes through a symlink. Content goes through `sync::atomic_write`, which also refuses
  secret-shaped content. Deletion needs an entry in `managed.json`.
- **Everything printed goes through `say!` / `say_err!`** (`src/out.rs`), which scrub secrets.
- **No overlay means no change.** Output without an overlay is pinned by a golden test
  (`tests/golden/no-overlay.txt`). Regenerate it with `UPDATE_GOLDEN=1 cargo test` only for an
  intended change, and say why in the pull request.
- **No `unwrap`, `expect` or `panic!` outside tests.**
- **Tests never touch real `$HOME` state.** Use a temporary HOME, as `tests/cli.rs` does.

## Setting up

You need Rust 1.90 or newer.

```bash
git clone https://github.com/jean1880/nuvek-agent-sync.git
cd nuvek-agent-sync
git config core.hooksPath .githooks   # gitleaks and rustfmt before each commit
```

The pre-commit hook needs [gitleaks](https://github.com/gitleaks/gitleaks) on your `PATH`.

## Testing

What CI runs, and what to run before you push:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo deny check licenses bans sources
```

- **A bug fix** comes with a test that fails without the fix.
- **New logic** comes with unit tests beside it.
- **A new projection** comes with a `check` → `apply` → `check` test under a temporary HOME.

## Commits

Commits follow [Conventional Commits](https://www.conventionalcommits.org/):

```text
fix(sync): report a missing hooks.json as drift
feat(render): project hook matchers to Codex
```

- **Types**: `feat`, `fix`, `docs`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`, `style`.
- **Subject**: says what is now true, in lower case, without a full stop.
- **Sign off every commit** (`git commit -s`). The `Signed-off-by:` line certifies the
  [Developer Certificate of Origin](https://developercertificate.org/): that you wrote the change
  or otherwise have the right to submit it under the project's licence. Pull requests with
  unsigned commits are not merged.

Write prose in Canadian English (colour, behaviour, centre, licence as a noun). Code identifiers
keep their own spelling.

## Pull requests

1. Run the checks above and make sure they pass.
2. Open the pull request against `master`. Its title follows the Conventional Commits format.
3. Link the issue with `Fixes #123`, so it closes on merge.

Releases are cut by the maintainer. Leave the version in `Cargo.toml` alone.

## AI-assisted contributions

Using AI tools in a contribution is fine. The responsibility for it stays with you: say so in the
pull request, understand every line, and run the checks yourself.

## Licence

nuvek-agent-sync is released under the [MIT Licence](LICENSE). By contributing, you agree that
your contribution is licensed under the same terms, and you certify the Developer Certificate of
Origin with your `Signed-off-by:` line.
