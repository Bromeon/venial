//! Token-stream fixtures shared between the criterion benches and the test that keeps
//! them parseable (`tests/bench_fixtures.rs`).
//!
//! Only item kinds supported by venial may appear here.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};

/// Many structs with attributes, generics, where-clauses and nontrivial field types.
pub fn struct_heavy(count: usize) -> TokenStream {
    let mut stream = TokenStream::new();
    for i in 0..count {
        let name = format_ident!("Struct{}", i);
        let fields = (0..20usize).map(|j| {
            let field = format_ident!("field_{}", j);
            quote! {
                #[serde(rename = "renamed")]
                pub #field: std::collections::HashMap<String, Vec<Option<u64>>>,
            }
        });
        stream.extend(quote! {
            #[derive(Debug, Clone)]
            #[allow(dead_code)]
            pub struct #name<T, U: Clone> where T: Default + Copy {
                #(#fields)*
            }
        });
    }
    stream
}

/// Single impl block with many attributed methods — the formerly quadratic `consume_fn` case.
pub fn impl_heavy(methods: usize) -> TokenStream {
    let methods = (0..methods).map(|i| {
        let method = format_ident!("method_{}", i);
        quote! {
            #[inline]
            #[allow(clippy::too_many_arguments)]
            pub fn #method(&mut self, value: i64, name: &str, data: Vec<u8>) -> Result<Vec<String>, Error> {
                let _ = (value, name, data);
                Ok(Vec::new())
            }
        }
    });
    quote! {
        impl MyStruct {
            #(#methods)*
        }
    }
}

/// Structs whose field types are long paths with nested generics. Callers additionally run
/// `TypeExpr::as_path` on every field type — the formerly quadratic `consume_colon2` case.
pub fn path_heavy(count: usize) -> TokenStream {
    let mut stream = TokenStream::new();
    for i in 0..count {
        let name = format_ident!("Paths{}", i);
        stream.extend(quote! {
            pub struct #name {
                pub first: a::b::c::d::Foo<x::y::Bar<T, U>, z::Baz<N>>,
                pub second: ::core::result::Result<alpha::beta::Gamma<Delta>, epsilon::zeta::Err>,
                pub third: very::long::module::path::to::some::nested::ty::Type<a::b::C, d::e::F>,
            }
        });
    }
    stream
}
