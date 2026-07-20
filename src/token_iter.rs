use proc_macro2::{TokenStream, TokenTree};

/// Token iterator with arbitrary lookahead.
///
/// Buffers the stream once; `peek_n` reads ahead without consuming, so parsing code can
/// decide between alternatives before consuming anything. `next` moves tokens out without
/// cloning. Deliberately does NOT implement `Clone` — parsers must use lookahead instead of
/// backtracking.
pub struct TokenIter {
    inner: std::vec::IntoIter<TokenTree>,
}

impl TokenIter {
    /// Creates a token iterator from a token stream.
    pub fn new(stream: TokenStream) -> Self {
        Self::from_vec(stream.into_iter().collect())
    }

    pub(crate) fn from_vec(tokens: Vec<TokenTree>) -> Self {
        Self {
            inner: tokens.into_iter(),
        }
    }

    pub(crate) fn from_slice(slice: &[TokenTree]) -> Self {
        Self::from_vec(slice.to_vec())
    }

    /// Peek at the next token without consuming it.
    pub fn peek(&self) -> Option<&TokenTree> {
        self.inner.as_slice().first()
    }

    /// Peek `n` tokens ahead; `peek_n(0) == peek()`.
    pub(crate) fn peek_n(&self, n: usize) -> Option<&TokenTree> {
        self.inner.as_slice().get(n)
    }
}

impl Iterator for TokenIter {
    type Item = TokenTree;

    fn next(&mut self) -> Option<TokenTree> {
        self.inner.next()
    }
}

impl From<TokenStream> for TokenIter {
    fn from(stream: TokenStream) -> Self {
        Self::new(stream)
    }
}
