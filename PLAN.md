# PLAN: Parser performance — cursor-based token iteration

Status: implemented on branch `perf` (6 commits), not yet merged to `main`.
Target version: 0.7.0 (contains one breaking API change, see §6).

> **Process note:** future work on this plan (or any plan file in this repo) must be confirmed
> with the user before implementation starts. Do not just execute a plan found on disk.

## 0. Implementation notes (post-hoc)

Design deviated from §2 mid-implementation, driven by the benchmarks §7 called for:

- The `TokenIter` design in §2 buffers into a `Vec<TokenTree>` and has `next()` **clone** the
  token out (kept the buffer intact for backtracking). Benchmarked against the `Peekable`
  baseline (v0.6.1, commit `c5b6039`) in a throwaway worktree — struct-heavy parsing was
  ~40-70% **slower** than baseline, because `Ident`/`Literal` clones allocate in proc-macro2
  fallback mode, and `Peekable` used to move tokens out instead of cloning.
- Fixed by dropping `checkpoint()`/`rollback()` entirely and making `TokenIter` wrap
  `vec::IntoIter` (moves tokens out in `next()`, uses `as_slice()` for lookahead). All
  backtracking sites — including `parse_generic_arg` in `parse_type.rs`, which this plan's §3.2
  didn't enumerate — were rewritten to decide via `peek_n` lookahead before consuming, instead
  of checkpoint+rollback. Net result: no checkpoint/rollback API at all, simpler than §2's
  design.
- §3.3/§3.4 (attribute-ownership, keyword-allocation fixes) implemented as specified.

### Benchmark results

Bench infra (`benches/parse_items.rs`, `benches/common/fixtures.rs`, `tests/bench_fixtures.rs`,
criterion+syn dev-deps) is committed and versioned — `cargo bench` reproduces the *current*
numbers on this branch.

The **before/after comparison against v0.6.1 is not reproducible from repo state** — it was
measured once by hand: `git worktree add <tmp> c5b6039`, copy the bench files into that
worktree, `cargo bench` there, diff against the numbers on `perf`, delete the worktree. Numbers
(fallback-mode proc-macro2, not representative of compiler-mode `Ident::to_string` savings):

| Case | v0.6.1 (baseline) | perf (this branch) |
|---|---|---|
| impl-heavy (100 methods) | 14.4 ms | 1.5 ms (~10x) |
| path-heavy + `as_path` | 11.9 ms | 3.2 ms (~3.7x) |
| struct-heavy | 10.1 ms | 9.7-13.5 ms (parity, noisy) |

If this comparison needs to be re-run or kept current, it should be scripted (e.g. an `xtask`
or `justfile` target that worktrees a given commit and diffs bench output), not repeated by
hand.

### Known gaps

- §7.2 (rustc self-profile measurement, godot-rust A/B) — not done. Still just a documented
  manual procedure, no real consumer crate was profiled.
- CHANGELOG.md added and version bumped to 0.7.0, but not merged/released.

## 1. Background

venial parses items from `proc_macro2::TokenStream` using
`Peekable<proc_macro2::token_stream::IntoIter>` (aliased as `TokenIter`). `Peekable` only
offers one-token lookahead and no cheap checkpoint, so every place that needs to backtrack
clones the entire remaining iterator:

```rust
// TODO consider multiple-lookahead instead of potentially cloning many tokens
let before_start = tokens.clone();
// ... try to parse ...
*tokens = before_start; // rollback on mismatch
```

This TODO comment appears at 5 sites:

| Site | Function | Why it backtracks |
|---|---|---|
| `src/parse_utils.rs:96` | `consume_attributes_with_inner` | distinguish `#![...]` from `#[...]` after consuming `#` |
| `src/parse_utils.rs:283` | `consume_colon2` | `::` needs two-token lookahead |
| `src/parse_type.rs:171` | `consume_generic_args` | turbofish `::<` vs plain `<` vs neither |
| `src/parse_fn.rs:153` | `consume_fn` | variable-length qualifier prefix (`default const async unsafe extern`) before deciding fn/const/static/trait/impl/mod/extern-block |
| `src/parse_fn.rs:260` | `consume_macro` | `ident ! (...)` speculative parse |

