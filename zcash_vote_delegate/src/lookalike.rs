//! The lookalike check for delegate handles (contract §11.7).
//!
//! Vizor compares a handle with its wallet list's handles and
//! `reserved_names`. Two handles are lookalikes when
//! their folds ([`fold_handle`]) are within optimal-string-alignment distance
//! 1. The fold is ASCII-only, which keeps every language's fold identical.

/// Folds `handle` for lookalike comparison, in this order:
///
/// 1. ASCII lowercase.
/// 2. Strip one leading `@`.
/// 3. Delete `_` and `-`.
/// 4. Map `0 → o`, `1 → l` and `i → l`.
/// 5. Replace `rn → m`, `vv → w` and `cl → d`, left to right without overlap.
///
/// The order matters: `r_n` folds to `m`, and `c1` and `ci` fold to `d`.
pub fn fold_handle(handle: &str) -> String {
    let lowercase = handle.to_ascii_lowercase();
    let unprefixed = lowercase.strip_prefix('@').unwrap_or(&lowercase);
    let mapped: Vec<char> = unprefixed
        .chars()
        .filter(|character| !matches!(character, '_' | '-'))
        .map(|character| match character {
            '0' => 'o',
            '1' | 'i' => 'l',
            other => other,
        })
        .collect();

    let mut folded = String::with_capacity(mapped.len());
    let mut position = 0;
    while position < mapped.len() {
        let digraph_replacement = match (mapped[position], mapped.get(position + 1)) {
            ('r', Some('n')) => Some('m'),
            ('v', Some('v')) => Some('w'),
            ('c', Some('l')) => Some('d'),
            _ => None,
        };
        match digraph_replacement {
            Some(replacement) => {
                folded.push(replacement);
                position += 2;
            }
            None => {
                folded.push(mapped[position]);
                position += 1;
            }
        }
    }
    folded
}

/// Whether `a` and `b` are lookalikes (see the module doc), in linear time.
/// A handle is its own lookalike, so callers skip the entry being checked.
pub fn are_lookalikes(a: &str, b: &str) -> bool {
    let a: Vec<char> = fold_handle(a).chars().collect();
    let b: Vec<char> = fold_handle(b).chars().collect();
    within_one_edit(&a, &b)
}

/// Whether `a` and `b` are within optimal-string-alignment distance 1.
fn within_one_edit(a: &[char], b: &[char]) -> bool {
    let (shorter, longer) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    if longer.len() - shorter.len() > 1 {
        return false;
    }
    let Some(first_difference) = shorter
        .iter()
        .zip(longer)
        .position(|(left, right)| left != right)
    else {
        // One is a prefix of the other, at most one character longer.
        return true;
    };
    if shorter.len() < longer.len() {
        return shorter[first_difference..] == longer[first_difference + 1..];
    }
    let after = first_difference + 1;
    let substituted = shorter[after..] == longer[after..];
    let transposed = after < shorter.len()
        && shorter[first_difference] == longer[after]
        && shorter[after] == longer[first_difference]
        && shorter[after + 1..] == longer[after + 1..];
    substituted || transposed
}
