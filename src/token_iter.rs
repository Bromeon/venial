use proc_macro2::{TokenStream, TokenTree};

/// Token iterator with O(1) checkpoint/rollback and arbitrary lookahead.
///
/// Collects the stream into a buffer once; all navigation is index-based.
/// Deliberately does NOT implement `Clone` — use `checkpoint()`/`rollback()`.
pub struct TokenIter {
    tokens: Vec<TokenTree>,
    pos: usize,
}

/// Opaque saved position, obtained from [`TokenIter::checkpoint`].
pub(crate) struct Checkpoint(usize);

impl TokenIter {
    /// Creates a token iterator from a token stream.
    pub fn new(stream: TokenStream) -> Self {
        Self {
            tokens: stream.into_iter().collect(),
            pos: 0,
        }
    }

    pub(crate) fn from_vec(tokens: Vec<TokenTree>) -> Self {
        Self { tokens, pos: 0 }
    }

    pub(crate) fn from_slice(slice: &[TokenTree]) -> Self {
        Self::from_vec(slice.to_vec())
    }

    /// Peek at the next token without consuming it.
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
