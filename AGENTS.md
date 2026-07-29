# AGENTS.md

## Agent Contract

The agent must follow these rules on every task. When in doubt, ask the user
before acting.

- **Assign issues first**: The very first action on any issue handed to you
  (an issue URL, "do #N", or any reference to an issue number) is to run
  `./scripts/issue-assign.sh <N>` to claim it. Do this **before** reading the
  issue body, exploring the codebase, or writing any code. If the script
  reports `already-assigned` and exits non-zero, **stop immediately**: another
  agent (or human) already owns it. Do not continue working on the issue.
  Report the conflict to the user and ask how to proceed. This rule exists
  because multiple agents share one GitHub account, so the assignee field is
  the only reliable ownership signal.
  See [`.devin/rules/agent-helpers.md`](./.devin/rules/agent-helpers.md).
- **Create PRs for issues**: When the user provides an issue to close (e.g. an
  issue URL or "do #N"), create a PR that closes it. Do not wait for a separate
  "create PR" request.
  See [`.devin/rules/pr-workflow.md`](./.devin/rules/pr-workflow.md).
- **Version control**: Use Jujutsu (`jj`) for all mutations. Do not use `git` to
  rewrite history.
  See [`.devin/rules/pr-workflow.md`](./.devin/rules/pr-workflow.md).
- **No surprise pushes**: Do not push to GitHub or create/update a PR unless the
  user explicitly asks, an issue was provided to close, or an existing PR needs
  updating.
  See [`.devin/rules/pr-workflow.md`](./.devin/rules/pr-workflow.md).
- **Notify on finish**: Send a desktop notification with `dunstify` once at the
  end of a task, or when asking the user for input.
  See [`.devin/rules/agent-helpers.md`](./.devin/rules/agent-helpers.md).
- **Verify before finishing**: Run the checks appropriate to the changed code
  (`cargo check --workspace`, `cargo nextest run --workspace`, `cargo clippy --workspace --all-targets --all-features`;
  mobile `npm run lint`, `npx tsc --noEmit`, `npm run fmt:check`).
- **Write the commit**: After finishing work, run `jj describe`. Commit messages
  are present tense, lowercase first word, no trailing period.
- **Rebase before push**: Never rewrite `main`. Before pushing, run
  `jj git fetch && jj rebase -r @ -d main`. Push with
  `jj git push --change @`.

## Detailed guidance

Read the relevant file before starting work on a topic:

| Topic | File |
|-------|------|
| PR / version control | [`.devin/rules/pr-workflow.md`](./.devin/rules/pr-workflow.md) |
| Notifications, scripts, and skills | [`.devin/rules/agent-helpers.md`](./.devin/rules/agent-helpers.md) |
| Project overview and tech stack | [`.devin/docs/project-overview.md`](./.devin/docs/project-overview.md) |
| Project structure | [`.devin/docs/project-structure.md`](./.devin/docs/project-structure.md) |
| Development environment and commands | [`.devin/docs/development-environment.md`](./.devin/docs/development-environment.md) |
| Core planner (takusu-core) | [`.devin/docs/core-planner.md`](./.devin/docs/core-planner.md) |
| Audio processing | [`.devin/docs/audio.md`](./.devin/docs/audio.md) |
| Local API and architecture | [`.devin/docs/local-api.md`](./.devin/docs/local-api.md) |
| CLI and HTTP client | [`.devin/docs/clients.md`](./.devin/docs/clients.md) |
| Code style and brittle code | [`.devin/docs/code-style.md`](./.devin/docs/code-style.md) |
| Agent implementation notes | [`.devin/docs/agent-implementation.md`](./.devin/docs/agent-implementation.md) |
| Workspace dependencies | [`.devin/docs/dependencies.md`](./.devin/docs/dependencies.md) |
