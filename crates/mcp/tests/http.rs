//! HTTP 传输集成测试 — 握手 / 鉴权 / 工具调用 / 停机（不调用 LLM）

use std::time::Duration;

use story_mcp::{start_server_http, HttpServerOptions, McpServerOptions};

fn client() -> reqwest::blocking::Client {
    // no_proxy：绕过系统代理环境变量，确保直连 127.0.0.1
    // （否则请求经代理转发，停机后会拿到 502 而不是连接失败）
    reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap()
}

/// POST 一条 JSON-RPC 消息，返回 (会话 ID, 解析出的 JSON-RPC 响应)
fn rpc(
    http: &reqwest::blocking::Client,
    url: &str,
    token: &str,
    session: Option<&str>,
    body: serde_json::Value,
) -> (Option<String>, serde_json::Value) {
    let mut req = http
        .post(url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&body);
    if let Some(sid) = session {
        req = req.header("Mcp-Session-Id", sid);
    }
    let resp = req.send().unwrap();
    assert_eq!(resp.status().as_u16(), 200, "JSON-RPC 请求应返回 200");
    let sid = resp
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let text = resp.text().unwrap();
    let value = if content_type.contains("text/event-stream") {
        // SSE：取含 result/error 的 data: 行
        text.lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l.trim()).ok())
            .rev()
            .find(|v| v.get("result").is_some() || v.get("error").is_some())
            .expect("SSE 流中应有 JSON-RPC 响应")
    } else {
        serde_json::from_str(&text).expect("响应应为 JSON")
    };
    (sid, value)
}

fn initialize(http: &reqwest::blocking::Client, url: &str, token: &str) -> Option<String> {
    let (sid, resp) = rpc(
        http,
        url,
        token,
        None,
        serde_json::json!({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "it", "version": "0"}
            }
        }),
    );
    assert!(resp.get("result").is_some(), "initialize 失败：{resp}");
    assert_eq!(
        resp["result"]["protocolVersion"], "2025-11-25",
        "版本协商应符合服务端版本表"
    );
    sid
}

fn spawn_server(read_only: bool) -> (tempfile::TempDir, story_mcp::McpHttpHandle) {
    let tmp = tempfile::tempdir().unwrap();
    let root = story_studio_lib::create_project(tmp.path(), "测试书").unwrap();
    let handle = start_server_http(
        McpServerOptions {
            project_dir: root,
            read_only,
        },
        HttpServerOptions {
            port: 0,
            // 测试注入独立 Token 与 Session 文件，避免读写真实的 ~/.story-studio/
            token: Some(format!("{:032x}", u64::from(!read_only))),
            session_path: Some(tmp.path().join("mcp-sessions.json")),
            ..Default::default()
        },
    )
    .unwrap();
    (tmp, handle)
}

#[test]
fn token_persists_across_starts() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("mcp-token");

    // 首次生成并落盘
    let first = story_mcp::load_or_create_token_at(&path).unwrap();
    assert_eq!(first.len(), 32);
    assert!(path.is_file());

    // 再次读取得到同一值（换书重启服务不轮换 Token 的基础）
    let second = story_mcp::load_or_create_token_at(&path).unwrap();
    assert_eq!(first, second);

    // 损坏的文件（长度不对）会被重新生成
    std::fs::write(&path, "bad").unwrap();
    let third = story_mcp::load_or_create_token_at(&path).unwrap();
    assert_ne!(third, first);
    assert_eq!(third.len(), 32);
}

#[test]
fn http_full_roundtrip() {
    let (_tmp, handle) = spawn_server(false);
    let url = handle.url();
    let token = handle.token().to_string();
    let http = client();

    // 无 Token → 401
    let status = http
        .post(&url)
        .header("Content-Type", "application/json")
        .body("{}")
        .send()
        .unwrap()
        .status();
    assert_eq!(status.as_u16(), 401, "缺 Token 应被拒");

    let session = initialize(&http, &url, &token);

    // tools/list
    let (_, listed) = rpc(
        &http,
        &url,
        &token,
        session.as_deref(),
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
    );
    let tools = listed["result"]["tools"]
        .as_array()
        .expect("tools/list 应返回工具数组");
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"search_text"));
    assert!(!names.contains(&"ask_user"), "ask_user 不应暴露");

    // tools/call（确定性工具）
    let (_, called) = rpc(
        &http,
        &url,
        &token,
        session.as_deref(),
        serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": "get_project_stats", "arguments": {}}
        }),
    );
    let result = &called["result"];
    assert_eq!(result["isError"], serde_json::json!(false));
    assert!(!result["content"][0]["text"].as_str().unwrap().is_empty());

    // 停机后端口关闭（用全新客户端，避免复用连接池里的旧连接）
    handle.stop();
    let fresh = client();
    let refused = fresh
        .post(&url)
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({"jsonrpc": "2.0", "id": 3, "method": "ping"}))
        .send();
    assert!(refused.is_err(), "停机后不应再能连接");
}

#[test]
fn http_read_only_mode() {
    let (_tmp, handle) = spawn_server(true);
    let url = handle.url();
    let token = handle.token().to_string();
    let http = client();

    let session = initialize(&http, &url, &token);
    let (_, listed) = rpc(
        &http,
        &url,
        &token,
        session.as_deref(),
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
    );
    let tools = listed["result"]["tools"].as_array().unwrap();
    assert!(!tools.is_empty());
    assert!(
        !tools.iter().any(|t| t["name"] == "create_character"),
        "只读模式不应暴露写工具"
    );
    handle.stop();
}

#[test]
fn http_invalid_project_fails_fast() {
    let tmp = tempfile::tempdir().unwrap();
    let err = start_server_http(
        McpServerOptions {
            project_dir: tmp.path().to_path_buf(),
            read_only: false,
        },
        HttpServerOptions {
            port: 0,
            ..Default::default()
        },
    );
    assert!(err.is_err(), "非项目目录应启动即失败");
}

#[test]
fn http_stale_or_unknown_session_auto_recovers() {
    let (_tmp, handle) = spawn_server(false);
    let url = handle.url();
    let token = handle.token().to_string();
    let http = client();

    // 客户端使用一个未曾 initialize 的未知 session-id（模拟客户端携带服务重启前遗留的 session）
    let unknown_session_id = "00000000-0000-0000-0000-000000000001";
    let (_, called) = rpc(
        &http,
        &url,
        &token,
        Some(unknown_session_id),
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "get_project_stats", "arguments": {}}
        }),
    );
    let result = &called["result"];
    assert_eq!(result["isError"], serde_json::json!(false), "未知 session 调工具应自动恢复并成功");
    assert!(!result["content"][0]["text"].as_str().unwrap().is_empty());
    handle.stop();
}

