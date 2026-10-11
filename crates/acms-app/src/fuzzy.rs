//! Fuzzy matching for the command palette.

/// Scores how well `query` matches `text`; higher is better, `None` means no match.
///
/// The query matches when its characters appear in `text` in order, ignoring case and spaces.
/// Runs of adjacent characters and matches at the start of a word score higher, so `"tt"` prefers
/// "Toggle Theme" over "Reset Layout".
pub fn score(query: &str, text: &str) -> Option<i32> {
    let mut wanted = query.chars().filter(|c| !c.is_whitespace()).map(lower);
    let mut next = wanted.next();

    let mut score = 0;
    let mut previous: Option<char> = None;
    let mut previous_matched = false;

    for ch in text.chars() {
        let Some(want) = next else { break };
        let matched = lower(ch) == want;
        if matched {
            score += 10;
            if previous_matched {
                score += 15;
            }
            if previous.is_none_or(|p| !p.is_alphanumeric()) {
                score += 10;
            }
            next = wanted.next();
        } else {
            score -= 1;
        }
        previous = Some(ch);
        previous_matched = matched;
    }

    next.is_none().then_some(score)
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_query_matches_everything() {
        assert_eq!(score("", "Anything"), Some(0));
        assert_eq!(score("   ", "Anything"), Some(0));
    }

    #[test]
    fn matches_subsequences_ignoring_case_and_spaces() {
        assert!(score("tgl", "Toggle Theme").is_some());
        assert!(score("TOGGLE THEME", "toggle theme").is_some());
        assert!(score("reset lay", "Reset Panel Layout").is_some());
    }

    #[test]
    fn rejects_out_of_order_or_missing_characters() {
        assert_eq!(score("xyz", "Toggle Theme"), None);
        assert_eq!(score("eht", "The"), None);
        assert_eq!(score("themes", "Theme"), None);
    }

    #[test]
    fn prefers_word_starts_and_adjacent_runs() {
        let initials = score("tt", "Toggle Theme");
        let scattered = score("tt", "Reset Layout");
        assert!(initials > scattered);

        let run = score("log", "Clear Log");
        let spread = score("log", "Toggle Light / Dark Theme");
        assert!(run > spread);
    }
}
