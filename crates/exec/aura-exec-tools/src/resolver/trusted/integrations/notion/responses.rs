//! Shaping Notion API responses into tool results.

use serde_json::{json, Value};

pub(super) fn notion_page_list_response(response: &Value) -> Value {
    let results = response
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let pages = results
        .iter()
        .map(notion_page_summary)
        .collect::<Vec<Value>>();
    json!({
        "pages": pages,
        "results": results,
        "next_cursor": response.get("next_cursor").cloned().unwrap_or(Value::Null),
        "has_more": response.get("has_more").and_then(Value::as_bool).unwrap_or(false),
    })
}

pub(super) fn notion_block_list_response(response: &Value) -> Value {
    json!({
        "blocks": response.get("results").cloned().unwrap_or_else(|| json!([])),
        "next_cursor": response.get("next_cursor").cloned().unwrap_or(Value::Null),
        "has_more": response.get("has_more").and_then(Value::as_bool).unwrap_or(false),
    })
}

pub(super) fn notion_page_summary(page: &Value) -> Value {
    json!({
        "id": page.get("id").and_then(Value::as_str).unwrap_or_default(),
        "url": page.get("url").and_then(Value::as_str).unwrap_or_default(),
        "title": notion_page_title(page),
        "properties": page.get("properties").cloned().unwrap_or_else(|| json!({})),
        "parent": page.get("parent").cloned().unwrap_or(Value::Null),
        "created_time": page.get("created_time").cloned().unwrap_or(Value::Null),
        "last_edited_time": page.get("last_edited_time").cloned().unwrap_or(Value::Null),
        "in_trash": page.get("in_trash").cloned().unwrap_or(Value::Null),
        "is_locked": page.get("is_locked").cloned().unwrap_or(Value::Null),
    })
}

pub(super) fn notion_database_summary(database: &Value) -> Value {
    json!({
        "id": database.get("id").and_then(Value::as_str).unwrap_or_default(),
        "url": database.get("url").and_then(Value::as_str).unwrap_or_default(),
        "title": notion_rich_text_plain(database.get("title")),
        "properties": database.pointer("/initial_data_source/properties")
            .or_else(|| database.get("properties"))
            .cloned()
            .unwrap_or_else(|| json!({})),
        "data_sources": database.get("data_sources").cloned().unwrap_or_else(|| json!([])),
    })
}

pub(super) fn notion_data_source_summary(data_source: &Value) -> Value {
    json!({
        "id": data_source.get("id").and_then(Value::as_str).unwrap_or_default(),
        "title": notion_rich_text_plain(data_source.get("title")),
        "properties": data_source.get("properties").cloned().unwrap_or_else(|| json!({})),
    })
}

fn notion_page_title(page: &Value) -> String {
    page.get("properties")
        .and_then(Value::as_object)
        .and_then(|properties| {
            properties.values().find_map(|property| {
                property
                    .get("title")
                    .and_then(Value::as_array)
                    .map(|title| notion_rich_text_plain(Some(&Value::Array(title.clone()))))
                    .filter(|title| !title.is_empty())
            })
        })
        .unwrap_or_else(|| notion_rich_text_plain(page.get("title")))
}

fn notion_rich_text_plain(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_array)
        .map(|fragments| {
            fragments
                .iter()
                .filter_map(|fragment| {
                    fragment
                        .get("plain_text")
                        .and_then(Value::as_str)
                        .or_else(|| {
                            fragment
                                .get("text")
                                .and_then(|text| text.get("content"))
                                .and_then(Value::as_str)
                        })
                })
                .collect::<String>()
        })
        .unwrap_or_default()
}
