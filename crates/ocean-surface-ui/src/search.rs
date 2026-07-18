//! Shared fuzzy matching for keyboard-first Ocean search surfaces.

/// Score a case-insensitive subsequence match.
///
/// Matches at the start of the target, word boundaries, consecutive runs, and
/// prefixes receive bonuses. `None` means the query is not a subsequence.
pub(crate) fn fuzzy_score(query: &str, target: &str) -> Option<f64> {
    if query.is_empty() {
        return Some(0.0);
    }

    let query_lower = query.to_lowercase();
    let target_lower = target.to_lowercase();
    let query_chars: Vec<char> = query_lower.chars().collect();
    let target_chars: Vec<char> = target_lower.chars().collect();
    // Boundary detection needs the original case so camelCase transitions
    // survive normalization.
    let target_orig: Vec<char> = target.chars().collect();

    let mut score = 0.0;
    let mut qi = 0usize;
    let mut prev_match = None;
    let mut consecutive = 0usize;

    for (ti, tc) in target_chars.iter().enumerate() {
        if qi >= query_chars.len() {
            break;
        }
        if *tc != query_chars[qi] {
            continue;
        }

        let mut char_score = 1.0;
        if ti == 0 {
            char_score += 10.0;
        }
        if ti < target_orig.len() && is_word_boundary(&target_orig, ti) {
            char_score += 5.0;
        }
        if let Some(prev) = prev_match {
            if ti == prev + 1 {
                consecutive += 1;
                char_score += 2.0 * consecutive as f64;
            } else {
                consecutive = 0;
            }
        }
        if qi == ti {
            char_score += 3.0;
        }

        score += char_score;
        prev_match = Some(ti);
        qi += 1;
    }

    (qi == query_chars.len()).then_some(score)
}

/// Whether `chars[idx]` begins a word: index zero, after a separator, or at a
/// lower-to-upper camelCase transition.
pub(crate) fn is_word_boundary(chars: &[char], idx: usize) -> bool {
    if idx == 0 {
        return true;
    }
    let prev = chars[idx - 1];
    let curr = chars[idx];
    matches!(prev, ' ' | '-' | '_' | '/' | '.') || (prev.is_lowercase() && curr.is_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_scores_zero() {
        assert_eq!(fuzzy_score("", "anything"), Some(0.0));
    }

    #[test]
    fn exact_match_scores_positive() {
        assert!(fuzzy_score("files", "files").expect("match") > 0.0);
    }

    #[test]
    fn exact_prefix_scores_higher_than_word_boundary() {
        let prefix = fuzzy_score("fp", "FilesPanel").expect("match");
        let boundary = fuzzy_score("fp", "SomeFilePanel").expect("match");
        assert!(prefix > boundary);
    }

    #[test]
    fn word_boundary_scores_higher_than_scattered() {
        let boundary = fuzzy_score("fp", "SomeFilePanel").expect("match");
        let scattered = fuzzy_score("mp", "somefilepanel").expect("match");
        assert!(boundary > scattered);
    }

    #[test]
    fn scattered_subsequence_still_matches() {
        assert!(fuzzy_score("smp", "somefilepanel").is_some());
    }

    #[test]
    fn missing_char_returns_none() {
        assert_eq!(fuzzy_score("xyz", "abcdef"), None);
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert_eq!(fuzzy_score("files", "FILES"), fuzzy_score("FILES", "files"));
    }

    #[test]
    fn consecutive_bonus_compounds() {
        let consecutive = fuzzy_score("fil", "files").expect("match");
        let scattered = fuzzy_score("fls", "files").expect("match");
        assert!(consecutive > scattered);
    }

    #[test]
    fn word_boundaries_cover_separators_and_camelcase() {
        assert!(is_word_boundary(&['a', 'b'], 0));
        assert!(is_word_boundary(&['a', ' ', 'b'], 2));
        assert!(is_word_boundary(&['a', '-', 'b'], 2));
        assert!(is_word_boundary(&['a', '_', 'b'], 2));
        assert!(is_word_boundary(&['a', '/', 'b'], 2));
        assert!(is_word_boundary(&['a', '.', 'b'], 2));
        assert!(is_word_boundary(&['a', 'B'], 1));
        assert!(!is_word_boundary(&['a', 'b', 'c'], 1));
        assert!(!is_word_boundary(&['A', 'B', 'C'], 1));
    }
}
