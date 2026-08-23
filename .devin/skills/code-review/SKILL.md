---
name: code-review
description: Review the current jj change with full context and output the final result in Japanese
allowed-tools:
  - read
  - grep
  - exec
---

Review the current working-copy change tracked by Jujutsu (`@`).

1. Run `jj diff -r @ --stat` to list the files that changed.
2. Run `jj diff -r @` to read the full diff of the change.
3. Read surrounding context and related files that are **not** in the diff.
   - For each modified file, read the full file (or at least the changed regions plus enough surrounding lines to understand the structure).
   - If the change touches a function, type, module, or trait, read the rest of that function/type/module/trait implementation.
   - If the change uses a helper, constant, or configuration, read where that helper is defined and how it is used elsewhere.
   - Use `grep` to find related call sites, definitions, tests, or similar patterns when necessary.
   - Do not review from the diff alone; understand the code that did not change, too.

4. Perform a code review from the following viewpoints:
   - **Correctness**: logic errors, edge cases, off-by-one mistakes, missing error handling, broken invariants.
   - **Security**: secrets, unsafe blocks, injection risks, permission leaks, serialization of sensitive data.
   - **Performance**: needless allocations, hot loops, repeated work, algorithmic inefficiency.
   - **Readability & style**: naming, comments, duplication, complexity, idiomatic Rust (or the project's main language).
   - **Consistency with the codebase**: follows existing patterns, conventions, and module boundaries.
   - **Tests & verification**: whether the change is covered by tests, whether new tests are needed, and whether `cargo check` / `cargo clippy` / `cargo nextest run` would pass.

5. Run the build/test commands appropriate to the changed code. If the project is a Rust workspace, run `cargo check` (and `cargo clippy` / `cargo nextest run` when appropriate) only when the change touches Rust; otherwise, run the relevant package/language checks (e.g. `npm run lint`, `npx tsc --noEmit`, `npm run fmt:check`, `npx jest` for the mobile app). Adapt to the project's actual build/test commands when you detect them.

6. Output the final review result in **Japanese only**.
   - Use clear sections such as: 概要、指摘事項、確認ポイント、推奨事項、総合評価.
   - Cite specific file paths and line numbers for every issue or point.
   - Keep the summary concise but concrete; do not just say "looks good" without checking thoroughly.

7. If everything is fine, say so in Japanese and explain briefly what you verified.
