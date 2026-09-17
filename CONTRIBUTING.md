# Contributing to Codeg

Thanks for taking the time to contribute. Bug fixes, focused improvements,
documentation, and well-scoped features are welcome.

This guide is intentionally lightweight. For setup details, see the
[Development guide](https://docs.codeg.app/reference/development). For an
overview of the project, see the [README](./README.md).

## Before You Start

- Search existing [issues](https://github.com/xintaofei/codeg/issues) and
  [pull requests](https://github.com/xintaofei/codeg/pulls) before starting.
- Base your work on the latest `main` branch.
- Link bug fixes to an issue when one exists.
- Discuss large features, architecture changes, or new dependencies in an
  issue before investing in an implementation.
- Keep each pull request focused on one problem. Small, focused changes are
  easier to review, test, and merge.

## Reporting Bugs

Please include the details that are relevant to the problem:

- Codeg version or commit
- operating system and CPU architecture
- runtime: Desktop, Server, Docker, or browser client
- agent type and version
- steps to reproduce
- expected and actual behavior
- whether the problem reproduces consistently
- logs, screenshots, or a short recording
- workarounds already tried

Remove tokens, credentials, private paths, and other sensitive information
before posting logs.

## Requesting Features

A useful feature request explains:

- the user problem and concrete use case
- the behavior you would like
- why the current behavior is insufficient
- which runtimes or platforms may be affected
- whether you can help implement or test the change

Please open an issue to agree on direction before implementing a large feature.

## Pull Requests

A pull request should make the change easy to understand and verify. Include:

- the problem being solved
- the chosen approach
- a linked issue, for example `Fixes #123`
- tests run and their results
- screenshots or a recording for visible UI changes
- known risks, compatibility concerns, and untested platforms

Keep the diff reviewable:

- address one topic per pull request
- do not mix in unrelated refactors or formatting
- do not commit generated files unless the change requires them
- update tests and documentation when behavior changes
- respond to review comments and keep the branch mergeable with `main`

### Titles and Commits

Conventional-style titles are encouraged because they match much of the
project's current history, but they are not enforced by commit tooling.
Examples:

```text
fix(chat): preserve draft state
feat(settings): add provider option
docs: add contributing guide
```

Use clear commit messages and remove temporary fixup commits when practical.

## Development and Testing

Install frontend dependencies from the repository root:

```bash
pnpm install --frozen-lockfile
```

Run checks appropriate to the area you changed. A focused change does not need
every command below, while shared or cross-runtime code may need several groups.
CI runs the complete frontend checks and the Rust desktop/server matrix.

### Frontend and UI

Run frontend commands from the repository root:

```bash
pnpm lint
pnpm exec vitest run path/to/relevant.test.ts
pnpm build
```

- Use `pnpm test` for the full Vitest suite when the change is broad or affects
  shared behavior.
- `pnpm build` performs the Next.js production build and TypeScript checking.
- There is currently no standalone `typecheck` script. For a faster type-only
  check, use `pnpm exec tsc --noEmit`.

### Rust Desktop

Run Cargo commands from `src-tauri/`:

```bash
cargo check
cargo test --features test-utils
cargo clippy --all-targets --features test-utils -- -D warnings
```

The `test-utils` feature enables scaffolding required by the integration tests
in `src-tauri/tests/`. Desktop checks also require the platform prerequisites
from the Development guide. Tauri validates the frontend output directory, so
run `pnpm build` from the repository root first if `out/index.html` is absent.

### Server

Run these from `src-tauri/` for server-specific changes:

```bash
cargo check --no-default-features --bin codeg-server
cargo test --no-default-features --bin codeg-server --lib
cargo clippy --no-default-features --bin codeg-server --lib -- -D warnings
```

The server build deliberately disables the default Tauri runtime feature.

### MCP Companion

Run these from `src-tauri/` for changes to `codeg-mcp` or its shared library
implementation:

```bash
cargo check --no-default-features --bin codeg-mcp
cargo test --no-default-features --lib acp::delegation
cargo clippy --no-default-features --bin codeg-mcp -- -D warnings
```

Most MCP behavior lives in the shared `codeg_lib` modules rather than the thin
binary entry point, so relevant tests run through the library target.

If you modify the vendored `sacp-tokio` dependency, run its tests explicitly;
it is a path dependency, not a workspace member:

```bash
cargo test --manifest-path vendor/sacp-tokio/Cargo.toml --lib
```

### Documentation Only

For documentation changes, preview the rendered Markdown, verify new links,
and run:

```bash
git diff --check
```

The repository does not currently define a Markdown lint command. Do not add
new tooling solely for a documentation fix.

## UI and Cross-Platform Changes

For visible UI changes, include a screenshot or short recording. State what you
tested and what you did not test; contributors are not expected to own every
platform. For example:

```text
Tested: macOS desktop
Not tested: Windows, Linux, server/browser mode
```

Consider macOS, Windows, Linux, Desktop, Server, Docker, and browser clients
when the affected code is shared across those environments.

## Documentation and Configuration

When behavior or configuration changes, update the relevant README, docs,
configuration examples, or localized UI messages. Avoid copying detailed setup
instructions into multiple files when a link to the canonical documentation is
enough.

## Security Reports

Do not publish sensitive vulnerability details in a public issue. Use a private
reporting option shown in the repository's Security tab if one is available. If
no private option is listed, ask the maintainers for a private channel without
including exploit details publicly.

## Before Requesting Review

- Rebase or merge the latest `main` as appropriate and resolve conflicts.
- Review the final diff for unrelated edits or generated files.
- Run checks appropriate to the changed area and report the results.
- Verify documentation links and user-visible examples.
- List platforms or runtimes you could not test.
