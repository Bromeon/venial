//! Parses every bench fixture once, so corpus breakage fails tests, not benches.

use venial::{Fields, Item, TokenIter};

#[path = "../benches/common/fixtures.rs"]
mod fixtures;

fn parse_all(stream: proc_macro2::TokenStream) -> Vec<Item> {
    let mut tokens = TokenIter::new(stream);
    let mut items = Vec::new();
    while tokens.peek().is_some() {
        items.push(venial::consume_item(&mut tokens).expect("bench fixture must parse"));
    }
    items
}

#[test]
fn struct_heavy_parses() {
    let items = parse_all(fixtures::struct_heavy(10));
    assert_eq!(items.len(), 10);
    assert!(items.iter().all(|item| matches!(item, Item::Struct(_))));
}

#[test]
fn impl_heavy_parses() {
    let items = parse_all(fixtures::impl_heavy(10));
    assert_eq!(items.len(), 1);
    match &items[0] {
        Item::Impl(impl_) => assert_eq!(impl_.body_items.len(), 10),
        other => panic!("expected impl block, got {:?}", other),
    }
}

#[test]
fn path_heavy_parses_and_resolves_paths() {
    let items = parse_all(fixtures::path_heavy(10));
    assert_eq!(items.len(), 10);

    for item in items {
        let Item::Struct(struct_) = item else {
            panic!("expected struct");
        };
        let Fields::Named(fields) = &struct_.fields else {
            panic!("expected named fields");
        };
        for field in fields.fields.items() {
            assert!(
                field.ty.as_path().is_some(),
                "field type must resolve as path: {:?}",
                field.ty
            );
        }
    }
}
