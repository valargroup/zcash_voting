//! The optimal-string-alignment check (§11.7). The vectors pin the fold and
//! the verdict of each pair.

use zcash_vote_delegate::{are_lookalikes, fold_handle, osa_distance};

/// Every string over `alphabet` of length at most `max_len`.
fn strings(alphabet: &[char], max_len: usize) -> Vec<String> {
    let mut all = vec![String::new()];
    let mut frontier = vec![String::new()];
    for _ in 0..max_len {
        frontier = frontier
            .iter()
            .flat_map(|prefix| {
                alphabet.iter().map(move |letter| {
                    let mut longer = prefix.clone();
                    longer.push(*letter);
                    longer
                })
            })
            .collect();
        all.extend(frontier.iter().cloned());
    }
    all
}

/// The linear lookalike check agrees with the quadratic distance on every
/// pair of short strings, where the fold changes nothing.
#[test]
fn lookalike_check_agrees_with_the_distance() {
    let corpus = strings(&['a', 'b', 'x'], 4);
    for a in &corpus {
        assert_eq!(fold_handle(a), *a);
        for b in &corpus {
            assert_eq!(
                are_lookalikes(a, b),
                osa_distance(a, b) <= 1,
                "{a:?} and {b:?}"
            );
        }
    }
}

/// The check stays linear on long input from an untrusted directory.
#[test]
fn lookalike_check_is_linear_in_the_input() {
    let long = "a".repeat(200_000);
    assert!(are_lookalikes(&long, &format!("{long}b")));
    assert!(!are_lookalikes(&long, &format!("b{long}b")));
}
