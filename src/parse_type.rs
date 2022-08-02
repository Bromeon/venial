use crate::error::Error;
use crate::parse_utils::{
    consume_attributes, consume_comma, consume_stuff_until, consume_vis_marker, parse_ident,
    try_consume_colon2,
};
use crate::punctuated::Punctuated;
use crate::types::{
    EnumVariant, EnumVariantValue, GenericArg, GenericArgList, GenericBound, GenericParam,
    GenericParamList, NamedField, NamedStructFields, StructFields, TupleField, TupleStructFields,
    TyExpr, WhereClause, WhereClauseItem,
};
use crate::types_edition::GroupSpan;
use proc_macro2::{Delimiter, Group, Ident, Punct, TokenStream, TokenTree};
use std::iter::Peekable;

type TokenIter = Peekable<proc_macro2::token_stream::IntoIter>;

pub(crate) fn consume_declaration_name(tokens: &mut TokenIter) -> Ident {
    let token = tokens
        .next()
        .expect("cannot parse declaration: expected identifier, found end-of-stream");
    parse_ident(token).unwrap_or_else(|token| {
        panic!(
            "cannot parse declaration: expected identifier, found token {:?}",
            token
        );
    })
}

pub(crate) fn consume_generic_params(tokens: &mut TokenIter) -> Option<GenericParamList> {
    let gt: Punct;
    let mut generic_params = Punctuated::new();
    let lt: Punct;

    match tokens.peek() {
        Some(TokenTree::Punct(punct)) if punct.as_char() == '<' => {
            gt = punct.clone();
        }
        _ => return None,
    };

    // consume '<'
    tokens.next();

    loop {
        let token = tokens
            .peek()
            .expect("cannot parse generic params: expected token after '<'");
        let prefix = match token {
            TokenTree::Punct(punct) if punct.as_char() == '>' => {
                lt = punct.clone();
                break;
            }
            TokenTree::Punct(punct) if punct.as_char() == '\'' => Some(tokens.next().expect("generic_param '")),
            TokenTree::Ident(ident) if ident == "const" => Some(tokens.next().expect("generic_param const")),
            TokenTree::Ident(_ident) => None,
            token => {
                panic!("cannot parse generic params: unexpected token {:?}", token)
            }
        };

        let name = parse_ident(tokens.next().expect("generic_param 1")).expect("generic_param 2");

        let bound = match tokens.peek().expect("generic_param 3") {
            TokenTree::Punct(punct) if punct.as_char() == ':' => {
                let colon = punct.clone();
                // consume ':'
                tokens.next();

                let bound_tokens = consume_stuff_until(
                    tokens,
                    |token| match token {
                        TokenTree::Punct(punct) if punct.as_char() == ',' => true,
                        _ => false,
                    },
                    false,
                );

                Some(GenericBound {
                    tk_colon: colon,
                    tokens: bound_tokens,
                })
            }
            TokenTree::Punct(punct) if punct.as_char() == ',' => None,
            TokenTree::Punct(punct) if punct.as_char() == '>' => None,
            token => {
                panic!("cannot parse generic params: unexpected token {:?}", token)
            }
        };

        let comma = consume_comma(tokens);

        generic_params.push(
            GenericParam {
                tk_prefix: prefix,
                name,
                bound,
            },
            comma,
        );
    }

    // consume '>'
    tokens.next();

    Some(GenericParamList {
        tk_l_bracket: gt,
        params: generic_params,
        tk_r_bracket: lt,
    })
}

fn consume_generic_arg(tokens: Vec<TokenTree>) -> GenericArg {
    // Note: method not called if tokens is empty
    let mut tokens = tokens.into_iter().peekable();

    // Try parsing 'lifetime
    if let TokenTree::Punct(punct) = tokens.peek().expect("generic_arg 1") {
        if punct.as_char() == '\'' {
            let tk_lifetime = punct.clone();
            tokens.next(); // consume '

            // after the ', there must be a single identifier
            match tokens.next() {
                Some(TokenTree::Ident(ident)) => {
                    assert!(
                        tokens.next().is_none(),
                        "cannot parse lifetime generic argument"
                    );

                    return GenericArg::Lifetime { tk_lifetime, ident };
                }
                Some(other) => {
                    panic!(
                        "expected identifier after ' lifetime symbol, got {:?}",
                        other
                    );
                }
                None => {
                    panic!("expected identifier after ' lifetime symbol, but ran out of tokens")
                }
            }
        }
    }

    // Then, try parsing Item = ...
    // (there is at least 1 token, so unwrap is safe)
    let before_ident = tokens.clone();
    if let TokenTree::Ident(ident) = tokens.next().expect("generic_arg 2") {
        if let Some(TokenTree::Punct(punct)) = tokens.next() {
            if punct.as_char() == '=' {
                let remaining: Vec<TokenTree> = tokens.collect();

                return GenericArg::Binding {
                    ident,
                    tk_equals: punct,
                    ty: TyExpr { tokens: remaining },
                };
            }
        }
    }

    // Last, all the rest is just tokens
    let remaining: Vec<TokenTree> = before_ident.collect();

    GenericArg::TyOrConst {
        expr: TyExpr { tokens: remaining },
    }
}

