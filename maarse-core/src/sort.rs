//! A small sort. The standard library's sort adds 1-2 KB of firmware for
//! every type it is used with, and the nRF52840's app area is nearly full.
//! Shell sort is a few dozen bytes per use and fast enough for the few
//! thousand elements the firmware sorts at most. Not stable.

/// Sorts `s` so that no element is `less` than the one before it.
pub fn sort_by<T>(s: &mut [T], mut less: impl FnMut(&T, &T) -> bool) {
    for gap in [701, 301, 132, 57, 23, 10, 4, 1] {
        for i in gap..s.len() {
            let mut j = i;
            while j >= gap && less(&s[j], &s[j - gap]) {
                s.swap(j, j - gap);
                j -= gap;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_like_the_standard_library() {
        let mut seed = 7u32;
        for len in [0, 1, 2, 5, 23, 58, 800, 3000] {
            let mut v: Vec<u32> = (0..len)
                .map(|_| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    seed % 500
                })
                .collect();
            let mut want = v.clone();
            want.sort();
            sort_by(&mut v, |a, b| a < b);
            assert_eq!(v, want, "{len}");
            want.reverse();
            sort_by(&mut v, |a, b| a > b);
            assert_eq!(v, want, "{len} downwards");
        }
    }
}
