use super::*;

#[tokio::test]
async fn installed_tool_executes_notion_search_via_runtime_provider_path() {
    assert_notion_runtime_tool(
        "notion_search_pages",
        serde_json::json!({"query":"Team"}),
        "POST",
        &[
            "POST /search ",
            "\"query\":\"Team\"",
            "\"value\":\"page\"",
        ],
        r#"{"results":[{"id":"page-1","url":"https://notion.so/page-1","properties":{"title":{"title":[{"plain_text":"Team Notes"}]}}}],"next_cursor":null,"has_more":false}"#,
        "\"Team Notes\"",
    )
    .await;
}

#[tokio::test]
async fn installed_tool_executes_notion_create_page_via_runtime_provider_path() {
    assert_notion_runtime_tool(
        "notion_create_page",
        serde_json::json!({"parent_page_id":"page-1","title":"Aura Page"}),
        "POST",
        &[
            "POST /pages ",
            "\"page_id\":\"page-1\"",
            "\"content\":\"Aura Page\"",
        ],
        r#"{"id":"page-2","url":"https://notion.so/page-2","properties":{"title":{"title":[{"plain_text":"Aura Page"}]}}}"#,
        "\"page-2\"",
    )
    .await;
}

#[tokio::test]
async fn installed_tool_executes_notion_fetch_page_via_runtime_provider_path() {
    assert_notion_runtime_tool(
        "notion_fetch_page",
        serde_json::json!({"page_id":"page-1"}),
        "GET",
        &["GET /pages/page-1 "],
        r#"{"id":"page-1","url":"https://notion.so/page-1","properties":{"title":{"title":[{"plain_text":"Team Notes"}]}}}"#,
        "\"Team Notes\"",
    )
    .await;
}

#[tokio::test]
async fn installed_tool_executes_notion_get_blocks_via_runtime_provider_path() {
    assert_notion_runtime_tool(
        "notion_get_block_children",
        serde_json::json!({"block_id":"page-1","page_size":10}),
        "GET",
        &["GET /blocks/page-1/children?page_size=10 "],
        r#"{"object":"list","results":[{"object":"block","id":"block-1","type":"paragraph","paragraph":{"rich_text":[{"plain_text":"Planning notes"}]}}],"next_cursor":null,"has_more":false}"#,
        "\"block-1\"",
    )
    .await;
}

#[tokio::test]
async fn installed_tool_executes_notion_append_blocks_via_runtime_provider_path() {
    assert_notion_runtime_tool(
        "notion_append_block_children",
        serde_json::json!({"block_id":"page-1","content":"Follow up"}),
        "PATCH",
        &[
            "PATCH /blocks/page-1/children ",
            "\"children\"",
            "\"content\":\"Follow up\"",
        ],
        r#"{"object":"list","results":[{"object":"block","id":"block-2","type":"paragraph","paragraph":{"rich_text":[{"plain_text":"Follow up"}]}}],"next_cursor":null,"has_more":false}"#,
        "\"block-2\"",
    )
    .await;
}

#[tokio::test]
async fn installed_tool_executes_notion_update_page_via_runtime_provider_path() {
    assert_notion_runtime_tool(
        "notion_update_page",
        serde_json::json!({"page_id":"page-1","title":"Updated Notes"}),
        "PATCH",
        &[
            "PATCH /pages/page-1 ",
            "\"properties\"",
            "\"content\":\"Updated Notes\"",
        ],
        r#"{"id":"page-1","url":"https://notion.so/page-1","properties":{"title":{"title":[{"plain_text":"Updated Notes"}]}}}"#,
        "\"Updated Notes\"",
    )
    .await;
}

#[tokio::test]
async fn installed_tool_executes_notion_update_page_markdown_via_runtime_provider_path() {
    assert_notion_runtime_tool(
        "notion_update_page_markdown",
        serde_json::json!({"page_id":"page-1","markdown":"# Updated Notes\n\nShipped the prototype."}),
        "PATCH",
        &[
            "PATCH /pages/page-1/markdown ",
            "\"type\":\"replace_content\"",
            "\"replace_content\"",
            "\"new_str\":\"# Updated Notes\\n\\nShipped the prototype.\"",
        ],
        r##"{"object":"page_markdown","id":"page-1","markdown":"# Updated Notes\n\nShipped the prototype.","truncated":false,"unknown_block_ids":[]}"##,
        "\"# Updated Notes\\n\\nShipped the prototype.\"",
    )
    .await;
}

#[tokio::test]
async fn installed_tool_executes_notion_query_data_source_via_runtime_provider_path() {
    assert_notion_runtime_tool(
        "notion_query_data_source",
        serde_json::json!({"data_source_id":"ds-1","page_size":10}),
        "POST",
        &[
            "POST /data_sources/ds-1/query ",
            "\"page_size\":10",
        ],
        r#"{"object":"list","results":[{"id":"page-3","url":"https://notion.so/page-3","properties":{"Name":{"title":[{"plain_text":"Roadmap Item"}]}}}],"next_cursor":null,"has_more":false}"#,
        "\"Roadmap Item\"",
    )
    .await;
}

#[tokio::test]
async fn installed_tool_executes_notion_create_database_via_runtime_provider_path() {
    assert_notion_runtime_tool(
        "notion_create_database",
        serde_json::json!({"parent_page_id":"page-1","title":"Roadmap"}),
        "POST",
        &[
            "POST /databases ",
            "\"page_id\":\"page-1\"",
            "\"initial_data_source\"",
            "\"content\":\"Roadmap\"",
        ],
        r#"{"object":"database","id":"db-1","url":"https://notion.so/db-1","title":[{"plain_text":"Roadmap"}],"initial_data_source":{"properties":{"Name":{"title":{}}}},"data_sources":[{"id":"ds-1","name":"Roadmap"}]}"#,
        "\"db-1\"",
    )
    .await;
}

#[tokio::test]
async fn installed_tool_executes_notion_create_data_source_via_runtime_provider_path() {
    assert_notion_runtime_tool(
        "notion_create_data_source",
        serde_json::json!({"database_id":"db-1","title":"Archive"}),
        "POST",
        &[
            "POST /data_sources ",
            "\"database_id\":\"db-1\"",
            "\"content\":\"Archive\"",
        ],
        r#"{"object":"data_source","id":"ds-2","title":[{"plain_text":"Archive"}],"properties":{"Name":{"title":{}}}}"#,
        "\"ds-2\"",
    )
    .await;
}
