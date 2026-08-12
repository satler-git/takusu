+++
name = "search-qualifiers"
description = "Task and memory search qualifier syntax reference."
+++

# Search qualifier syntax

Use these qualifiers in `list_tasks`/`search_tasks` `q` and in `memory_search` `q` when supported.

## Boolean syntax
- `AND` or space: both terms must match.
- `OR`: either side matches.
- `-` or `NOT`: exclude.
- `()` group expressions.

## Task search qualifiers
- `status:<pending|scheduled|in_progress|completed|skipped|overdue>`
- `title:<text>`, `desc:<text>`
- `start:<date>`, `end:<date>`
- `scheduled-start:<date>`, `scheduled-end:<date>`
- `from:<date>` (alias for `end:>=`), `until:<date>` (alias for `start:<=`)
- `habit:<hN|N>`
- `depends:<#N|UUID>` — task depends on the given task
- `dependents:<#N|UUID>` — tasks that depend on the given task
- `deps_count:<op>N` — e.g. `>0`, `2`
- `is:<fixed|parallelizable|allows_parallel|overdue>`
- `has:<description|completed_at|schedule|depends>`

Dates accept `YYYY-MM-DD`, `today`, `tomorrow`, `yesterday`, `Nd` (relative), and operators like `>=2026-07-25`.

## Memory search syntax
- Multiple keywords are ANDed.
- `*` is a wildcard matching any sequence of characters.
- `kind` parameter accepts comma-separated OR values: `proper_noun,fact`.

## Examples
- `status:pending 買い物`
- `end:today OR end:tomorrow`
- `-habit:h1`
- `depends:#42`
- `deps_count:>0`
- `研究室 大学`
- `研究*大学`
- `kind=proper_noun,fact`
