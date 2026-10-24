//! 适配层测试 — 过滤规则与描述符转换（不启动 MCP 服务、不调用 LLM）
//!
//! 读 / 写分类的唯一真源是 `ToolRegistry` 的注册点
//! （`register` / `register_readonly`）。本文件锁定 MCP 只读模式的契约：
//! 只读模式暴露的集合必须恰好等于注册表里声明为只读的工具（减去隐藏工具），
//! 且**会写项目文档树的工具绝不能出现在其中**。

use std::sync::Arc;

use story_mcp::{build_tool_descriptors, is_exposed, HIDDEN_TOOLS};
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

/// 会改动项目文档树的代表性工具——注册表里一个都不许标成只读
const MUTATING_TOOLS: &[&str] = &[
    "create_entity",
    "create_scene",
    "write_fragment_text",
    "append_fragment_text",
    "edit_fragment_text",
    "delete_entity",
    "delete_scene",
    "delete_chapter",
    "advance_thread",
    "update_scene",
    "repair_project",
    "set_outline_item",
    "merge_entities",
    "split_scene",
    // 回归：现场生成的场景摘要会写回 structure.toml —— 只读模式下暴露它，
    // 等于让 --read-only 服务改稿件，同时闸门还会按读锁放行（读写并发）
    "get_story_recap",
];

#[test]
fn mutating_tools_are_not_classified_read_only() {
    let (_tmp, registry) = temp_registry();
    for tool in MUTATING_TOOLS {
        assert!(
            !registry.is_read_only(tool),
            "{tool} 会改动项目，不应被注册为只读（分类在 create_tool_registry 的注册点）"
        );
    }
}

#[test]
fn read_only_classification_is_declared_for_every_listed_tool() {
    let (_tmp, registry) = temp_registry();
    let names: Vec<String> = registry.definitions().into_iter().map(|d| d.name).collect();
    let read_only = registry.read_only_names();
    assert!(
        !read_only.is_empty(),
        "注册表应至少有一个只读工具（否则只读模式形同虚设）"
    );
    for tool in &read_only {
        assert!(
            names.iter().any(|n| n == tool),
            "read_only_names() 含未注册的工具：{tool}"
        );
    }
    // 只读集合必须是注册表的真子集：不能把全部工具都标成只读
    assert!(
        read_only.len() < names.len(),
        "只读工具数（{}）不应等于全部工具数（{}）",
        read_only.len(),
        names.len()
    );
}

#[test]
fn ask_user_is_read_only_for_the_gate_but_hidden_from_mcp() {
    let (_tmp, registry) = temp_registry();
    let names: Vec<String> = registry.definitions().into_iter().map(|d| d.name).collect();
    assert!(
        names.iter().any(|n| n == "ask_user"),
        "registry 应包含 ask_user"
    );
    // 闸门分类：等待用户回答期间绝不能持写锁
    assert!(registry.is_read_only("ask_user"));
    // MCP：无宿主 UI，两种模式都不暴露
    assert!(!is_exposed(&registry, "ask_user", false));
    assert!(!is_exposed(&registry, "ask_user", true));
}

#[test]
fn full_mode_exposes_all_except_hidden() {
    let (_tmp, registry) = temp_registry();
    let expected = registry.definitions().len() - 1; // 仅隐藏 ask_user
    let descriptors = build_tool_descriptors(&registry, false);
    assert_eq!(descriptors.len(), expected);
}

#[test]
fn read_only_mode_exposes_exactly_the_read_only_set() {
    let (_tmp, registry) = temp_registry();
    let descriptors = build_tool_descriptors(&registry, true);
    let expected: Vec<&str> = registry
        .read_only_names()
        .into_iter()
        .filter(|n| !HIDDEN_TOOLS.contains(n))
        .collect();
    let mut got: Vec<&str> = descriptors.iter().map(|t| t.name.as_ref()).collect();
    got.sort_unstable();
    let mut want = expected;
    want.sort_unstable();
    assert_eq!(got, want, "只读模式暴露的集合应恰好等于注册表的只读分类");
    for tool in &got {
        assert!(
            registry.is_read_only(tool),
            "{tool} 出现在只读模式却没被注册为只读"
        );
    }
}

#[test]
fn descriptor_preserves_name_description_schema() {
    let (_tmp, registry) = temp_registry();
    let search_def = registry
        .definitions()
        .into_iter()
        .find(|d| d.name == "search_text")
        .expect("search_text 应已注册");
    let descriptors = build_tool_descriptors(&registry, false);
    let tool = descriptors
        .iter()
        .find(|t| t.name.as_ref() == "search_text")
        .expect("search_text 应出现在描述符中");
    assert!(!tool.description.as_ref().unwrap().is_empty());
    assert_eq!(
        tool.input_schema.get("type").and_then(|v| v.as_str()),
        search_def.parameters.get("type").and_then(|v| v.as_str())
    );
}

#[test]
fn deterministic_tool_executes_through_registry() {
    let (_tmp, registry) = temp_registry();
    // 只读集合内的确定性工具：不触发 LLM
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
