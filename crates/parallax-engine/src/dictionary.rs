//! String dictionaries with a deterministic sort order.
//!
//! Text sorts case-insensitively, with byte order breaking ties, and with no locale
//! involved: the order is the same on every machine. Each code has a rank in that
//! order, so sorting and grouping compare integers.

use std::cmp::Ordering;

use rustc_hash::FxHashMap;

/// Interned strings for one column.
#[derive(Clone, Debug, Default)]
pub struct Dictionary {
    values: Vec<Box<str>>,
    folded: Vec<Box<str>>,
    lookup: FxHashMap<Box<str>, u32>,
    ranks: Vec<u32>,
    ranks_stale: bool,
}

/// Compares two strings in the engine's text order.
#[must_use]
pub fn collate(a: &str, b: &str) -> Ordering {
    let fold = |s: &str| s.to_lowercase();
    fold(a).cmp(&fold(b)).then_with(|| a.cmp(b))
}

impl Dictionary {
    /// Returns an empty dictionary.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the number of distinct strings.
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Returns `true` if no string has been interned.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Returns the code for `value`, interning it if needed.
    ///
    /// # Panics
    ///
    /// Panics if more than `u32::MAX` distinct strings are interned.
    pub fn intern(&mut self, value: &str) -> u32 {
        if let Some(&code) = self.lookup.get(value) {
            return code;
        }
        let code = u32::try_from(self.values.len()).expect("fewer than 2^32 distinct strings");
        self.values.push(value.into());
        self.folded.push(value.to_lowercase().into());
        self.lookup.insert(value.into(), code);
        self.ranks_stale = true;
        code
    }

    /// Returns the code for `value` without interning it.
    #[must_use]
    pub fn code(&self, value: &str) -> Option<u32> {
        self.lookup.get(value).copied()
    }

    /// Returns the string for `code`.
    #[must_use]
    pub fn get(&self, code: u32) -> &str {
        &self.values[code as usize]
    }

    /// Recomputes ranks after new strings were interned.
    pub fn refresh_ranks(&mut self) {
        if !self.ranks_stale && self.ranks.len() == self.values.len() {
            return;
        }
        let mut order: Vec<u32> = (0..self.values.len() as u32).collect();
        order.sort_unstable_by(|&a, &b| {
            self.folded[a as usize]
                .cmp(&self.folded[b as usize])
                .then_with(|| self.values[a as usize].cmp(&self.values[b as usize]))
        });
        self.ranks = vec![0; self.values.len()];
        for (rank, &code) in order.iter().enumerate() {
            self.ranks[code as usize] = rank as u32;
        }
        self.ranks_stale = false;
    }

    /// Returns the position of `code` in text order.
    ///
    /// # Panics
    ///
    /// Panics if ranks are stale; call [`Dictionary::refresh_ranks`] after interning.
    #[must_use]
    pub fn rank(&self, code: u32) -> u32 {
        assert!(!self.ranks_stale, "dictionary ranks are stale");
        self.ranks[code as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_follow_case_insensitive_order_with_byte_tie_breaks() {
        let mut d = Dictionary::new();
        let words = [
            "beta",
            "Alpha",
            "alpha",
            "ALPHA",
            "Gamma",
            "",
            "e\u{301}cole",
            "\u{e9}cole",
        ];
        let codes: Vec<u32> = words.iter().map(|w| d.intern(w)).collect();
        d.refresh_ranks();
        let mut sorted: Vec<&str> = words.to_vec();
        sorted.sort_by(|a, b| collate(a, b));
        let mut by_rank: Vec<(u32, &str)> = codes.iter().map(|&c| (d.rank(c), d.get(c))).collect();
        by_rank.sort_unstable();
        let ranked: Vec<&str> = by_rank.into_iter().map(|(_, s)| s).collect();
        assert_eq!(ranked, sorted);
        assert_eq!(&sorted[..4], &["", "ALPHA", "Alpha", "alpha"]);
    }

    #[test]
    fn interning_is_idempotent_and_distinguishes_lookalikes() {
        let mut d = Dictionary::new();
        let a = d.intern("e\u{301}cole");
        let b = d.intern("\u{e9}cole");
        assert_ne!(
            a, b,
            "combining and precomposed forms are different strings"
        );
        assert_eq!(d.intern("e\u{301}cole"), a);
        assert_eq!(d.code("NULL"), None);
        assert_eq!(d.len(), 2);
    }
}
