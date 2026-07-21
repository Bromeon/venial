use crate::parse_type::{
    consume_field_type, consume_generic_params, consume_item_name, consume_lifetime,
    consume_where_clause,
};
use crate::parse_utils::{
    consume_any_ident, consume_comma, consume_ident, consume_outer_attributes, consume_punct,
    consume_stuff_until, parse_any_ident, parse_punct,
};
use crate::punctuated::Punctuated;
use crate::token_iter::TokenIter;
use crate::types::{
    FnParam, FnQualifiers, FnReceiverParam, FnTypedParam, Function, GroupSpan, TypeExpr,
};
use crate::{Attribute, Macro, VisMarker};
use proc_macro2::{Delimiter, Ident, Punct, TokenStream, TokenTree};

/// Which non-function item a declaration turned out to be.
///
/// The fn qualifier prefix is ambiguous: `const` also starts a constant, `unsafe` also starts a
/// trait/impl/mod, `extern` also starts an extern crate or extern block. When venial detects one
/// of those, it reports the item kind here so the caller can re-parse from the same position.
#[derive(Debug)]
pub(crate) enum NotFunction {
    Const,
    Static,
    Trait,
    Impl,
    Mod,
    ExternCrate,
    ExternBlock,
}

/// Error type of [`consume_fn`], which gives ownership of attributes and visibility marker back
/// to the caller, so they can be reused by fallback parsers.
pub(crate) struct NotFunctionError {
    pub kind: NotFunction,
    pub attributes: Vec<Attribute>,
    pub vis_marker: Option<VisMarker>,
}

/// Result of [`scan_fn_qualifiers`]: the token offsets of each qualifier that is present.
#[derive(Default)]
struct QualifierScan {
    /// Number of tokens the qualifier prefix spans; the `fn` keyword (or fallback token) is at
    /// this offset.
    len: usize,
    tk_default: Option<usize>,
    tk_const: Option<usize>,
    tk_async: Option<usize>,
    tk_unsafe: Option<usize>,
    tk_extern: Option<usize>,
    extern_abi: Option<usize>,
}

impl QualifierScan {
    /// Whether exactly either `const` or `unsafe` is set, and no other qualifier
    /// (so the tokens could be the start of a constant, trait, impl or mod declaration).
    fn has_only_const_xor_unsafe(&self) -> bool {
        (self.tk_const.is_some() ^ self.tk_unsafe.is_some())
            && self.tk_default.is_none()
            && self.tk_async.is_none()
            && self.tk_extern.is_none()
    }
}

/// Scans the fn qualifier prefix by lookahead, without consuming anything.
///
/// Single source of truth for the qualifier grammar: [`consume_fn_qualifiers`] uses it to build
/// [`FnQualifiers`], [`consume_fn`] uses it to decide fn-vs-fallback before consuming a token.
fn scan_fn_qualifiers(tokens: &TokenIter) -> QualifierScan {
    // Qualifiers appear in fixed order: `default const async unsafe extern "abi"`. `stage` tracks
    // how far we are; anything out of order ends the scan (and fails the `fn` keyword check at the
    // call site). Ident-to-string conversion is done once per token, since it allocates in
    // compiler mode.
    let mut scan = QualifierScan::default();
    let mut stage = 0;
    while let Some(TokenTree::Ident(ident)) = tokens.peek_n(scan.len) {
        let index = scan.len;
        match ident.to_string().as_str() {
            "default" if stage < 1 => {
                scan.tk_default = Some(index);
                stage = 1;
            }
            "const" if stage < 2 => {
                scan.tk_const = Some(index);
                stage = 2;
            }
            "async" if stage < 3 => {
                scan.tk_async = Some(index);
                stage = 3;
            }
            "unsafe" if stage < 4 => {
                scan.tk_unsafe = Some(index);
                stage = 4;
            }
            "extern" if stage < 5 => {
                scan.tk_extern = Some(index);
                stage = 5;
                if let Some(TokenTree::Literal(_)) = tokens.peek_n(index + 1) {
                    scan.extern_abi = Some(index + 1);
                    scan.len += 1;
                }
            }
            _ => break,
        }
        scan.len += 1;
    }

    scan
}

