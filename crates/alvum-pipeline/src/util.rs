/// Slices `s` to at most `max_chars` Unicode scalar values, avoiding mid-char byte splits.
pub fn truncate_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// Strip markdown code fences that LLMs sometimes wrap around JSON output.
pub fn strip_markdown_fences(s: &str) -> &str {
    s.trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_ascii() {
        assert_eq!(truncate_chars("hello world", 5), "hello");
    }

    #[test]
    fn truncate_unicode() {
        assert_eq!(truncate_chars("héllo", 3), "hél");
    }

    #[test]
    fn truncate_longer_than_string() {
        assert_eq!(truncate_chars("hi", 100), "hi");
    }

    #[test]
    fn strip_json_fences() {
        assert_eq!(strip_markdown_fences("```json\n[1,2]\n```"), "[1,2]");
    }

    #[test]
    fn strip_plain_fences() {
        assert_eq!(strip_markdown_fences("```\n[1,2]\n```"), "[1,2]");
    }

    #[test]
    fn strip_no_fences() {
        assert_eq!(strip_markdown_fences("[1,2]"), "[1,2]");
    }
}
