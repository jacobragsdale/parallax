//! A growable, mutable bit set, used for validity and liveness.

/// A bit per row, packed into 64-bit words.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bitmap {
    words: Vec<u64>,
    len: usize,
}

impl Bitmap {
    /// Returns an empty bitmap.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            words: Vec::new(),
            len: 0,
        }
    }

    /// Returns the number of bits.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if there are no bits.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Appends one bit.
    pub fn push(&mut self, value: bool) {
        if self.len.is_multiple_of(64) {
            self.words.push(0);
        }
        self.len += 1;
        self.set(self.len - 1, value);
    }

    /// Returns bit `index`.
    ///
    /// # Panics
    ///
    /// Panics if `index` is out of bounds.
    #[must_use]
    pub fn get(&self, index: usize) -> bool {
        assert!(index < self.len, "bit {index} of {}", self.len);
        self.words[index / 64] >> (index % 64) & 1 == 1
    }

    /// Sets bit `index`.
    ///
    /// # Panics
    ///
    /// Panics if `index` is out of bounds.
    pub fn set(&mut self, index: usize, value: bool) {
        assert!(index < self.len, "bit {index} of {}", self.len);
        let mask = 1u64 << (index % 64);
        if value {
            self.words[index / 64] |= mask;
        } else {
            self.words[index / 64] &= !mask;
        }
    }

    /// Returns the number of set bits.
    #[must_use]
    pub fn count_ones(&self) -> usize {
        self.words.iter().map(|w| w.count_ones() as usize).sum()
    }

    /// Iterates over the indices of set bits, in order.
    pub fn ones(&self) -> impl Iterator<Item = usize> + '_ {
        self.words.iter().enumerate().flat_map(|(w, &word)| {
            let mut bits = word;
            std::iter::from_fn(move || {
                if bits == 0 {
                    return None;
                }
                let bit = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                Some(w * 64 + bit)
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_get_set_and_count() {
        let mut b = Bitmap::new();
        for i in 0..200 {
            b.push(i % 3 == 0);
        }
        assert_eq!(b.len(), 200);
        assert!(b.get(0) && !b.get(1) && b.get(198));
        b.set(1, true);
        b.set(198, false);
        assert_eq!(b.count_ones(), 67);
        let ones: Vec<usize> = b.ones().collect();
        assert_eq!(ones.len(), 67);
        assert_eq!(&ones[..3], &[0, 1, 3]);
    }
}