pub(crate) fn consume_fn_qualifiers(tokens: &mut TokenIter) -> FnQualifiers {
    fn ident_at(tokens: &TokenIter, index: Option<usize>) -> Option<Ident> {
        match tokens.peek_n(index?) {
            Some(TokenTree::Ident(ident)) => Some(ident.clone()),
            other => unreachable!("qualifier scan pointed at non-ident token {:?}", other),
        }
    }

    let scan = scan_fn_qualifiers(tokens);
    let qualifiers = FnQualifiers {
        tk_default: ident_at(tokens, scan.tk_default),
        tk_const: ident_at(tokens, scan.tk_const),
        tk_async: ident_at(tokens, scan.tk_async),
        tk_unsafe: ident_at(tokens, scan.tk_unsafe),
        tk_extern: ident_at(tokens, scan.tk_extern),
        extern_abi: match scan.extern_abi.and_then(|index| tokens.peek_n(index)) {
            Some(TokenTree::Literal(literal)) => Some(literal.clone()),
            Some(other) => unreachable!("qualifier scan pointed at non-literal token {:?}", other),
            None => None,
        },
    };

    for _ in 0..scan.len {
        tokens.next();
    }

    qualifiers
}

fn parse_fn_params(tokens: TokenStream) -> Punctuated<FnParam> {
    let mut fields = Punctuated::new();

    let mut tokens = TokenIter::new(tokens);
    loop {
        if tokens.peek().is_none() {
            break;
        }
        let attributes = consume_outer_attributes(&mut tokens);

        let tk_ref = consume_punct(&mut tokens, '&');
        let lifetime = consume_lifetime(&mut tokens, false);
        let tk_mut = consume_ident(&mut tokens, "mut");
        let tk_self = consume_ident(&mut tokens, "self");

        let param = if let Some(tk_self) = tk_self {
            FnParam::Receiver(FnReceiverParam {
                attributes,
                tk_ref,
                lifetime,
                tk_mut,
                tk_self,
            })
        } else {
            // TODO - handle non-ident argument names
            let param_name = parse_any_ident(&mut tokens, "fn param name");
            let tk_colon = parse_punct(&mut tokens, ':', "fn params");

            let ty_tokens = consume_field_type(&mut tokens);
            FnParam::Typed(FnTypedParam {
                attributes,
                tk_mut,
                name: param_name,
                tk_colon,
                ty: TypeExpr { tokens: ty_tokens },
            })
        };

        let comma = consume_comma(&mut tokens);

        fields.push(param, comma);
    }

    fields
}

fn consume_fn_return(tokens: &mut TokenIter) -> Option<([Punct; 2], TypeExpr)> {
    let dash = consume_punct(tokens, '-')?;

    let tip = match tokens.next() {
        Some(TokenTree::Punct(punct)) if punct.as_char() == '>' => punct,
        _ => panic!("cannot parse fn return: expected '>' after '-' token"),
    };

    Some((
        [dash, tip],
        TypeExpr {
            tokens: consume_stuff_until(
                tokens,
                |token| match token {
                    TokenTree::Group(group) if group.delimiter() == Delimiter::Brace => true,
                    TokenTree::Ident(i) if i == &Ident::new("where", i.span()) => true,
                    TokenTree::Punct(punct) if punct.as_char() == ';' => true,
                    _ => false,
                },
                true,
            ),
        },
    ))
}

