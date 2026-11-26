//! Request bodies for the Notion API.

use super::args::{
    has_any, insert_optional_array, insert_optional_bool, insert_optional_object,
    insert_optional_page_size, insert_optional_string, notion_id_from_ref, optional_array,
    optional_bool, optional_notion_id, optional_object, required_notion_id,
};
use crate::error::ToolError;
use crate::resolver::json_paths::{optional_string, required_string};
use serde_json::{json, Map, Value};

const MAX_PARAGRAPH_BLOCKS: usize = 20;

pub(super) fn create_page_payload(args: &Value) -> Result<Value, ToolError> {
    let mut body = Map::new();
    body.insert("parent".to_string(), notion_page_parent(args)?);
    body.insert("properties".to_string(), notion_page_properties(args)?);
    insert_page_content(&mut body, args)?;
    insert_optional_object(&mut body, args, &["icon"], "icon")?;
    insert_optional_object(&mut body, args, &["cover"], "cover")?;
    insert_optional_object(&mut body, args, &["template"], "template")?;
    insert_optional_object(&mut body, args, &["position"], "position")?;
    Ok(Value::Object(body))
}

pub(super) fn update_page_payload(args: &Value) -> Result<Value, ToolError> {
    if let Some(payload) = optional_object(args, &["payload"])? {
        return Ok(payload);
    }

    let mut body = Map::new();
    if has_any(args, &["properties", "title"]) {
        body.insert("properties".to_string(), notion_page_properties(args)?);
    }
    insert_optional_object(&mut body, args, &["icon"], "icon")?;
    insert_optional_object(&mut body, args, &["cover"], "cover")?;
    insert_optional_object(&mut body, args, &["template"], "template")?;
    insert_optional_bool(
        &mut body,
        args,
        &["in_trash", "inTrash", "archived"],
        "in_trash",
    );
    insert_optional_bool(&mut body, args, &["is_locked", "isLocked"], "is_locked");
    insert_optional_bool(
        &mut body,
        args,
        &["erase_content", "eraseContent"],
        "erase_content",
    );
    if body.is_empty() {
        return Err(ToolError::ExternalToolError(
            "missing page update fields such as `title`, `properties`, `in_trash`, `icon`, or `cover`".into(),
        ));
    }
    Ok(Value::Object(body))
}

pub(super) fn update_page_markdown_payload(args: &Value) -> Result<Value, ToolError> {
    if let Some(payload) = optional_object(args, &["payload"])? {
        return Ok(payload);
    }

    let markdown = required_string(args, &["markdown", "content", "body"])?;
    let mut replace_content = Map::new();
    replace_content.insert("new_str".to_string(), Value::String(markdown));
    insert_optional_bool(
        &mut replace_content,
        args,
        &["allow_deleting_content", "allowDeletingContent"],
        "allow_deleting_content",
    );

    let mut body = Map::new();
    body.insert(
        "type".to_string(),
        Value::String("replace_content".to_string()),
    );
    body.insert(
        "replace_content".to_string(),
        Value::Object(replace_content),
    );
    insert_optional_bool(
        &mut body,
        args,
        &["allow_async", "allowAsync"],
        "allow_async",
    );
    Ok(Value::Object(body))
}

pub(super) fn append_blocks_payload(args: &Value) -> Result<Value, ToolError> {
    let mut body = Map::new();
    body.insert("children".to_string(), Value::Array(notion_blocks(args)?));
    if let Some(position) = optional_object(args, &["position"])? {
        body.insert("position".to_string(), position);
    } else if let Some(after) = optional_string(args, &["after_block_id", "afterBlockId"]) {
        body.insert(
            "position".to_string(),
            json!({ "type": "after_block", "after_block": { "id": notion_id_from_ref(&after) } }),
        );
    }
    Ok(Value::Object(body))
}

pub(super) fn query_data_source_payload(args: &Value) -> Result<Value, ToolError> {
    if let Some(payload) = optional_object(args, &["payload", "query"])? {
        return Ok(payload);
    }

    let mut body = Map::new();
    insert_optional_object(&mut body, args, &["filter"], "filter")?;
    insert_optional_array(&mut body, args, &["sorts"], "sorts")?;
    insert_optional_page_size(&mut body, args);
    insert_optional_string(
        &mut body,
        args,
        &["start_cursor", "startCursor"],
        "start_cursor",
    );
    Ok(Value::Object(body))
}

pub(super) fn create_database_payload(args: &Value) -> Result<Value, ToolError> {
    let mut body = Map::new();
    let title = required_string(args, &["title"])?;
    body.insert("parent".to_string(), notion_database_parent(args)?);
    body.insert("title".to_string(), notion_rich_text_array(&title));
    if let Some(description) = optional_string(args, &["description"]) {
        body.insert(
            "description".to_string(),
            notion_rich_text_array(&description),
        );
    }
    insert_optional_bool(&mut body, args, &["is_inline", "isInline"], "is_inline");
    insert_optional_object(&mut body, args, &["icon"], "icon")?;
    insert_optional_object(&mut body, args, &["cover"], "cover")?;

    let initial_data_source = optional_object(args, &["initial_data_source", "initialDataSource"])?
        .unwrap_or_else(|| json!({ "properties": data_source_properties(args) }));
    body.insert("initial_data_source".to_string(), initial_data_source);
    Ok(Value::Object(body))
}

