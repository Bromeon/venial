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

/// If venial fails to parse the declaration as a function, it can detect that it
/// is either a constant (`const` ambiguity), an impl or a module (`unsafe` ambiguity).

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

pub(crate) fn consume_fn_qualifiers(tokens: &mut TokenIter) -> FnQualifiers {
    let mut tk_default = None;
    let mut tk_const = None;
    let mut tk_async = None;
    let mut tk_unsafe = None;

    // Qualifiers appear in fixed order: `default const async unsafe extern`. `stage` tracks how
    // far we are; anything out of order ends the loop (and fails the `fn` keyword check later).
    // Ident-to-string conversion is done once per token, since it allocates in compiler mode.
    let mut stage = 0;
    while let Some(TokenTree::Ident(ident)) = tokens.peek() {
        match ident.to_string().as_str() {
            "default" if stage < 1 => {
                tk_default = Some(ident.clone());
                stage = 1;
            }
            "const" if stage < 2 => {
                tk_const = Some(ident.clone());
                stage = 2;
            }
            "async" if stage < 3 => {
                tk_async = Some(ident.clone());
                stage = 3;
            }
            "unsafe" if stage < 4 => {
                tk_unsafe = Some(ident.clone());
                stage = 4;
            }
            _ => break,
        }
        tokens.next();
    }

    let tk_extern;
    let extern_abi;
    match tokens.peek() {
        Some(TokenTree::Ident(ident)) if ident == "extern" => {
            tk_extern = Some(ident.clone());
            tokens.next();

            match tokens.peek() {
                Some(TokenTree::Literal(literal)) => {
                    extern_abi = Some(literal.clone());
                    tokens.next();
                }
                _ => {
                    extern_abi = None;
                }
            }
        }
        _ => {
            tk_extern = None;
            extern_abi = None;
        }
    };

    FnQualifiers {
        tk_default,
        tk_const,
        tk_async,
        tk_unsafe,
        tk_extern,
        extern_abi,
    }
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
/// when `const` is followed by an identifier which is not `fn`, then `None` is returned. This is to
/// allow fallback to a constant declaration (both can begin with the `const` token).
pub(crate) fn consume_fn(
    tokens: &mut TokenIter,
    attributes: Vec<Attribute>,
    vis_marker: Option<VisMarker>,
) -> Result<Function, NotFunctionError> {
    // Scan the variable-length qualifier prefix (`default const async unsafe extern "abi"`)
    // via lookahead, so fn vs. fallback is decided before consuming any token.
    let mut n = 0;
    let mut stage = 0;
    let mut has_const = false;
    let mut has_unsafe = false;
    let mut has_extern = false;
    let mut has_other_qualifier = false;
    while let Some(TokenTree::Ident(ident)) = tokens.peek_n(n) {
        match ident.to_string().as_str() {
            "default" if stage < 1 => {
                has_other_qualifier = true;
                stage = 1;
            }
            "const" if stage < 2 => {
                has_const = true;
                stage = 2;
            }
            "async" if stage < 3 => {
                has_other_qualifier = true;
                stage = 3;
            }
            "unsafe" if stage < 4 => {
                has_unsafe = true;
                stage = 4;
            }
            "extern" if stage < 5 => {
                has_extern = true;
                stage = 5;
                if let Some(TokenTree::Literal(_)) = tokens.peek_n(n + 1) {
                    n += 1; // ABI literal
                }
            }
            _ => break,
        }
        n += 1;
    }
    let const_xor_unsafe_only = (has_const ^ has_unsafe) && !has_other_qualifier && !has_extern;

    // fn keyword, or fallback to another item kind
    let fallback_kind = match tokens.peek_n(n) {
        Some(TokenTree::Ident(ident)) => {
            let ident_str = ident.to_string();
            if ident_str == "fn" {
                None
            } else if has_extern && ident_str == "crate" {
                Some(NotFunction::ExternCrate)
            } else if ident_str == "static" {
                Some(NotFunction::Static)
            } else if const_xor_unsafe_only {
                // This is not a function, detect what else it is.
                if has_const {
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

        // extern "C" { ...
        Some(TokenTree::Literal(_)) if has_extern => Some(NotFunction::ExternBlock),

        // extern { ...
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

    let mut macro_ = consume_macro_inner(tokens).expect("matched by lookahead");
    macro_.attributes = attributes;
    Ok(macro_)
}

fn consume_macro_inner(tokens: &mut TokenIter) -> Option<Macro> {
    let name = consume_any_ident(tokens)?;
    let tk_bang = consume_punct(tokens, '!')?;
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

    Some(Macro {
        attributes: Vec::new(), // filled in by consume_macro
        name,
        tk_bang,
        tk_declared_name,
        tk_braces_or_parens: GroupSpan::new(&macro_body),
        inner_tokens,
        tk_semicolon,
    })
}