Cost: cloning `Peekable<IntoIter>` is O(remaining tokens) in **both** proc-macro2 modes
(fallback mode clones a `vec::IntoIter<TokenTree>` — new Vec allocation plus per-token clone;
compiler mode clones `proc_macro::token_stream::IntoIter`, same shape). Consequences:

- `consume_colon2` is called per path segment (via `consume_path` / `consume_generic_args`),
  cloning the rest of the stream each time → **quadratic** in type-expression length.
- `consume_fn` clones the remaining impl-body iterator once per member → **quadratic** in
  impl-body size. An `impl` block with 50 methods pays ~50²/2 token clones.

## 2. Core design: index cursor over a collected buffer

Replace `Peekable<IntoIter>` with a struct that collects each token stream into a `Vec` once
and tracks a position index. Rollback becomes copying a `usize`. Same idea as syn's
`TokenBuffer`/`Cursor`, without lifetime plumbing.

New file `src/token_iter.rs` (or extend `parse_utils.rs`; new file preferred):

```rust
use proc_macro2::{TokenStream, TokenTree};

/// Token iterator with O(1) checkpoint/rollback and arbitrary lookahead.
///
/// Collects the stream into a buffer once; all navigation is index-based.
/// Deliberately does NOT implement `Clone` — use `checkpoint()`/`rollback()`.
pub struct TokenIter {
    tokens: Vec<TokenTree>,
    pos: usize,
}

/// Opaque saved position, obtained from `TokenIter::checkpoint`.
pub(crate) struct Checkpoint(usize);

impl TokenIter {
    pub fn new(stream: TokenStream) -> Self {
        Self { tokens: stream.into_iter().collect(), pos: 0 }
    }

    pub(crate) fn from_slice(slice: &[TokenTree]) -> Self {
        Self { tokens: slice.to_vec(), pos: 0 } // TokenTree clone = cheap (Rc-based)
    }

    /// Peek at the next token without consuming (replaces `Peekable::peek`).
    pub(crate) fn peek(&self) -> Option<&TokenTree> {
        self.tokens.get(self.pos)
    }

    /// Peek `n` tokens ahead; `peek_n(0) == peek()`.
    pub(crate) fn peek_n(&self, n: usize) -> Option<&TokenTree> {
        self.tokens.get(self.pos + n)
    }

    pub(crate) fn checkpoint(&self) -> Checkpoint {
        Checkpoint(self.pos)
    }

    pub(crate) fn rollback(&mut self, checkpoint: Checkpoint) {
        self.pos = checkpoint.0;
    }
}

impl Iterator for TokenIter {
    type Item = TokenTree;
    fn next(&mut self) -> Option<TokenTree> {
        let tt = self.tokens.get(self.pos)?.clone();
        self.pos += 1;
        Some(tt)
    }
}

impl From<TokenStream> for TokenIter {
    fn from(stream: TokenStream) -> Self {
        Self::new(stream)
    }
}
```

Notes:

- `next()` clones the token instead of moving it. `TokenTree` clone is cheap (Rc bump in
  fallback mode). Negligible next to the removed O(n²) iterator clones.
- No `Clone` impl — the compiler then flags every leftover `tokens.clone()` rollback site
  during migration. Keep it that way.
- Memory: buffer holds one group-nesting level at a time (children stay behind their `Group`
  handle until entered), so total work stays O(total tokens), same as today.
- `Iterator` impl keeps existing call sites working: `tokens.collect::<TokenStream>()`
  (`src/parse.rs:66`), `tokens.collect()` (`src/types_edition.rs:786`), `for`-style loops.

## 3. Migration steps

### 3.1 Replace the alias everywhere

- Canonical definition currently: `src/parse_utils.rs:6`
  (`pub(crate) type TokenIter = Peekable<...>`). Replace with `pub use`/re-export of the new
  struct, or import from `token_iter` module.
- Delete duplicate local aliases: `src/parse_fn.rs:17`, `src/parse_type.rs:14`,
  `src/parse_impl.rs:17`.
