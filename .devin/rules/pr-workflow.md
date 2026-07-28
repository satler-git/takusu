# PR and Version Control Workflow

## Primary rule

Use **Jujutsu (`jj`)** for all history-mutating operations. `git` may be used
read-only (`git log`, `git diff`).

## When to push or create a PR

Push to GitHub with `jj git push --change` **only when** one of the following
is true:

1. The user explicitly asked you to push or to create/update a PR.
2. The user handed you an issue to close (e.g. an issue URL or "do #N").
3. An existing PR is already associated with the change and the user asked for
   an update.

If none of these apply, **do not push or create a PR**. Notify the user with
`dunstify`, report the result, and wait for an explicit request.

## Issue-driven PRs

If the user provided an issue to close, treat it as explicit permission to push
and create a PR. Do not wait for a separate "create PR" request.

## Basic workflow

1. Do the work.
2. Run appropriate checks (`cargo check`, `cargo nextest run`,
   `cargo clippy --all-targets --all-features`, mobile `npm run lint` /
   `npx tsc --noEmit` / `npm run fmt:check`).
3. `jj describe` with a present-tense, lowercase, no-trailing-period message.
4. If `main` has moved, rebase first: `jj git fetch && jj rebase -r @ -d main`.
5. `jj git push --change @`
6. Create the PR with `gh pr create`. Include `Closes #N` in the PR body so
   GitHub auto-closes the issue on merge.

## Conventions

- Commit messages: present tense, lowercase first word, no trailing period.
  Examples: `iroiro fix`, `chore: fmt`, `separate takusu-local`.
- Push a single change with `jj git push --change` for feature work.
- This repo uses a single `main` bookmark. Feature work is rebased onto `main`.
- Never force-push or rebase `main` itself.
- Do not post "Fixed in #N" comments on issues.

## Common commands

| Command | Purpose |
|---------|---------|
| `jj st` | Show working copy status |
| `jj log -r 'main..@'` | Show commits ahead of main |
| `jj new` | Create a new empty change on top of `@` |
| `jj squash` | Squash `@` into its parent |
| `jj amend` | Amend `@` with working copy changes |
| `jj rebase -r <rev> -d <dest>` | Rebase a change onto another |
| `jj split <rev>` | Split a change interactively |
| `jj describe` | Edit the description of `@` |
| `jj git fetch` | Fetch from the Git remote |
| `jj git push --change @` | Push `@` as a reviewable branch |