/// Tries to parse a function definition.
///
/// Panics when the following tokens do not constitute a function definition, with one exception:
/// when the qualifier prefix is ambiguous (`const`, `unsafe`, `extern`) and is followed by a
/// keyword starting another item kind, then `Err` is returned. Nothing is consumed in that case,
/// so the caller can re-parse the same tokens as that item kind. The `Err` value also hands
/// `attributes` and `vis_marker` back, to avoid cloning them on the (common) success path.
pub(crate) fn consume_fn(
    tokens: &mut TokenIter,
    attributes: Vec<Attribute>,
    vis_marker: Option<VisMarker>,
) -> Result<Function, NotFunctionError> {
    // Scan the variable-length qualifier prefix (`default const async unsafe extern "abi"`)
    // via lookahead, so fn vs. fallback is decided before consuming any token.
    let scan = scan_fn_qualifiers(tokens);
    let has_extern = scan.tk_extern.is_some();

    // fn keyword, or fallback to another item kind
    let fallback_kind = match tokens.peek_n(scan.len) {
        Some(TokenTree::Ident(ident)) => {
            let ident_str = ident.to_string();
            if ident_str == "fn" {
                None
            } else if has_extern && ident_str == "crate" {
                Some(NotFunction::ExternCrate)
            } else if ident_str == "static" {
                Some(NotFunction::Static)
            } else if scan.has_only_const_xor_unsafe() {
                // This is not a function, detect what else it is.
                if scan.tk_const.is_some() {
                    Some(NotFunction::Const)
                } else {
                    match ident_str.as_str() {
                        "trait" => Some(NotFunction::Trait),
                        "impl" => Some(NotFunction::Impl),
                        "mod" => Some(NotFunction::Mod),
                        _ => panic!(
                            "expected one of 'fn|trait|impl|mod' after 'unsafe', got {ident:?}"
                        ),
                    }
                }
            } else {
                panic!("expected 'fn' keyword, got ident '{}'", ident)
            }
        }

        // extern { ... or extern "C" { ... (the ABI literal, if any, is part of the scan)
        Some(TokenTree::Group(group)) if has_extern && group.delimiter() == Delimiter::Brace => {
            Some(NotFunction::ExternBlock)
        }

        _ => {
            panic!("expected 'fn' keyword")
        }
    };

    if let Some(kind) = fallback_kind {
        // Nothing was consumed, caller can parse the item as something else.
        return Err(NotFunctionError {
            kind,
            attributes,
            vis_marker,
        });
    }

    let qualifiers = consume_fn_qualifiers(tokens);
    let tk_fn_keyword = match tokens.next() {
        Some(TokenTree::Ident(ident)) => ident,
        _ => unreachable!("checked by qualifier lookahead"),
    };

    let fn_name = consume_item_name(tokens);
    let generic_params = consume_generic_params(tokens);

    let (params, tk_params_parens) = match tokens.next().unwrap() {
        TokenTree::Group(group) if group.delimiter() == Delimiter::Parenthesis => {
            (parse_fn_params(group.stream()), GroupSpan::new(&group))
        }
        _ => panic!("cannot parse function; missing parameter list"),
    };

    let (tk_return_arrow, return_ty) = if let Some((arrow, ty)) = consume_fn_return(tokens) {
        (Some(arrow), Some(ty))
    } else {
        (None, None)
    };

    let where_clause = consume_where_clause(tokens);

    let (function_body, tk_semicolon) = match &tokens.next().unwrap() {
        TokenTree::Group(group) if group.delimiter() == Delimiter::Brace => {
            (Some(group.clone()), None)
        }
        TokenTree::Punct(punct) if punct.as_char() == ';' => (None, Some(punct.clone())),
        _ => panic!("cannot parse function; missing body or `;`"),
    };

    Ok(Function {
        attributes,
        vis_marker,
        qualifiers,
        tk_fn_keyword,
        name: fn_name,
        generic_params,
        tk_params_parens,
        params,
        where_clause,
        tk_return_arrow,
        return_ty,
        tk_semicolon,
        body: function_body,
    })
}

/// On failure, gives ownership of `attributes` back to the caller via the `Err` variant.
pub(crate) fn consume_macro(
    tokens: &mut TokenIter,
    attributes: Vec<Attribute>,
) -> Result<Macro, Vec<Attribute>> {
    // `ident !` starts a macro invocation; anything else could be the start of another item.
    let is_macro = matches!(tokens.peek_n(0), Some(TokenTree::Ident(_)))
        && matches!(tokens.peek_n(1), Some(TokenTree::Punct(punct)) if punct.as_char() == '!');
    if !is_macro {
        return Err(attributes);
    }

    let name = parse_any_ident(tokens, "macro name");
    let tk_bang = parse_punct(tokens, '!', "macro invocation");
    let tk_declared_name = consume_any_ident(tokens);

    let (is_paren, macro_body) = match tokens.next().expect("unexpected end of macro") {
        TokenTree::Group(group) if group.delimiter() == Delimiter::Parenthesis => (true, group),
        TokenTree::Group(group) if group.delimiter() == Delimiter::Brace => (false, group),
        _ => panic!("cannot parse macro; missing `{{}}` or `()` group"),
    };

    let inner_tokens = macro_body.stream().into_iter().collect();

    let tk_semicolon = if is_paren {
        Some(parse_punct(tokens, ';', "macro invocation semicolon"))
    } else {
        None
    };

    Ok(Macro {
        attributes,
        name,
        tk_bang,
        tk_declared_name,
        tk_braces_or_parens: GroupSpan::new(&macro_body),
        inner_tokens,
        tk_semicolon,
    })
}