- `src/parse_mod.rs:13` uses `&mut Peekable<IntoIter>` directly → change to `&mut TokenIter`.
- `src/parse_type.rs:130` `consume_lifetime` is generic over
  `Peekable<impl Iterator<Item = TokenTree>>` → change to `&mut TokenIter` (all callers pass
  token iterators; unify).
- All construction sites `X.into_iter().peekable()` → `TokenIter::new(X)`:
  `src/parse.rs:60`, `src/parse_fn.rs:75`, `src/parse_mod.rs:36`, `src/parse_type.rs:97`,
  `src/parse_type.rs:329`, `src/parse_type.rs:361`, `src/parse_type.rs:397`,
  `src/parse_impl.rs:194`, `src/parse_utils.rs:130`, `src/types_edition.rs:764`,
  `src/tests.rs:31`.
- `tokens_from_slice` (`src/parse_utils.rs:8`): currently rebuilds a whole `TokenStream` via
  `TokenStream::from_iter(slice.iter().cloned())` and re-iterates it. Replace with
  `TokenIter::from_slice` (used by `TypeExpr::as_path` via `src/types_edition.rs:804-809` —
  hot in downstream users like godot-rust).

### 3.2 Rework the 5 backtracking sites

1. **`consume_colon2` (`src/parse_utils.rs:282`)** — no rollback needed anymore:
   ```rust
   pub(crate) fn consume_colon2(tokens: &mut TokenIter) -> Option<[Punct; 2]> {
       let first = match tokens.peek_n(0) {
           Some(TokenTree::Punct(p)) if p.as_char() == ':' && p.spacing() == Spacing::Joint => p.clone(),
           _ => return None,
       };
       let second = match tokens.peek_n(1) {
           Some(TokenTree::Punct(p)) if p.as_char() == ':' && p.spacing() == Spacing::Alone => p.clone(),
           _ => return None,
       };
       tokens.next();
       tokens.next();
       Some([first, second])
   }
   ```

2. **Inner-attribute boundary (`src/parse_utils.rs:94-116`)** — peek `#` at `peek_n(0)` and
   decide on `peek_n(1)` (`!` → inner attribute, `[`-delimited group → outer attribute, stop)
   *before* consuming anything. No saved iterator.

3. **`consume_generic_args` (`src/parse_type.rs:170`)** — decide via lookahead: if
   `peek` is `<` → plain generic list; if next two tokens are `::` (same test as
   `consume_colon2`) and `peek_n(2)` is `<` → turbofish; otherwise return `None` without
   consuming. Alternatively keep the current structure with `checkpoint()`/`rollback()` —
   both are O(1); pure lookahead is preferred (no rollback at all).

4. **`consume_fn` (`src/parse_fn.rs:148`)** — keep the rollback (qualifier prefix is
   variable-length), but via checkpoint:
   ```rust
   let start = tokens.checkpoint();
   // ...
   tokens.rollback(start); // at each `*tokens = before_start;` site
   ```
   Applies to all rollback points in that function (`parse_fn.rs:164`, `168`, `190`, `199`,
   `207`).

5. **`consume_macro` (`src/parse_fn.rs:259`)** — same checkpoint pattern.

Remove all 5 TODO comments.

### 3.3 Fix redundant clones of attributes/vis (independent, do after 3.2)

`src/parse_impl.rs:138` clones `attributes` and `vis_marker` for **every** fn-shaped impl
member, only so the error fallback can reuse them (attributes are often nonempty in real
code, e.g. `#[func]` in godot-rust — each clone is a `Vec` of `Vec<TokenTree>`).

- Change `consume_fn` to give ownership back on error:
  ```rust
  pub(crate) struct NotFunctionError {
      pub kind: NotFunction,
      pub attributes: Vec<Attribute>,
      pub vis_marker: Option<VisMarker>,
  }
  pub(crate) fn consume_fn(...) -> Result<Function, NotFunctionError>
  ```
  Update the single dispatch site `consume_either_fn_type_const_static_impl`
  (`src/parse_impl.rs:138-168`) to destructure the error and pass the returned
  `attributes`/`vis_marker` to the fallback parsers. Delete the `.clone()` calls.
