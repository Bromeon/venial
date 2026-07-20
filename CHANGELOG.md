# Changelog

## v0.7.0

Performance rework of the parser's token iteration. No change in parsing behavior or output types.

### Breaking changes

- `consume_item` now takes `&mut venial::TokenIter` instead of
  `&mut Peekable<proc_macro2::token_stream::IntoIter>`. Construct one with
  `TokenIter::new(stream)` or `TokenIter::from(stream)`; it implements `Iterator` and offers
  `peek()`.

### Performance

- Token iteration no longer clones the remaining iterator for backtracking; all speculative
  parsing decisions are made via lookahead before consuming tokens. This removes quadratic
  behavior in impl bodies (per member) and path expressions (per `::` segment).
- Fewer `Ident::to_string` calls in keyword dispatch, which allocate per comparison during
  real macro expansion (compiler mode).
- Attributes and visibility markers are no longer cloned for every fn-shaped impl member.
- Added criterion benchmarks (`cargo bench`): impl-heavy ~10x faster, path-heavy
  (`TypeExpr::as_path`) ~3.5x faster, struct-heavy on par with v0.6.1.
