//! Small JSON-repair helpers for LLM level outputs.
//!
//! The level-specific modules own semantic repair because only they know
//! the valid references for their input. This module keeps the mechanical
//! coercions shared and boring.

use alvum_core::util::strip_markdown_fences;
use serde_json::{Map, Value};

pub(crate) fn response_items(response: &str) -> Option<Vec<Value>> {
    let value = serde_json::from_str::<Value>(strip_markdown_fences(response)).ok()?;
    match value {
        Value::Array(items) => Some(items),
        Value::Object(_) => Some(vec![value]),
        _ => None,
    }
}

pub(crate) fn string_field(map: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|key| map.get(*key))
        .find_map(string_value)
}

pub(crate) fn string_value(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => non_empty(text),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

pub(crate) fn string_array_field(map: &Map<String, Value>, keys: &[&str]) -> Vec<String> {
    keys.iter()
        .filter_map(|key| map.get(*key))
        .find_map(string_array_value)
        .unwrap_or_default()
}

pub(crate) fn id_array_field(map: &Map<String, Value>, keys: &[&str]) -> Vec<String> {
    keys.iter()
        .filter_map(|key| map.get(*key))
        .find_map(id_array_value)
        .unwrap_or_default()
}

pub(crate) fn string_array_value(value: &Value) -> Option<Vec<String>> {
    match value {
        Value::Array(items) => Some(items.iter().filter_map(string_value).collect()),
        Value::String(text) => Some(split_string_list(text)),
        _ => None,
    }
}

pub(crate) fn id_array_value(value: &Value) -> Option<Vec<String>> {
    match value {
        Value::Array(items) => Some(
            items
                .iter()
                .filter_map(|item| match item {
                    Value::Object(map) => string_field(map, &["id", "thread_id", "cluster_id"]),
                    _ => string_value(item),
                })
                .collect(),
        ),
        Value::String(text) => Some(split_string_list(text)),
        Value::Object(map) => string_field(map, &["id", "thread_id", "cluster_id"])
            .map(|id| vec![id])
            .or_else(|| string_array_value(value)),
        _ => None,
    }
}

pub(crate) fn bool_field(map: &Map<String, Value>, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .filter_map(|key| map.get(*key))
        .find_map(|value| {
            value.as_bool().or_else(|| {
                value.as_str().and_then(|text| {
                    let normalized = text.trim().to_ascii_lowercase();
                    match normalized.as_str() {
                        "true" | "yes" | "open" => Some(true),
                        "false" | "no" | "closed" => Some(false),
                        _ => None,
                    }
                })
            })
        })
}

pub(crate) fn f32_field(map: &Map<String, Value>, keys: &[&str]) -> Option<f32> {
    keys.iter()
        .filter_map(|key| map.get(*key))
        .find_map(|value| {
            value
                .as_f64()
                .map(|number| number as f32)
                .or_else(|| value.as_str()?.trim().parse::<f32>().ok())
        })
}

pub(crate) fn clamp01(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

pub(crate) fn sentence_prefix(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    let sentence_end = trimmed
        .char_indices()
        .find(|(_, ch)| matches!(ch, '.' | '!' | '?'))
        .map(|(idx, ch)| idx + ch.len_utf8());
    let end = sentence_end.unwrap_or_else(|| {
        trimmed
            .char_indices()
            .nth(max_chars)
            .map(|(idx, _)| idx)
            .unwrap_or(trimmed.len())
    });
    trimmed[..end].trim().to_string()
}

pub(crate) fn id_fragment(text: &str) -> String {
    let mut out = String::new();
    let mut last_was_separator = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_was_separator = false;
        } else if !last_was_separator {
            out.push('_');
            last_was_separator = true;
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        "item".into()
    } else {
        trimmed.into()
    }
}

pub(crate) fn non_empty(text: &str) -> Option<String> {
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn split_string_list(text: &str) -> Vec<String> {
    text.split([',', ';']).filter_map(non_empty).collect()
}