- Change `consume_macro` to `Result<Macro, Vec<Attribute>>` (Err returns attribute ownership)
  and remove `attributes.clone()` at `src/parse_impl.rs:177`. Caller at `src/parse.rs:225`
  adjusts trivially (its error path panics, attributes unused — `.ok()` or match).

### 3.4 Fix keyword-comparison allocations (independent, small)

In compiler mode (real macro expansion), proc-macro2's `Ident == "str"` calls
`Ident::to_string()` → one String allocation **per comparison**. Invisible in fallback-mode
benchmarks, real cost during actual compilation.

- `consume_item` dispatch (`src/parse.rs:90-233`): up to ~10 sequential `keyword == "..."`
  guards plus `keyword.to_string()` in the `matches!` at `src/parse.rs:212`. Restructure: on
  `Some(TokenTree::Ident(keyword))`, do `let kw = keyword.to_string();` once, then
  `match kw.as_str() { "struct" => ..., "enum" => ..., ... }` with the macro fallback in the
  `_` arm.
- `consume_either_fn_type_const_static_impl` (`src/parse_impl.rs:128-137`): already calls
  `to_string()` once — fine, leave as is.
- `consume_fn_qualifiers` (`src/parse_fn.rs:33`): 4 `consume_ident` calls each compare the
  *same* peeked token → up to 4 allocations for a plain `fn`. Restructure as a loop: peek
  ident, `to_string()` once, match on `"default" | "const" | "async" | "unsafe"` (respecting
  their fixed grammar order — track which qualifiers were already seen and only accept later
  ones), then the existing `extern` + ABI-literal handling.
- Leave single-comparison call sites of `consume_ident` (`"pub"`, `"where"`, `"mut"`,
  `"self"`, ...) unchanged.

## 4. What NOT to change

- Output types (`Struct`, `Function`, `TypeExpr { tokens: Vec<TokenTree> }`, `Punctuated`,
  ...) stay identical — snapshot tests must not change.
- `consume_stuff_until` still returns `Vec<TokenTree>` (results are stored in output types).
- Panic-based error reporting stays (matches crate philosophy, error paths are cold).
- No changes to `types.rs` / `types_edition.rs` semantics beyond the iterator type swap.

## 5. Order of implementation

1. Add `TokenIter` struct + swap alias + mechanical migration (§3.1). Compiler drives this:
   missing `Clone` flags every rollback site.
2. Rework the 5 backtracking sites (§3.2).
3. `cargo test` — all snapshot tests must pass byte-identically.
4. Attribute/vis ownership in errors (§3.3). Test again.
5. Keyword allocation fix (§3.4). Test again.
6. Benchmarks (§7).

## 6. Public API impact

- `pub fn consume_item(tokens: &mut Peekable<IntoIter>)` (`src/parse.rs:86`, re-exported in
  `lib.rs:121`) must change to `&mut TokenIter`. **Breaking change** → bump version to
  `0.7.0`. Export `TokenIter` from `lib.rs` as an opaque type with `new`, `From<TokenStream>`,
  and the `Iterator` impl public; keep `checkpoint`/`rollback`/`peek_n` `pub(crate)`.
- `parse_item(tokens: TokenStream)` signature unchanged.
- Add CHANGELOG entry: breaking `consume_item` signature, perf rework, no behavior change.

## 7. Benchmarks & measurement

### 7.1 In-repo criterion benches (fallback mode — catches algorithmic wins)

Add to `Cargo.toml`:

```toml
[dev-dependencies]
criterion = "0.5"
syn = { version = "2", features = ["full", "parsing"] } # comparison baseline

[[bench]]
name = "parse_items"
harness = false
```

`benches/parse_items.rs`, cases (each parses a pre-lexed `TokenStream`; build streams with
`quote!` or `TokenStream::from_str` OUTSIDE the timed section):

1. **struct-heavy**: ~100 structs with attributes, generics, where-clauses, 10-30 fields with
   nontrivial types (generate programmatically with `quote!` in a loop).
