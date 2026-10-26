# Security policy

## Supported versions

Only the latest release (and `master`) receives security fixes.

## Reporting a vulnerability

Please report vulnerabilities privately through GitHub's
[private vulnerability reporting](https://github.com/jean1880/nuvek-agent-sync/security/advisories/new),
not in a public issue. Only the maintainers see the report.

Include the version, the steps to reproduce and the impact you expect. Leave out real secrets,
tokens, hostnames and home paths: a placeholder shows the problem just as well.

You should get an acknowledgement within a week. Once a fix is released, the advisory is published
with credit to the reporter unless you prefer otherwise.

## Scope

agent-sync writes the configuration that decides what coding agents may run. Of particular
interest:

- a secret-shaped value that `apply` writes to disk despite the pre-write scan;
- a `${NAME}` reference that gets expanded although `NAME` is not in `map.toml` `placeholders`;
- `check` reporting a surface as in sync when a permission, hook or MCP server differs from the
  source;
- an overlay or a source path that makes `apply` write or delete outside the tool directories it
  manages;
- a permission that `promote` widens beyond the entry that was approved.

Vulnerabilities in Claude Code, Antigravity, Codex or a dependency belong with that project.
