# DELETE ME BEFORE MERGE — machine handover notes

Scratch file. Not part of the change. Delete before merging `perf`.

## Nothing is lost

All 8 commits of `perf` are pushed: `origin/perf == local perf == d6bcffa`, verified with
`git ls-remote`. No stashes, no untracked work, no local-only branches. The machine can go.

## CI has never run on this branch

`.github/workflows/ci.yml` triggers on `push: branches: [master]`, but the default branch is
`main`. So push CI is dead for every branch. Only the `pull_request` trigger fires.

**`perf` has therefore never been CI-verified.** All verification was local, on one machine:

- Windows 10, `rustc 1.97.0-nightly (9ec5d5f32 2026-04-21)`
- `cargo test --all-features` — 141 pass (94 doc, 3 fixture, 44 unit), insta snapshots unchanged
- `cargo clippy --all-targets` clean, `cargo fmt --check` clean

Not checked anywhere: Linux/macOS, stable toolchain, MSRV. The new dev-deps (`criterion 0.5`,
`syn 2` with `full,parsing`) raise the effective MSRV for `cargo test`/`cargo bench` — nobody
measured by how much. Open a PR first; that is the only way to get CI eyes on this.

## PLAN.md is committed on this branch

`PLAN.md` (commit `8df1754`) is a working document, not crate content. Decide whether it belongs
in `main` or should be dropped from the branch. Its §9 has the code-review record and the two
actionables left open on purpose:

- baseline benchmark comparison is a one-off manual `git worktree` run, not scripted/versioned
- §7.2 (rustc self-profile, godot-rust A/B) never executed

## Criterion baselines die with the machine

`cargo bench` history lives in `target/criterion/` (gitignored, machine-local). The v0.6.1 vs
`perf` numbers in PLAN.md §0 were measured by hand in a deleted worktree and cannot be
reproduced from repo state. `cargo bench` on a new machine gives current numbers only — no
before/after. If the comparison matters for the release, script it (see PLAN.md §0).

## Nothing else

No credentials, no local config, no uncommitted experiments, no environment setup beyond a
stock Rust toolchain.