pub(crate) fn consume_generic_args(tokens: &mut TokenIter) -> Option<GenericArgList> {
    let before = tokens.clone();
    let tk_turbofish_colons = try_consume_colon2(tokens);

    let tk_l_bracket = match tokens.peek() {
        Some(TokenTree::Punct(punct)) if punct.as_char() == '<' => {
            let gt = punct.clone();
            tokens.next();
            gt
        }
        _ => {
            *tokens = before;
            return None;
        }
    };

    let mut generic_args = Punctuated::new();
    loop {
        // Tokenize until next comma (skips nested <>)
        let arg_tokens = consume_stuff_until(
            tokens,
            |tk| matches!(tk, TokenTree::Punct(punct) if punct.as_char() == ','),
            false,
        );
        let comma = consume_comma(tokens);

        // Exit when end reached
        if arg_tokens.is_empty() {
            break;
        }

        generic_args.push(consume_generic_arg(arg_tokens), comma);
    }

    let tk_r_bracket = match tokens.peek() {
        Some(TokenTree::Punct(punct)) if punct.as_char() == '>' => {
            let lt = punct.clone();
            tokens.next();
            lt
        }
        _ => panic!("generic argument list must end with '>'"),
    };

    Some(GenericArgList {
        tk_turbofish_colons,
        tk_l_bracket,
        args: generic_args,
        tk_r_bracket,
    })
}

pub(crate) fn consume_where_clause(tokens: &mut TokenIter) -> Option<WhereClause> {
    let where_token: Ident;
    match tokens.peek() {
        Some(TokenTree::Ident(ident)) if ident == "where" => {
            where_token = ident.clone();
        }
        _ => return None,
    }
    tokens.next();

    let mut items = Punctuated::new();
    loop {
        let token = tokens
            .peek()
            .expect("cannot parse where clause: expected tokens");
        match token {
            TokenTree::Group(group) if group.delimiter() == Delimiter::Brace => break,
            TokenTree::Punct(punct) if punct.as_char() == ';' => break,
            _ => (),
        };

        let left_side = consume_stuff_until(
            tokens,
            |token| match token {
                TokenTree::Punct(punct) if punct.as_char() == ':' => true,
                _ => false,
            },
            false,
        );

        let colon = match tokens.next() {
            Some(TokenTree::Punct(punct)) if punct.as_char() == ':' => punct.clone(),
            Some(token) => panic!(
                "cannot parse where clause: expected ':', found token {:?}",
                token
            ),
            None => {
                panic!("cannot parse where clause: expected colon, found end of stream")
            }
        };
        let bound_tokens = consume_stuff_until(
            tokens,
            |token| match token {
                TokenTree::Punct(punct) if punct.as_char() == ',' => true,
                TokenTree::Group(group) if group.delimiter() == Delimiter::Brace => true,
                TokenTree::Punct(punct) if punct.as_char() == ';' => true,
                _ => false,
            },
            true,
        );

        let comma = consume_comma(tokens);

        items.push(
            WhereClauseItem {
                left_side,
                bound: GenericBound {
                    tk_colon: colon,
                    tokens: bound_tokens,
                },
            },
            comma,
        );
    }

    Some(WhereClause {
        tk_where: where_token,
        items,
    })
}

pub(crate) fn consume_field_type(tokens: &mut TokenIter) -> Vec<TokenTree> {
    let field_type_tokens = consume_stuff_until(
        tokens,
        |token| match token {
            TokenTree::Punct(punct) if punct.as_char() == ',' => true,
            _ => false,
        },
        false,
    );

    if field_type_tokens.is_empty() && consume_comma(tokens).is_some() {
        panic!("cannot parse type: unexpected token ','");
    } else if field_type_tokens.is_empty() {
        panic!("cannot parse type: expected tokens, found end-of-stream");
    }

    field_type_tokens
}