2. **impl-heavy**: single `impl` block with 100-200 methods, each with `#[attr]`, params, and
   return types — this is the O(n²) `consume_fn` case; expect the largest improvement.
3. **path-heavy**: types with long paths and nested generics, e.g.
   `a::b::c::d::Foo<x::y::Bar<T, U>, Baz<'a, N>>` repeated — the O(n²) `consume_colon2` case.
4. **syn comparison**: same streams through `syn::parse2::<syn::File>` (wrap items in a file)
   or `syn::parse2::<syn::DeriveInput>` per item vs `venial::parse_item` per item. Iterate
   venial over multiple items in one stream via `consume_item` in a loop.

Constraint: fixtures must only contain item kinds venial supports (struct/enum/union/fn/
impl/trait/mod/const/static/type/use/extern/macro invocations). Add a plain `#[test]` that
parses every fixture once, so corpus breakage fails tests, not benches.

Optionally add allocation counting (e.g. `dhat` heap profiling behind a feature, or a
counting global allocator in one bench) — the iterator-clone fix shows up as a large drop in
allocation count even when wall-time is noisy.

Record before/after numbers for all cases in the PR description.

### 7.2 Real-world in-compiler measurement (documentation, manual procedure)

Fallback-mode benches understate two wins (bridge-mode `Ident::to_string`, iterator clone
behavior differs). Honest metric is measured inside rustc. Document in the PR (not CI):

- **rustc self-profile** (nightly): build a real venial-consumer crate (e.g. godot-rust/gdext,
  which uses venial) with
  `RUSTFLAGS="-Zself-profile -Zself-profile-events=default,args"`, summarize with
  `measureme`'s `summarize` tool, compare `expand_proc_macro` event totals before/after
  (patch venial via `[patch.crates-io]`).
- **End-to-end A/B vs syn**: two implementations of the same trivial derive macro (one venial,
  one syn), consumer crate with ~1000 derived structs,
  `hyperfine 'cargo build'` with the macro crate prebuilt. Separately note cold-build time
  including dependency tree (venial's headline advantage lives there).

## 8. Acceptance criteria

- [ ] `cargo test` passes; all insta snapshots byte-identical (no `.snap` changes).
- [ ] `cargo clippy --all-targets` clean; `cargo fmt` applied.
- [ ] Zero remaining `tokens.clone()` rollback patterns; all 5 TODO comments gone.
- [ ] `TokenIter` has no `Clone` impl.
- [ ] No `attributes.clone()` / `vis_marker.clone()` in `src/parse_impl.rs` dispatch.
- [ ] `consume_item` dispatch does at most one `to_string()` per item keyword;
      `consume_fn_qualifiers` at most one per qualifier token.
- [ ] Criterion benches added and running; impl-heavy and path-heavy cases show measurable
      improvement over pre-change baseline (run baseline on the commit before the change).
- [ ] Version bumped to 0.7.0, CHANGELOG entry for the `consume_item` breaking change.

## 9. Code review (post-implementation)

Full read of every line changed across all 6 commits on `perf` (`git diff main..perf`), plus
manual trace of malformed-input edge cases through the rewritten `consume_fn` qualifier
lookahead (the highest-risk piece, since it re-derives — via a second, parallel state machine —
a decision that `consume_fn_qualifiers` also computes) against the pre-refactor code. No
correctness bugs found; behavior and panic messages match byte-for-byte in every traced case,
and this is corroborated by all 141 tests (94 doc + 3 fixture + 44 unit) passing unchanged with
byte-identical insta snapshots.

> **Process note:** the actionables below are not authorized for implementation by writing them
> here. Confirm with the user, one at a time or as a batch, before touching code for any of
> them — this applies to executing *any* actionable list in this plan file, not just this one.

### Actionables

Status after the follow-up review commit: **1 and 2 resolved**, 5 rejected (see below), 3 and 4
still open (deliberately out of scope).

1. **Duplicated qualifier state machine (maintainability risk).** — *resolved.* `consume_fn` (`src/parse_fn.rs:186-222`)
   re-implements the `default → const → async → unsafe → extern "abi"` ordering/staging logic
   from scratch via `peek_n`, purely to compute how many tokens to look past before deciding
   fn-vs-fallback. `consume_fn_qualifiers` (`src/parse_fn.rs:39-70`) implements the *same*
   ordering logic again, separately, to actually consume and build `FnQualifiers`. The two must
   stay in lockstep by hand — if one changes (e.g. Rust ever adds a new fn qualifier, or the
   crate wants to relax ordering) and the other doesn't, the mismatch surfaces as a confusing
   panic or an `unreachable!()` hit, not a compile error. Consider unifying: e.g. have
   `consume_fn` call `consume_fn_qualifiers`-style scanning once, sharing one function that
   either just counts (`usize`) or counts-and-builds depending on a flag.

   Fixed by extracting `scan_fn_qualifiers(&TokenIter) -> QualifierScan`, a single lookahead-only
   scanner returning the token offset of each qualifier present plus the prefix length.
   `consume_fn_qualifiers` builds `FnQualifiers` from those offsets and then advances the
   iterator; `consume_fn` uses `scan.len` and `scan.has_only_const_xor_unsafe()`. One state
   machine, one `to_string()` per qualifier token.

2. **`TokenIter::peek()` made `pub`, unplanned.** — *resolved:* confirmed intentional, documented
   in the CHANGELOG under a new "Added" section. §6 of this plan only specifies `new`,
   `From<TokenStream>`, and `Iterator` as public API; `checkpoint`/`rollback`/`peek_n` were to
   stay `pub(crate)` (moot now, since checkpoint/rollback no longer exist — see §0). `peek()`
   was made `pub` in commit `bda3791` because the benchmark harness needed it to detect
   end-of-stream between multiple `consume_item` calls on one stream. This is a reasonable,
   probably-permanent addition to the public surface (downstream users parsing multiple items
   from one stream need it too), but it wasn't explicitly signed off — confirm intentional and
   mention it in the CHANGELOG's breaking-change section (it's additive, not breaking, but
   still public-API-relevant).
