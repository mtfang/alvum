/// Slices `s` to at most `max_chars` Unicode scalar values, avoiding mid-char byte splits.
pub fn truncate_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// Extract JSON content from an LLM response that may include markdown fences
/// and/or explanation text before or after the JSON.
pub fn strip_markdown_fences(s: &str) -> &str {
    let trimmed = s.trim();

    // Try to find a JSON array or object by matching balanced brackets
    // Look for the first [ or { and find its matching closer
    if let Some(start) = trimmed.find(|c| c == '[' || c == '{') {
        let open = trimmed.as_bytes()[start];
        let close = if open == b'[' { b']' } else { b'}' };
        let mut depth = 0;
        let mut in_string = false;
        let mut escape = false;

        for (i, &byte) in trimmed.as_bytes()[start..].iter().enumerate() {
            if escape {
                escape = false;
                continue;
            }
            if byte == b'\\' && in_string {
                escape = true;
                continue;
            }
            if byte == b'"' {
                in_string = !in_string;
                continue;
            }
            if in_string {
                continue;
            }
            if byte == open {
                depth += 1;
            } else if byte == close {
                depth -= 1;
                if depth == 0 {
                    return &trimmed[start..start + i + 1];
                }
            }
        }
    }

    // Fallback: simple fence stripping
    trimmed
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

    #[test]
    fn strip_fences_with_trailing_text() {
        let input = "```json\n[]\n```\n\nNeither decision has a causal link because...";
        assert_eq!(strip_markdown_fences(input), "[]");
    }

    #[test]
    fn strip_fences_with_leading_text() {
        let input = "Here are the results:\n```json\n[{\"id\":\"dec_001\"}]\n```";
        assert_eq!(strip_markdown_fences(input), "[{\"id\":\"dec_001\"}]");
    }

    #[test]
    fn extract_nested_json() {
        let input = "```json\n[{\"a\": [1,2], \"b\": {\"c\": 3}}]\n```";
        assert_eq!(strip_markdown_fences(input), "[{\"a\": [1,2], \"b\": {\"c\": 3}}]");
    }

    #[test]
    fn extract_json_no_fences_with_trailing() {
        let input = "[]\n\nNo decisions found in this audio.";
        assert_eq!(strip_markdown_fences(input), "[]");
    }
}
