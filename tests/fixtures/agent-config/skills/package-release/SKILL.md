---
name: package-release
description: "Cut and publish package releases: SemVer rules (major, minor, fix or patch), version-bump automation, release notes, git tags and CI verification (cut a release, bump version, publish package, tag release)."
---

# Package Releases

> **Triggers & Scope:** Use when cutting, bumping or publishing a release for a package or application. It governs version arithmetic, bump automation, release-notes authoring and CI verification. Git mechanics (commits, tags, GitHub Releases) are in `git-workflow-expert`.

<!-- customise: name your package pipeline (for example a package-repository builder) and where release notes live. -->

Releases are pipeline-driven: packages build in clean CI environments from git tags. A manual install on a daily-driver machine (`make install`, an unvetted package file) shadows packaged binaries in `$PATH` and causes drift between the machine and the repository, so ship through the pipeline.

## SEMANTIC VERSIONING (SemVer)
Versions are `MAJOR.MINOR.PATCH` with strict monotonic progression.
1. **MAJOR (`X.0.0`)**: incompatible API or CLI breaks, architectural rewrites, removal of capabilities, breaking configuration schemas. Resets MINOR and PATCH to 0, and the release notes carry explicit migration instructions.
2. **MINOR (`x.Y.0`)**: backwards-compatible new functionality, new tools, UI panels, CLI arguments. Resets PATCH to 0.
3. **PATCH (`x.y.Z`)**: backwards-compatible bug fixes, performance tuning, leak fixes, styling and packaging fixes. Increments PATCH only.

### Package-repository monotonicity
A package repository compares versions with the package manager's own ordering (for Debian, `dpkg --compare-versions`). A new version must be strictly greater than every version already published in the target suite.
- Pre-release or beta builds from a feature branch (for example `1:3.0.2~beta...`) are rejected once stable has advanced past their base version. Advance the base version in the manifest past the latest published version across ALL suites before triggering a build.

## BUMP AUTOMATION
Release-enabled repositories should provide one `make bump-version <option>` interface so nobody hand-edits version files:
- `make bump-version patch` (or `fix`, or no argument) increments PATCH.
- `make bump-version minor` increments MINOR and resets PATCH.
- `make bump-version major` increments MAJOR and resets the others.
- `make bump-version <X.Y.Z>` sets an explicit version.
- `make release <option>` runs the bump, stages release artifacts, makes the Conventional Commit and creates the annotated tag.

The bump updates the primary manifest (`Cargo.toml`, `package.json`, ...), synchronises the lockfile, and stubs `docs/releases/v<version>.md` if it is missing.

## RELEASE NOTES
Every tag has a notes document at `docs/releases/v<version>.md`.
- **Prose:** your configured spelling variant. Preserve code symbols and identifiers verbatim.
- **Attribution:** none for an AI. The user is the sole author.
- **Structure:**
  1. **Headline:** one sentence naming the key deliverables and the reason.
  2. **Added:** new capabilities, flags or interfaces.
  3. **Changed:** modified behaviour, performance work, refactors.
  4. **Fixed:** bug fixes and error-handling improvements.
  5. **Upgrade and install:** the per-platform install or upgrade commands.
  6. **Known limitations** (if any): deferred items and external constraints.

## PUBLISHING WORKFLOW
1. **Bump:** `make bump-version <patch|minor|major>`.
2. **Edit the notes** in `docs/releases/v<version>.md`.
3. **Commit and tag:** `git commit -am "chore(release): cut <version>"`, then `git tag -a v<version> -m "Release v<version>"`.
4. **Push** the branch, then the tag.
5. **Verify in CI and the repository:** watch the build to success, then confirm the package candidate resolves (for apt, `apt-cache policy <app>`).

Publish a GitHub Release from the notes file as described in `git-workflow-expert`.

### Beta builds (test a branch before merging)
1. Push the branch with its base version bumped past stable (see monotonicity above).
2. Trigger the beta build through your pipeline and wait for success.
3. Install the beta from the beta suite to test it.
4. When the branch is merged or abandoned, reset the beta task to the default branch and rebuild.