pub(super) fn create_data_source_payload(args: &Value) -> Result<Value, ToolError> {
    let mut body = Map::new();
    body.insert("parent".to_string(), notion_data_source_parent(args)?);
    body.insert("properties".to_string(), data_source_properties(args));
    if let Some(title) = optional_string(args, &["title"]) {
        body.insert("title".to_string(), notion_rich_text_array(&title));
    }
    insert_optional_object(&mut body, args, &["icon"], "icon")?;
    Ok(Value::Object(body))
}

fn insert_page_content(body: &mut Map<String, Value>, args: &Value) -> Result<(), ToolError> {
    if let Some(markdown) =
        optional_string(args, &["markdown", "markdown_contents", "markdownContents"])
    {
        body.insert("markdown".to_string(), Value::String(markdown));
        return Ok(());
    }
    if let Some(children) = optional_array(args, &["children", "blocks"])? {
        if !children.is_empty() {
            body.insert("children".to_string(), Value::Array(children));
        }
        return Ok(());
    }
    let blocks = notion_children_blocks(optional_string(args, &["content", "body"]).as_deref());
    if !blocks.is_empty() {
        body.insert("children".to_string(), Value::Array(blocks));
    }
    Ok(())
}

fn notion_blocks(args: &Value) -> Result<Vec<Value>, ToolError> {
    if let Some(children) = optional_array(args, &["children", "blocks"])? {
        if !children.is_empty() {
            return Ok(children);
        }
    }
    let blocks = notion_children_blocks(optional_string(args, &["content", "body"]).as_deref());
    if blocks.is_empty() {
        Err(ToolError::ExternalToolError(
            "missing block content: provide `children`, `blocks`, or `content`".into(),
        ))
    } else {
        Ok(blocks)
    }
}

fn notion_page_parent(args: &Value) -> Result<Value, ToolError> {
    if let Some(parent) = optional_object(args, &["parent"])? {
        return Ok(parent);
    }
    if let Some(page_id) = optional_notion_id(args, &["parent_page_id", "parentPageId"]) {
        return Ok(json!({ "page_id": page_id }));
    }
    if let Some(data_source_id) = optional_notion_id(args, &["data_source_id", "dataSourceId"]) {
        return Ok(json!({ "data_source_id": data_source_id }));
    }
    if let Some(database_id) = optional_notion_id(args, &["database_id", "databaseId"]) {
        return Ok(json!({ "database_id": database_id }));
    }
    if optional_bool(args, &["workspace"]).unwrap_or(false) {
        return Ok(json!({ "workspace": true }));
    }
    Err(ToolError::ExternalToolError(
        "missing required field `parent_page_id`, `data_source_id`, `database_id`, or `parent`"
            .into(),
    ))
}

fn notion_database_parent(args: &Value) -> Result<Value, ToolError> {
    if let Some(parent) = optional_object(args, &["parent"])? {
        return Ok(parent);
    }
    if let Some(page_id) = optional_notion_id(args, &["parent_page_id", "parentPageId"]) {
        return Ok(json!({ "page_id": page_id }));
    }
    if optional_bool(args, &["workspace"]).unwrap_or(false) {
        return Ok(json!({ "workspace": true }));
    }
    Err(ToolError::ExternalToolError(
        "missing required field `parent_page_id` or `parent`".into(),
    ))
}

fn notion_data_source_parent(args: &Value) -> Result<Value, ToolError> {
    if let Some(parent) = optional_object(args, &["parent"])? {
        return Ok(parent);
    }
    let database_id = required_notion_id(args, &["database_id", "databaseId"])?;
    Ok(json!({ "database_id": database_id }))
}

fn notion_page_properties(args: &Value) -> Result<Value, ToolError> {
    if let Some(properties) = optional_object(args, &["properties"])? {
        return Ok(properties);
    }
    let title = required_string(args, &["title"])?;
    let title_property = optional_string(args, &["title_property", "titleProperty"])
        .unwrap_or_else(|| {
            if has_any(
                args,
                &[
                    "data_source_id",
                    "dataSourceId",
                    "database_id",
                    "databaseId",
                ],
            ) {
                "Name".to_string()
            } else {
                "title".to_string()
            }
        });
    let mut properties = Map::new();
    properties.insert(title_property, notion_title_property(&title));
    Ok(Value::Object(properties))
}

fn data_source_properties(args: &Value) -> Value {
    args.get("properties")
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| json!({ "Name": { "title": {} } }))
}

fn notion_title_property(title: &str) -> Value {
    json!({ "title": notion_rich_text_array(title) })
}

fn notion_rich_text_array(text: &str) -> Value {
    json!([{ "type": "text", "text": { "content": text } }])
}

fn notion_children_blocks(content: Option<&str>) -> Vec<Value> {
    content
        .unwrap_or_default()
        .split("\n\n")
        .map(str::trim)
        .filter(|paragraph| !paragraph.is_empty())
        .take(MAX_PARAGRAPH_BLOCKS)
        .map(|paragraph| {
            json!({
                "object": "block",
                "type": "paragraph",
                "paragraph": {
                    "rich_text": [{
                        "type": "text",
                        "text": { "content": paragraph }
                    }]
                }
            })
        })
        .collect()
}
