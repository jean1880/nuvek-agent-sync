# C# / .NET

Read the repository's own `AGENTS.md` and layout notes before changing anything. The points below
hold for most .NET projects, and especially for ones with a deterministic core and an engine or UI on top.

## Shape that matters
- **Keep the core engine-free.** Logic that needs an engine or UI type belongs in the host project;
  anything testable belongs in a library with no engine references.
- **Target frameworks can differ on purpose.** A library consumed by a host that supports an older
  framework must target that framework; the test project may target the installed SDK and consume it.
  A project cannot reference an assembly that targets a newer framework, so never raise the library's
  target to match the tests. `RollForward` lets the host run the library on a newer runtime.
- **Let the compiler enforce invariants.** Analyzers such as `Microsoft.CodeAnalysis.BannedApiAnalyzers`
  (a `BannedSymbols.txt`) can fail the build on floats, wall-clock time or ambient randomness where
  determinism matters, and `TreatWarningsAsErrors` makes that a hard stop. Do not relax a ban list to
  get a build through.
- **Pin what changes results.** A physics or serialisation library whose update alters outputs (replay
  hashes, golden files) is pinned. Upgrading it is a deliberate change that regenerates the goldens.

## Gate
Run from the repository root:
1. `dotnet build`: zero warnings is the bar where warnings are errors.
2. `dotnet test`: xunit v3 on Microsoft.Testing.Platform needs the repository's `dotnet.config` found from
   the CURRENT directory. Run from a subdirectory and test discovery can fail. Running the built test
   executable directly also works.
3. `dotnet format --verify-no-changes` for style when analyzers are tied to the build.
4. A smoke run of the host when it changed.

## Tests
- xunit v3 test projects are executables, so a `Main` is generated; do not add `xunit.runner.visualstudio`.
- Keep tests serial when the code under test keeps process-wide static state that is not thread-safe.
  Parallelise across processes, never threads, in a tool that runs many scenarios.
- Prefer golden-output and same-input-same-output determinism tests for core changes. A behaviour change
  with no replay or scenario test is unfinished.
- Write a test that fails first for a bug.

## Review
There is no .NET-specific reviewer agent. Review as a separate pass from writing, brief the reviewer with
the gate output and any determinism contract, and say which review ran.