pub(crate) fn consume_enum_discriminant(
    tokens: &mut TokenIter,
) -> Result<Option<EnumVariantValue>, Error> {
    let equal: Punct;
    match tokens.peek() {
        Some(TokenTree::Punct(punct)) if punct.as_char() == '=' => {
            equal = punct.clone();
        }
        _ => return Ok(None),
    };

    // consume '='
    tokens.next();

    let value_token = tokens.next().expect("consume_enum_discriminant");

    // If the value expression has more than one token, we output an error.
    match tokens.peek() {
        None => (),
        Some(TokenTree::Punct(punct)) if punct.as_char() == ',' => (),
        Some(_token) => return Err(Error::new("Complex values for enum variants are not supported unless they are between parentheses.")),
    }

    Ok(Some(EnumVariantValue {
        tk_equal: equal,
        value: value_token,
    }))
}

pub(crate) fn parse_tuple_fields(token_group: Group) -> TupleStructFields {
    let mut fields = Punctuated::new();

    let mut tokens = token_group.stream().into_iter().peekable();
    loop {
        if tokens.peek().is_none() {
            break;
        }

        let attributes = consume_attributes(&mut tokens);
        let vis_marker = consume_vis_marker(&mut tokens);

        let ty_tokens = consume_field_type(&mut tokens);

        let comma = consume_comma(&mut tokens);

        fields.push(
            TupleField {
                attributes,
                vis_marker,
                ty: TyExpr { tokens: ty_tokens },
            },
            comma,
        );
    }

    TupleStructFields {
        fields,
        tk_parens: GroupSpan::new(&token_group),
    }
}

pub(crate) fn parse_named_fields(token_group: Group) -> NamedStructFields {
    let mut fields = Punctuated::new();

    let mut tokens = token_group.stream().into_iter().peekable();
    loop {
        if tokens.peek().is_none() {
            break;
        }

        let attributes = consume_attributes(&mut tokens);
        let vis_marker = consume_vis_marker(&mut tokens);

        let ident = parse_ident(tokens.next().expect("parse_named_fields 1")).expect("parse_named_fields 2");

        let colon = match tokens.next() {
            Some(TokenTree::Punct(punct)) if punct.as_char() == ':' => punct,
            token => panic!(
                "cannot parse named fields: expected ':', found token {:?}",
                token
            ),
        };

        let ty_tokens = consume_field_type(&mut tokens);
        let comma = consume_comma(&mut tokens);

        fields.push(
            NamedField {
                attributes,
                vis_marker,
                name: ident,
                tk_colon: colon,
                ty: TyExpr { tokens: ty_tokens },
            },
            comma,
        );
    }

    NamedStructFields {
        fields,
        tk_braces: GroupSpan::new(&token_group),
    }
}

pub(crate) fn parse_enum_variants(tokens: TokenStream) -> Result<Punctuated<EnumVariant>, Error> {
    let mut variants = Punctuated::new();

    let mut tokens = tokens.into_iter().peekable();
    loop {
        if tokens.peek().is_none() {
            break;
        }

        let attributes = consume_attributes(&mut tokens);
        let vis_marker = consume_vis_marker(&mut tokens);

        let ident = parse_ident(tokens.next().expect("parse_enum_variants 1")).expect("parse_enum_variants 2");

        let contents = match tokens.peek() {
            None => StructFields::Unit,
            Some(TokenTree::Punct(punct)) if punct.as_char() == ',' => StructFields::Unit,
            Some(TokenTree::Punct(punct)) if punct.as_char() == '=' => StructFields::Unit,
            Some(TokenTree::Group(group)) if group.delimiter() == Delimiter::Parenthesis => {
                let group = group.clone();
                // Consume group
                tokens.next();
                StructFields::Tuple(parse_tuple_fields(group))
            }
            Some(TokenTree::Group(group)) if group.delimiter() == Delimiter::Brace => {
                let group = group.clone();
                // Consume group
                tokens.next();
                StructFields::Named(parse_named_fields(group))
            }
            token => panic!("cannot parse enum variant: unexpected token {:?}", token),
        };

        let enum_discriminant = consume_enum_discriminant(&mut tokens);

        let comma = consume_comma(&mut tokens);

        variants.push(
            EnumVariant {
                attributes,
                vis_marker,
                name: ident,
                contents,
                value: enum_discriminant?,
            },
            comma,
        );
    }

    Ok(variants)
}
