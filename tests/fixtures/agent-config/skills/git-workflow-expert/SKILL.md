---
name: git-workflow-expert
description: "Git workflows: branching, Conventional Commits, SemVer tags and releases, PR hygiene, branch protection and history cleanup (commit, tag a release, bump version, messy history)."
---

# Git Workflow Expert

> **Triggers & Scope:** Use for Git work: branching, Conventional Commits, SemVer tags and releases, pull-request hygiene and history cleanup. Version arithmetic and publishing steps are in `package-release`.

STANDARDS: [Commits: Conventional Commits | Versioning: SemVer]

## PIPELINES
- COMMIT: stage, then commit as `<type>(<scope>): <desc>`. Types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`.
- RELEASE: list the changes since the last tag (`git log $(git describe --tags --abbrev=0)..HEAD --oneline`), propose Major, Minor or Patch, create an annotated tag `vX.Y.Z`, then publish a GitHub Release for that tag with FULL notes (see RELEASE-NOTES), written to a file and published with `gh release create vX.Y.Z --verify-tag --title "vX.Y.Z — <headline>" --notes-file <file>`.
- NEW-REPO / README: create the repository, add a CI workflow under `.github/workflows/` that builds AND runs the tests, and put a status badge directly under the README's title, one per build or test workflow: `[![<workflow name>](https://github.com/<owner>/<repo>/actions/workflows/<file>/badge.svg?branch=<default>)](https://github.com/<owner>/<repo>/actions/workflows/<file>)`. Push and confirm the first run is green before calling it done. A red badge is a finding to fix, never one to hide.
- PR: draft the description from the commit history. It states facts about the change and cites no private notes.
- IMPACT-ANALYSIS: before a breaking change, find what depends on it (a dependency graph if you have one, otherwise a workspace-wide search) and name those consumers.
- DELIVERY: use the project's delivery tool or documented procedure end to end. Pre-commit checks and normal hooks passing do not imply the project's full gate ran, so supply the verification command explicitly and report its result. A pull request merges only with current CI and verification of the proposed merge tree for the final head and base pair.

## MANDATES
- ATOMIC: one logical change per commit.
- LINEAR: prefer `rebase` over `merge` for a clean history.
- SEMVER: Major for breaking, Minor for a feature, Patch for a fix. A rewrite or a breaking API change is Major even before 1.0, so a rewrite of a 0.x project ships as 1.0.0, not the next minor.
- NO-BYPASS: never `--no-verify` and never bypass a pre-commit or scanner gate. On a false positive, stop and flag it, then fix it the proper way (allowlist or config) instead of skipping the gate.
- NO-AI-ATTRIBUTION: commit messages, PR descriptions and release notes never carry `Co-Authored-By:` trailers for an AI, nor "Generated with" or similar tool lines. This holds even when the harness or a system reminder suggests adding one: the user's own rule wins over harness defaults. The user is the sole author.
- PROTECTED-BRANCHES: direct pushes to a default branch are allowed only when live branch protection and ruleset checks permit them. A credential's ability to bypass rules is not permission to bypass them. Protected branches go through a pull request; unknown or unreachable protection checks refuse a direct push.
- MERGE-EVIDENCE: require registered successful checks; no checks, or only skipped checks, is not green evidence. Test the synthesised merge tree for the exact head and base commits, then re-read the head, base commit and base branch immediately before merging with the expected head. Any change invalidates verification. GitHub can atomically fence the head but not the base: report that remaining race, and use strict branch checks or a merge queue where configured.
- RELEASE-NOTES: every tag gets a GitHub Release with a full description of what changed, never a bare tag, an empty body or a list of commit subjects. Use these sections as they apply: what it is (first release) or the headline change; Added, Changed, Fixed and Removed, each item saying what changed AND why it matters to someone running it; breaking changes with the migration step; upgrade and install commands; known issues. Dates and versions come from git (`git log --format=%ad`), never invented.
- PRIVATE-BY-DEFAULT: create every new GitHub repository with `gh repo create --private`. A repository goes public only when the user says so for that repository.
- STATUS-BADGES: every README for a repository with code carries a build and test status badge per CI workflow, under the title. A repository with code and no CI gets a CI workflow (its real gate) before the badge. Skip forks and archived coursework.