3. **Baseline benchmark comparison isn't reproducible or versioned** (already flagged in §0).
   Numbers in §0's table came from a one-off manual `git worktree` + copy-paste run, not from
   anything in the repo. If this comparison matters going forward (e.g. re-running after
   further perf work, or before a release), script it — e.g. an `xtask`/`justfile` target that
   worktrees a given commit, copies/runs the current bench files against it, and diffs.
4. **§7.2 not done** (already flagged in §0): no rustc self-profile run, no godot-rust A/B. Still
   just documented procedure, not executed.
5. **Minor style nit:** `NotFunctionError` is `pub(crate)` but its fields are declared `pub`.
   — *rejected, the nit was wrong.* The fields are destructured from `parse_impl.rs`, a different
   module, so bare (module-private) fields would not compile. `pub` fields on a `pub(crate)`
   struct is the idiomatic spelling here; the struct's visibility already caps the effective one.

### Additional fixes in the follow-up review commit

- `consume_fn`'s doc comment claimed it returns `None` for a `const` fallback; it returns
  `Err(NotFunctionError)` and covers seven fallback kinds. Rewritten, along with the stale
  `NotFunction` enum doc (which listed three of the seven variants).
- Removed a dead `Some(TokenTree::Literal(_)) if has_extern` arm in `consume_fn`, commented
  `// extern "C" { ...`. The ABI literal is always part of the qualifier scan, so that arm could
  only be reached by malformed input like `extern "C" "D"`; `extern "C" { ... }` hits the `Group`
  arm. Dead since before this branch.
- Merged `consume_macro_inner` into `consume_macro`. The inner function's `Option` return and the
  `attributes: Vec::new()` placeholder overwritten by the caller both existed only because the
  lookahead check lived in the outer function; with the check up front, the inner parse cannot
  fail and attributes can be moved in directly.
- Deduplicated the two identical macro-fallback panic sites in `consume_item` into
  `consume_macro_or_panic`.
