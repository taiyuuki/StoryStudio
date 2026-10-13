//! 适配层测试 — 过滤规则与描述符转换（不启动 MCP 服务、不调用 LLM）

use std::sync::Arc;

use story_mcp::{build_tool_descriptors, is_exposed, HIDDEN_TOOLS, READ_ONLY_TOOLS};
use story_studio_lib::{create_project, create_provider, create_tool_registry};

/// 临时项目 + 完整工具注册表（占位 provider，测试只走确定性工具）
fn temp_registry() -> (tempfile::TempDir, Arc<story_ai_runtime::ToolRegistry>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = create_project(tmp.path(), "测试书").unwrap();
    // 占位 provider：无 API Key 也能构造，本文件的工具不会触发 LLM 调用
    let provider = create_provider(
        "anthropic",
        None,
        None,
        None,
        false,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .unwrap();
    let registry = Arc::new(create_tool_registry(&root, Arc::from(provider)));
    (tmp, registry)
}

#[test]
fn read_only_whitelist_entries_all_exist() {
    let (_tmp, registry) = temp_registry();
    let names: Vec<String> = registry.definitions().into_iter().map(|d| d.name).collect();
    for tool in READ_ONLY_TOOLS {
        assert!(
            names.contains(&tool.to_string()),
            "read-only 白名单包含不存在的工具：{tool}（registry 改名后需要同步白名单）"
        );
    }
}

#[test]
fn read_only_whitelist_excludes_mutating_tools() {
    let mutating = [
        "create_character",
        "write_fragment_text",
        "delete_entity",
        "advance_thread",
        "update_scene",
        "repair_project",
        "set_outline_item",
    ];
    for tool in mutating {
        assert!(
            !READ_ONLY_TOOLS.contains(&tool),
            "{tool} 是修改类工具，不应出现在只读白名单"
        );
    }
}

#[test]
fn ask_user_hidden_in_both_modes() {
    let (_tmp, registry) = temp_registry();
    let names: Vec<String> = registry
        .definitions()
        .iter()
        .map(|d| d.name.clone())
        .collect();
    assert!(
        names.iter().any(|n| n == "ask_user"),
        "registry 应包含 ask_user"
    );
    assert!(!is_exposed("ask_user", false));
    assert!(!is_exposed("ask_user", true));
}

#[test]
fn full_mode_exposes_all_except_hidden() {
    let (_tmp, registry) = temp_registry();
    let defs = registry.definitions();
    let expected = defs.len() - 1; // 仅隐藏 ask_user
    let descriptors = build_tool_descriptors(defs, false);
    assert_eq!(descriptors.len(), expected);
}

#[test]
fn read_only_mode_exposes_only_whitelist() {
    let (_tmp, registry) = temp_registry();
    let defs = registry.definitions();
    let descriptors = build_tool_descriptors(defs, true);
    // 白名单 ∖ 隐藏工具（ask_user 无项目副作用、在闸门分类中属读端，
    // 但 MCP 无宿主 UI 不暴露）
    let expected = READ_ONLY_TOOLS
        .iter()
        .filter(|n| !HIDDEN_TOOLS.contains(n))
        .count();
    assert_eq!(descriptors.len(), expected);
    for tool in &descriptors {
        assert!(READ_ONLY_TOOLS.contains(&tool.name.as_ref()));
    }
}

#[test]
fn descriptor_preserves_name_description_schema() {
    let (_tmp, registry) = temp_registry();
    let defs = registry.definitions();
    let search_def = defs
        .iter()
        .find(|d| d.name == "search_text")
        .expect("search_text 应已注册")
        .clone();
    let descriptors = build_tool_descriptors(vec![search_def.clone()], false);
    let tool = &descriptors[0];
    assert_eq!(tool.name.as_ref(), "search_text");
    assert!(!tool.description.as_ref().unwrap().is_empty());
    assert_eq!(
        tool.input_schema.get("type").and_then(|v| v.as_str()),
        search_def.parameters.get("type").and_then(|v| v.as_str())
    );
}

#[test]
fn deterministic_tool_executes_through_registry() {
    let (_tmp, registry) = temp_registry();
    // 只读白名单内的确定性工具：不触发 LLM
    let result = registry.execute(
        "search_text",
        &serde_json::json!({"query": "主角", "max_hits": 10}),
    );
    match result {
        Some(Ok(text)) => assert!(!text.is_empty(), "空项目上 search_text 也应有输出说明"),
        Some(Err(e)) => panic!("search_text 执行失败：{e}"),
        None => panic!("search_text 未注册"),
    }
}

#[test]
fn clamp_truncates_oversized_result() {
    let raw = "字".repeat(30_000);
    let clamped = story_ai_runtime::clamp_tool_result(&raw);
    assert!(clamped.contains("已截断"));
    assert!(clamped.chars().count() < raw.chars().count());
}
