//! The quadratic lookalike distance the tests check the crate against.

/// The optimal-string-alignment (restricted Damerau-Levenshtein) distance
/// between `a` and `b`, in Unicode scalar values. Quadratic, the
/// reference the linear `are_lookalikes` is checked against.
pub fn osa_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    // distances[i][j] is the distance between a[..i] and b[..j].
    let mut distances = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in distances.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in distances[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let substitution_cost = usize::from(a[i - 1] != b[j - 1]);
            let mut distance = (distances[i - 1][j] + 1)
                .min(distances[i][j - 1] + 1)
                .min(distances[i - 1][j - 1] + substitution_cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                distance = distance.min(distances[i - 2][j - 2] + 1);
            }
            distances[i][j] = distance;
        }
    }
    distances[a.len()][b.len()]
}
