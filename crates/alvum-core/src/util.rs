/// Slices `s` to at most `max_chars` Unicode scalar values, avoiding mid-char byte splits.
pub fn truncate_chars(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// Defang any literal occurrences of an XML-style wrapper tag inside
/// `content`, returning the (possibly-modified) string and the count
/// of replacements. Used to stop captured user data from prematurely
/// closing a `<observations>…</observations>`-style wrapper used to
/// mark the data section of an LLM prompt.
///
/// Defanging inserts a zero-width space (`\u{200B}`) between `<` and
/// the tag name. The resulting string is visually identical to the
/// original but no longer matches the wrapper's open/close tokens,
/// while remaining readable to the LLM as ordinary text.
///
/// We do NOT escape every `<` and `>` — captured content frequently
/// contains code (HTML, JSX, generics) that the LLM needs to reason
/// over. Only the specific wrapper tag we use to delimit the data
/// section is at risk of being abused.
pub fn defang_wrapper_tag(content: &str, tag: &str) -> (String, usize) {
    let open = format!("<{tag}");
    let close = format!("</{tag}");
    let zwsp = '\u{200B}';
    let defanged_open = format!("<{zwsp}{tag}");
    let defanged_close = format!("</{zwsp}{tag}");

    let mut out = String::with_capacity(content.len());
    let mut count = 0usize;
    let mut i = 0;
    let bytes = content.as_bytes();
    while i < bytes.len() {
        let rest = &content[i..];
        if rest.starts_with(&close) {
            out.push_str(&defanged_close);
            i += close.len();
            count += 1;
        } else if rest.starts_with(&open) {
            out.push_str(&defanged_open);
            i += open.len();
            count += 1;
        } else {
            // Walk one UTF-8 character at a time. Indexing by byte
            // would slice through multibyte glyphs.
            let ch = content[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    (out, count)
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

    #[test]
    fn defang_replaces_close_tag() {
        let (out, count) = defang_wrapper_tag(
            "stuff </observations> more stuff",
            "observations",
        );
        assert_eq!(count, 1);
        assert!(!out.contains("</observations>"));
        // The defanged form still contains the visible characters; the
        // zero-width space is what makes it not match.
        assert!(out.contains("</\u{200B}observations>"));
    }

    #[test]
    fn defang_replaces_open_tag() {
        let (out, count) = defang_wrapper_tag(
            "<observations>nested</observations>",
            "observations",
        );
        assert_eq!(count, 2);
        assert!(out.starts_with("<\u{200B}observations>"));
        assert!(out.contains("</\u{200B}observations>"));
    }

    #[test]
    fn defang_leaves_unrelated_xml_alone() {
        // A captured snippet of HTML/JSX with `<div>` and `<>` must
        // pass through untouched so the LLM can still reason about
        // it. Only the specific wrapper tag is affected.
        let (out, count) = defang_wrapper_tag(
            "<div>code</div> <Component />",
            "observations",
        );
        assert_eq!(count, 0);
        assert_eq!(out, "<div>code</div> <Component />");
    }

    #[test]
    fn defang_preserves_multibyte_characters() {
        // Walking by bytes would slice mid-glyph. The implementation
        // walks by `char` so emoji and non-ASCII content survive.
        let (out, count) = defang_wrapper_tag(
            "hello 🎉 </observations> world",
            "observations",
        );
        assert_eq!(count, 1);
        assert!(out.contains("🎉"));
        assert!(!out.contains("</observations>"));
    }
}
