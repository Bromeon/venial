use std::collections::VecDeque;

pub(crate) fn multipeek<I: Iterator>(iter: I) -> Multipeek<I> {
    Multipeek::new(iter)
}

#[derive(Clone, Debug)]
pub(crate) struct Multipeek<I: Iterator> {
    iter: I,

    // Possible optimization: a fixed [Option<Item>; N] ring buffer, when N is fixed
    buffer: VecDeque<I::Item>,
}

impl<I: Iterator> Multipeek<I> {
    fn new(iter: I) -> Self {
        Self {
            iter,
            buffer: VecDeque::new(),
        }
    }

    pub fn peek(&mut self) -> Option<&I::Item> {
        self.peek_nth(0)
    }

    pub fn peek_nth(&mut self, ahead: usize) -> Option<&I::Item> {
        // Still in cache: return nth element
        if ahead < self.buffer.len() {
            return Some(&self.buffer[ahead]);
        }

        // Exceeds cached lookahead: advance queue
        let advance_by = ahead - self.buffer.len() + 1;
        for _ in 0..advance_by {
            if let Some(item) = self.iter.next() {
                self.buffer.push_back(item);
            } else {
                // end of stream
                return None;
            }
        }

        Some(&self.buffer[ahead])
    }
}

impl<I: Iterator> Iterator for Multipeek<I> {
    type Item = I::Item;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(item) = self.buffer.pop_front() {
            // Still in cache: return next element
            Some(item)
        } else {
            // Iterate normally (do not extend cache)
            self.iter.next()
        }
    }

    // TODO size_hint etc
}
