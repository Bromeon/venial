//! Criterion benches for item parsing, with syn as comparison baseline.
//!
//! Note: these run proc-macro2 in fallback mode, which shows algorithmic wins (iterator
//! clones, backtracking) but understates `Ident::to_string` costs that only exist in
//! compiler mode during real macro expansion.

use criterion::{criterion_group, criterion_main, Criterion};
use proc_macro2::TokenStream;
use venial::{Fields, Item, TokenIter};

#[path = "common/fixtures.rs"]
mod fixtures;

fn parse_all_venial(stream: TokenStream) -> Vec<Item> {
    let mut tokens = TokenIter::new(stream);
    let mut items = Vec::new();
    while tokens.peek().is_some() {
        items.push(venial::consume_item(&mut tokens).expect("bench fixture must parse"));
    }
    items
}

/// Parses all items, then resolves every named-field type as a path.
fn parse_and_resolve_paths(stream: TokenStream) -> usize {
    let mut paths = 0;
    for item in parse_all_venial(stream) {
        if let Item::Struct(struct_) = item {
            if let Fields::Named(fields) = &struct_.fields {
                for field in fields.fields.items() {
                    if field.ty.as_path().is_some() {
                        paths += 1;
                    }
                }
            }
        }
    }
    paths
}

fn bench_parse_items(c: &mut Criterion) {
    // Build streams outside the timed sections; cloning a TokenStream is cheap (Rc).
    let struct_heavy = fixtures::struct_heavy(100);
    let impl_heavy = fixtures::impl_heavy(150);
    let path_heavy = fixtures::path_heavy(100);

    c.bench_function("venial_struct_heavy", |b| {
        b.iter(|| parse_all_venial(struct_heavy.clone()))
    });
    c.bench_function("venial_impl_heavy", |b| {
        b.iter(|| parse_all_venial(impl_heavy.clone()))
    });
    c.bench_function("venial_path_heavy_as_path", |b| {
        b.iter(|| parse_and_resolve_paths(path_heavy.clone()))
    });

    c.bench_function("syn_struct_heavy", |b| {
        b.iter(|| syn::parse2::<syn::File>(struct_heavy.clone()).unwrap())
    });
    c.bench_function("syn_impl_heavy", |b| {
        b.iter(|| syn::parse2::<syn::File>(impl_heavy.clone()).unwrap())
    });
    c.bench_function("syn_path_heavy", |b| {
        b.iter(|| syn::parse2::<syn::File>(path_heavy.clone()).unwrap())
    });
}

criterion_group!(benches, bench_parse_items);
criterion_main!(benches);
