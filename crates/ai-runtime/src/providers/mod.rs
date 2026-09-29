//! Providers — 真实 AI Provider 实现
//!
//! 共享 HTTP 基础设施 + per-provider 的请求/响应转换。
//! v0.1 支持 Anthropic Claude 和 OpenAI GPT，非流式 + 流式。

pub mod anthropic;
pub mod openai;

pub use anthropic::AnthropicProvider;
pub use openai::{ApiMode, OpenAiProvider};

use crate::provider::{AiError, GenerateRequest, GenerateResponse, StreamEvent};

// ============================================================================
// ThinkingConfig — 跨 Provider 的思考强度抽象
// ============================================================================

/// 思考强度（跨 Provider 抽象，内部按 Provider 映射为各自的原生参数）
///
/// - OpenAI（Chat Completions）→ 顶层 `reasoning_effort`（minimal/low/medium/high）
/// - OpenAI（Responses API）→ 嵌套 `reasoning:{effort:...}`
/// - Anthropic → `thinking.budget_tokens`（按档位换算 token 预算，并强制 temperature=1.0）
///
/// 当用户的 Provider 不支持思考时，该字段会被忽略。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingConfig {
    /// 自动：不发送任何思考参数，完全跟随模型/服务端默认。
    /// 适用于必须开启思考的模型（DeepSeek-R1、GLM-Z1、QwQ、o1 等——
    /// 它们不接受 minimal 之类的关闭参数），以及拒绝 reasoning_effort 的兼容服务。
    Auto,
    /// 关闭（OpenAI → minimal；Anthropic → 不发送 thinking 字段）
    #[default]
    Off,
    /// 低
    Low,
    /// 中
    Medium,
    /// 高
    High,
    /// 显式 token 预算（仅 Anthropic 有效，会直接写入 budget_tokens）
    Budget(u32),
}

/// Anthropic 各档位对应的默认 budget_tokens
///
/// 选择依据（保守且适配现有 max_tokens 上限）：
/// - Low = 2048：快速思考，不显著增加延迟
/// - Medium = 8000：平衡，适配 fragment 续写的 4096+ max_tokens
/// - High = 24000：深度推理（实际值会被 clamp 到 max_tokens - 1）
impl ThinkingConfig {
    /// 返回该档位映射到 Anthropic 的 budget_tokens（Off / Auto / Budget 变体返回 None）
    pub fn anthropic_budget(&self) -> Option<u32> {
        match self {
            ThinkingConfig::Auto => None,
            ThinkingConfig::Off => None,
            ThinkingConfig::Low => Some(2048),
            ThinkingConfig::Medium => Some(8000),
            ThinkingConfig::High => Some(24000),
            ThinkingConfig::Budget(n) => Some(*n),
        }
    }
}

// ============================================================================
// ThinkingDialect — OpenAI 兼容服务的思考参数方言
// ============================================================================

/// 思考的「开关」与「深度」在不同服务上是**不同参数**：
///
/// - OpenAI 原生：`reasoning_effort`（深度档位；minimal≈关）
/// - 智谱 GLM：`thinking.type` = "enabled"/"disabled"（独立开关，**不识别 reasoning_effort**，
///   且 open.bigmodel.cn 默认不开启思考——只发 effort 档位会被静默忽略）
/// - 通义千问：`enable_thinking` = true/false（独立开关）
///
/// 按 API URL 自动识别；无法识别时按 OpenAI 原生处理。
/// 用户仍可用 extra_body 覆盖（优先级最高）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThinkingDialect {
    /// OpenAI 原生：reasoning_effort / reasoning.effort
    #[default]
    OpenAiEffort,
    /// 智谱 GLM（bigmodel.cn）：thinking.type 开关
    ZhipuSwitch,
    /// 通义千问（dashscope）：enable_thinking 开关
    QwenSwitch,
}

/// 按 API URL 识别思考参数方言
pub fn thinking_dialect_for_url(api_url: Option<&str>) -> ThinkingDialect {
    let url = api_url.unwrap_or_default().to_lowercase();
    if url.contains("bigmodel.cn") {
        ThinkingDialect::ZhipuSwitch
    } else if url.contains("dashscope") {
        ThinkingDialect::QwenSwitch
    } else {
        ThinkingDialect::OpenAiEffort
    }
}

// ============================================================================
// 请求超时 — 默认值与归一化
// ============================================================================

/// 生成请求超时的默认值（秒）；深度思考/长文生成可按档案调大
pub const DEFAULT_TIMEOUT_SECS: u64 = 300;
/// 超时下限（秒），防误配成过小值导致必然超时
pub const MIN_TIMEOUT_SECS: u64 = 10;
/// 超时上限（秒，2 小时）
pub const MAX_TIMEOUT_SECS: u64 = 7200;

/// 归一化用户配置的 timeout_secs：None → 默认，并 clamp 到上下限
pub(crate) fn normalize_timeout_secs(secs: Option<u64>) -> u64 {
    secs.unwrap_or(DEFAULT_TIMEOUT_SECS)
        .clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS)
}

// ============================================================================
// 共享工具
// ============================================================================

/// 将 temperature 舍入到小数点后 2 位，返回适合 JSON 序列化的 `f64`。
///
/// 某些 API（如智谱 GLM 的 Anthropic 兼容接口）要求 temperature 精度不超过 2 位小数，
/// 否则返回 400 `temperature参数非法：限制小数点[2]位`。
///
/// **必须在 f64 精度下舍入**：`serde_json` 序列化 `f32` 时会先提升为 `f64`，
/// 而 `0.7_f32 as f64` 会变成 `0.699999988079071`，即便 f32 层已舍入也无效。
/// 先 `as f64` 再舍入可保证 JSON 输出为干净的 `0.7`。
pub(crate) fn round_temperature(temp: f32) -> f64 {
    let temp = temp as f64;
    (temp * 100.0).round() / 100.0
}

/// 将 thinking 配置与 extra_body 自定义字段合并进请求体。
///
/// 优先级（后者覆盖前者）：
/// 1. adapter 构造的基础字段（已在 `body` 中）
/// 2. thinking 配置（OpenAI → reasoning_effort；Anthropic → thinking.budget_tokens + 强制 temperature=1.0）
/// 3. extra_body（来自用户自定义，覆盖一切同名字段）
///
/// `max_tokens` 用于 clamp Anthropic 的 budget_tokens（必须 < max_tokens）。
pub(crate) fn apply_overrides(
    body: &mut serde_json::Value,
    adapter: &dyn ProviderAdapter,
    max_tokens: usize,
) {
    let obj = match body.as_object_mut() {
        Some(o) => o,
        None => return,
    };
    let provider_id = adapter.id();

    // 1. thinking 配置（按 provider 映射）
    // Auto 档位不发送任何思考参数——交给模型/服务端默认行为，
    // 用于必须开启思考的模型（发 minimal 会被拒）与拒绝 reasoning_effort 的兼容服务。
    if let Some(tc) = adapter.thinking_config() {
        if !matches!(tc, ThinkingConfig::Auto) {
            match provider_id {
                "openai" => match adapter.thinking_dialect() {
                    ThinkingDialect::OpenAiEffort => {
                        let effort = match tc {
                            ThinkingConfig::Off => "minimal",
                            ThinkingConfig::Low => "low",
                            ThinkingConfig::Medium => "medium",
                            ThinkingConfig::High => "high",
                            // Budget 对 OpenAI 无意义，按 High 处理
                            ThinkingConfig::Budget(_) => "high",
                            // 上方守卫已排除 Auto（该档位不发送思考参数）
                            ThinkingConfig::Auto => unreachable!(),
                        };
                        obj.insert(
                            "reasoning_effort".to_string(),
                            serde_json::Value::String(effort.to_string()),
                        );
                    }
                    // 智谱 GLM：思考是独立开关 thinking.type，reasoning_effort 不被识别
                    ThinkingDialect::ZhipuSwitch => {
                        let enabled = !matches!(tc, ThinkingConfig::Off);
                        obj.insert(
                        "thinking".to_string(),
                        serde_json::json!({ "type": if enabled { "enabled" } else { "disabled" } }),
                    );
                    }
                    // 通义千问：思考是独立开关 enable_thinking
                    ThinkingDialect::QwenSwitch => {
                        let enabled = !matches!(tc, ThinkingConfig::Off);
                        obj.insert(
                            "enable_thinking".to_string(),
                            serde_json::Value::Bool(enabled),
                        );
                    }
                },
                "openai-responses" => {
                    // Responses API 的 reasoning 是嵌套对象：{"reasoning":{"effort":...}}
                    // 与 Chat Completions 的顶层 reasoning_effort 不同。
                    let effort = match tc {
                        ThinkingConfig::Off => "minimal",
                        ThinkingConfig::Low => "low",
                        ThinkingConfig::Medium => "medium",
                        ThinkingConfig::High => "high",
                        ThinkingConfig::Budget(_) => "high",
                        // 上方守卫已排除 Auto（该档位不发送思考参数）
                        ThinkingConfig::Auto => unreachable!(),
                    };
                    obj.insert(
                        "reasoning".to_string(),
                        serde_json::json!({ "effort": effort }),
                    );
                }
                "anthropic" => {
                    // 智谱的 Anthropic 兼容层只认 thinking.type（无 budget_tokens 概念）。
                    // GLM 默认开启思考，「不思考」必须显式 disabled 才真正关掉（省思维链 token）。
                    if adapter.thinking_dialect() == ThinkingDialect::ZhipuSwitch {
                        let enabled = !matches!(tc, ThinkingConfig::Off);
                        obj.insert(
                        "thinking".to_string(),
                        serde_json::json!({ "type": if enabled { "enabled" } else { "disabled" } }),
                    );
                        if enabled {
                            // 开思考时 temperature 强制 1.0（Anthropic 协议要求）
                            obj.insert("temperature".to_string(), serde_json::Value::from(1.0_f64));
                        }
                    } else if let Some(budget) = tc.anthropic_budget() {
                        // Anthropic 的 extended thinking：budget_tokens 与可见输出
                        // **共享 max_tokens 配额**（budget 必须 < max_tokens）。
                        // 若 max_tokens 不变（如 8192），开启 Medium(8000) 思考后
                        // 可见输出几乎为 0 → 必然触发 MaxTokens 截断 → 空响应。
                        //
                        // 修复：保留用户请求的 budget，并把 max_tokens 抬高到
                        // `原始值 + budget`，让思考与输出各有空间。
                        let budget = budget.max(1024) as usize; // Anthropic 最低 1024
                                                                // 抬高 max_tokens，确保可见输出至少保留原始配额。
                                                                // 设上限 128000（主流思考型模型的最大输出），避免 budget 过大时膨胀超限。
                        let bumped = (max_tokens + budget).min(128_000);
                        obj.insert(
                            "max_tokens".to_string(),
                            serde_json::Value::from(bumped as u64),
                        );
                        obj.insert(
                            "thinking".to_string(),
                            serde_json::json!({
                                "type": "enabled",
                                "budget_tokens": budget
                            }),
                        );
                        // Anthropic 开启 extended thinking 时强制 temperature = 1.0
                        obj.insert("temperature".to_string(), serde_json::Value::from(1.0_f64));
                    }
                }
                _ => {}
            }
        }
    }

    // 2. extra_body 深合并（用户自定义，覆盖优先级最高）
    if let Some(custom) = adapter.custom_body() {
        for (k, v) in custom {
            obj.insert(k.clone(), v.clone());
        }
    }
}

// ============================================================================
// ProviderAdapter — per-provider 的请求/响应转换抽象
// ============================================================================

/// 每个 Provider 的 API 适配器——处理请求体构建与响应解析的差异。
///
/// Anthropic 和 OpenAI 的 API 差异主要在：
/// - endpoint URL
/// - system prompt 的位置（Anthropic 独立字段，OpenAI 是 messages 第一项）
/// - 响应结构（字段名不同）
/// - 认证头
pub trait ProviderAdapter: Send + Sync {
    /// Provider 标识
    fn id(&self) -> &str;

    /// API endpoint URL（完整路径）
    fn endpoint(&self) -> String;

    /// 从环境变量读取 API Key
    fn api_key(&self) -> Result<String, AiError>;

    /// 构建请求体 JSON
    fn to_request_body(&self, request: &GenerateRequest) -> serde_json::Value;

    /// 构建额外的 HTTP 头（认证头等）
    fn extra_headers(&self) -> Vec<(String, String)>;

    /// 追加自定义 HTTP 头（由 Profile 提供，注入时不覆盖认证头）
    fn custom_headers(&self) -> &[(String, String)] {
        &[]
    }

    /// 自定义请求体字段（在 `to_request_body` 之后深合并，覆盖同名基础字段）
    fn custom_body(&self) -> Option<&serde_json::Map<String, serde_json::Value>> {
        None
    }

    /// 思考强度配置（跨 Provider 抽象映射）
    fn thinking_config(&self) -> Option<&ThinkingConfig> {
        None
    }

    /// 思考参数方言（按 API URL 识别；默认 OpenAI 原生 reasoning_effort）
    fn thinking_dialect(&self) -> ThinkingDialect {
        ThinkingDialect::OpenAiEffort
    }

    /// 单次请求超时（秒，总时长含流式读取；深度思考/长文生成需要更大值）
    fn timeout_secs(&self) -> u64 {
        300
    }

    /// 解析响应 JSON
    fn parse_response(&self, body: &serde_json::Value) -> Result<GenerateResponse, AiError>;

    /// 解析流式 SSE 数据行，返回 StreamEvent
    ///
    /// `line` 是 SSE "data:" 之后的内容（已去除 "data: " 前缀）。
    /// 返回 `None` 表示该行不包含有效数据（如 [DONE] 标记）。
    /// 返回 `Some(StreamEvent)` 表示解析到增量或错误。
    fn parse_stream_chunk(&self, line: &str) -> Option<StreamEvent>;
}

// ============================================================================
// HttpClient — 共享 HTTP 客户端
// ============================================================================

/// 执行同步 HTTP 请求的客户端（ureq，无 tokio 依赖）
// ============================================================================
// 模型列表 — GET /models（OpenAI 兼容与 Anthropic 通用）
// ============================================================================

/// 拉取 Provider 支持的模型 ID 列表（按字母序、去重）。
///
/// `api_key` 留空时读环境变量（anthropic → ANTHROPIC_API_KEY，其余 →
/// OPENAI_API_KEY）；Ollama 等本地服务无 Key 也能成功。
pub fn list_models(
    provider_name: &str,
    api_url: Option<&str>,
    api_url_full: bool,
    api_key: Option<&str>,
) -> Result<Vec<String>, AiError> {
    let is_anthropic = provider_name == "anthropic";
    let key = match api_key.map(str::trim).filter(|k| !k.is_empty()) {
        Some(k) => k.to_string(),
        None => {
            let env = if is_anthropic {
                "ANTHROPIC_API_KEY"
            } else {
                "OPENAI_API_KEY"
            };
            std::env::var(env).unwrap_or_default()
        }
    };

    let url = models_endpoint(provider_name, api_url, api_url_full);
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(20))
        .build();
    let mut req = agent.get(&url);
    if is_anthropic {
        // Anthropic 用 x-api-key + 版本头（与其 messages API 一致）
        req = req
            .set("x-api-key", &key)
            .set("anthropic-version", "2023-06-01");
    } else {
        req = req.set("Authorization", &format!("Bearer {key}"));
    }

    let resp = match req.call() {
        Ok(r) => r,
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            eprintln!("[AI] GET {url} → HTTP {code}\n响应: {body}");
            let message = parse_api_error_body(&body);
            return Err(AiError::Api {
                status: code,
                message,
            });
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("timeout") || msg.contains("deadline") {
                return Err(AiError::Network(format!("请求超时: {msg}")));
            }
            if msg.contains("connect") || msg.contains("dns") {
                return Err(AiError::Network(format!(
                    "连接失败: {msg}（检查 API URL 是否正确、本地服务是否在运行）"
                )));
            }
            return Err(AiError::Network(msg));
        }
    };

    let body: serde_json::Value = serde_json::from_str(
        &resp
            .into_string()
            .map_err(|e| AiError::Parse(format!("模型列表读取失败: {e}")))?,
    )
    .map_err(|e| AiError::Parse(format!("模型列表解析失败: {e}")))?;

    let mut ids: Vec<String> = body
        .get("data")
        .and_then(|d| d.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|m| m.get("id").and_then(|x| x.as_str()))
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// 推导模型列表端点，与各 Provider 的 `endpoint()` 拼接规则保持一致：
/// - 无 api_url / base 模式：`{base}/v1/models`（anthropic 与 openai 同构）
/// - 完整 URL 模式：剥掉尾部的 chat/responses/messages 方法路径后，
///   原本带 `/v1/` 的回到 `/v1/models`，只有版本段（如 `/v4`）的接 `/models`
pub fn models_endpoint(provider_name: &str, api_url: Option<&str>, api_url_full: bool) -> String {
    let is_anthropic = provider_name == "anthropic";
    let default_base = if is_anthropic {
        "https://api.anthropic.com"
    } else {
        "https://api.openai.com"
    };

    let base = match api_url.map(str::trim).filter(|u| !u.is_empty()) {
        Some(u) => {
            let b = u.trim_end_matches('/').to_string();
            if api_url_full
                || b.ends_with("/chat/completions")
                || b.ends_with("/responses")
                || b.ends_with("/messages")
            {
                // 剥掉 /v1/ 前缀的方法路径 → 恢复为 {base}/v1/models
                for suffix in ["/v1/chat/completions", "/v1/responses", "/v1/messages"] {
                    if let Some(s) = b.strip_suffix(suffix) {
                        return format!("{}/v1/models", s.trim_end_matches('/'));
                    }
                }
                // 剥掉裸方法路径（URL 自带版本段，如 智谱 /v4）→ {版本段}/models
                for suffix in ["/chat/completions", "/responses", "/messages"] {
                    if let Some(s) = b.strip_suffix(suffix) {
                        return format!("{}/models", s.trim_end_matches('/'));
                    }
                }
                // api_url_full 但不是已知方法路径：原样接 /models
                return format!("{b}/models");
            }
            // base 已带 /v1（常见手填写法）先剥掉，与 chat 端点归一化一致
            b.strip_suffix("/v1").map(str::to_string).unwrap_or(b)
        }
        None => default_base.to_string(),
    };
    // base 模式与 chat 端点同前缀：chat 是 {base}/v1/chat/completions，
    // 模型列表就是 {base}/v1/models（DeepSeek 等只认 /v1/models）
    format!("{base}/v1/models")
}

pub struct HttpClient;

impl std::fmt::Debug for HttpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpClient").finish()
    }
}

impl HttpClient {
    pub fn new() -> Self {
        Self
    }

    pub fn send(
        &self,
        adapter: &dyn ProviderAdapter,
        request: &GenerateRequest,
    ) -> Result<GenerateResponse, AiError> {
        let api_key = adapter.api_key()?;
        let mut body = adapter.to_request_body(request);
        // 合并 thinking 配置与 extra_body 自定义字段（优先级：基础 < thinking < extra_body）
        apply_overrides(&mut body, adapter, request.max_tokens);

        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(adapter.timeout_secs()))
            .build();

        let endpoint = adapter.endpoint();
        let mut req = agent
            .post(&endpoint)
            .set("Content-Type", "application/json");

        for (key, value) in adapter.extra_headers() {
            req = req.set(&key, &value);
        }

        if !adapter
            .extra_headers()
            .iter()
            .any(|(k, _)| k == "x-api-key")
        {
            req = req.set("Authorization", &format!("Bearer {api_key}"));
        }

        // 追加用户自定义头（跳过认证头，避免误覆盖）
        for (key, value) in adapter.custom_headers() {
            let lower = key.to_lowercase();
            if lower == "authorization" || lower == "x-api-key" {
                continue;
            }
            req = req.set(key, value);
        }

        let body_str = serde_json::to_string(&body)
            .map_err(|e| AiError::Parse(format!("请求序列化失败: {e}")))?;

        let resp = match req.send_string(&body_str) {
            Ok(r) => r,
            Err(ureq::Error::Status(code, r)) => {
                let body = r.into_string().unwrap_or_default();
                eprintln!("[AI] POST {endpoint} → HTTP {code}\n响应: {body}");
                let message = parse_api_error_body(&body);
                return Err(AiError::Api {
                    status: code,
                    message,
                });
            }
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("timeout") || msg.contains("deadline") {
                    return Err(AiError::Network(format!("请求超时: {msg}")));
                }
                if msg.contains("connect") || msg.contains("dns") {
                    return Err(AiError::Network(format!("连接失败: {msg}")));
                }
                return Err(AiError::Network(format!("HTTP 错误: {msg}")));
            }
        };

        let status = resp.status();
        let response_text = resp
            .into_string()
            .map_err(|e| AiError::Network(format!("读取响应失败: {e}")))?;

        if status != 200 && status != 201 {
            eprintln!("[AI] POST {endpoint} → HTTP {status}\n响应: {response_text}");
            let message = parse_api_error_body(&response_text);
            if status == 401 || status == 403 {
                return Err(AiError::Auth);
            }
            return Err(AiError::Api { status, message });
        }

        let body: serde_json::Value = serde_json::from_str(&response_text).map_err(|e| {
            AiError::Parse(format!("响应 JSON 解析失败: {e}\n原文: {response_text}"))
        })?;

        adapter.parse_response(&body)
    }
}

impl Default for HttpClient {
    fn default() -> Self {
        Self
    }
}

// ============================================================================
// HttpStreamClient — 异步流式 HTTP 客户端
// ============================================================================

/// 使用异步 reqwest 实现的流式 HTTP 客户端。
///
/// SSE (Server-Sent Events) 格式解析：
/// - `data: {...}` → 传递给 adapter.parse_stream_chunk()
/// - `data: [DONE]` → 结束标记
/// - 空行 → 忽略
pub struct HttpStreamClient;

impl HttpStreamClient {
    /// 发送流式请求，通过 on_event 回调推送每个 StreamEvent。
    ///
    /// 此方法是 async 的，应在 tokio runtime 中调用。
    pub async fn stream(
        adapter: &(dyn ProviderAdapter + Sync),
        request: &GenerateRequest,
        on_event: &(dyn Fn(StreamEvent) + Send + Sync),
    ) -> Result<(), AiError> {
        let api_key = adapter.api_key()?;
        let mut body = adapter.to_request_body(request);
        // 合并 thinking 配置与 extra_body 自定义字段（优先级：基础 < thinking < extra_body）
        apply_overrides(&mut body, adapter, request.max_tokens);

        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(30))
            .timeout(std::time::Duration::from_secs(adapter.timeout_secs()))
            .build()
            .map_err(|e| AiError::Network(format!("创建 HTTP 客户端失败: {e}")))?;

        let endpoint = adapter.endpoint();
        let mut req_builder = client
            .post(&endpoint)
            .header("Content-Type", "application/json");

        for (key, value) in adapter.extra_headers() {
            req_builder = req_builder.header(key, value);
        }

        if !adapter
            .extra_headers()
            .iter()
            .any(|(k, _)| k == "x-api-key")
        {
            req_builder = req_builder.header("Authorization", format!("Bearer {api_key}"));
        }

        // 追加用户自定义头（跳过认证头，避免误覆盖）
        for (key, value) in adapter.custom_headers() {
            let lower = key.to_lowercase();
            if lower == "authorization" || lower == "x-api-key" {
                continue;
            }
            req_builder = req_builder.header(key, value);
        }

        // Streaming request body needs the stream flag
        let mut body_map = match body.as_object() {
            Some(obj) => obj.clone(),
            None => {
                return Err(AiError::Network("请求体不是 JSON object".to_string()));
            }
        };
        body_map.insert("stream".to_string(), serde_json::Value::Bool(true));

        let resp = req_builder.json(&body_map).send().await.map_err(|e| {
            if e.is_timeout() {
                AiError::Network(format!("请求超时: {e}"))
            } else if e.is_connect() {
                AiError::Network(format!("连接失败: {e}"))
            } else {
                AiError::Network(format!("HTTP 错误: {e}"))
            }
        })?;

        let status = resp.status();
        if !status.is_success() {
            let response_text = resp
                .text()
                .await
                .unwrap_or_else(|_| "无法读取响应".to_string());
            eprintln!("[AI] STREAM POST {endpoint} → HTTP {status}\n响应: {response_text}");
            let message = serde_json::from_str::<serde_json::Value>(&response_text)
                .ok()
                .and_then(|v| {
                    v.get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(|m| m.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| {
                            v.get("error")
                                .and_then(|e| e.as_str())
                                .map(|s| s.to_string())
                        })
                })
                .unwrap_or(response_text.clone());

            if status.as_u16() == 401 || status.as_u16() == 403 {
                return Err(AiError::Auth);
            }
            return Err(AiError::Api {
                status: status.as_u16(),
                message,
            });
        }

        // Read SSE stream
        use futures::StreamExt;
        let mut stream = resp.bytes_stream();
        // 累积原始字节：HTTP chunk 的边界由网络层决定，可能与 UTF-8 字符边界不对齐。
        // 必须保留不完整的尾部字节，留待下一个 chunk 拼接后再解码，
        // 否则中文字符（3 字节）被切断后会变成 U+FFFD（���）。
        let mut byte_buffer: Vec<u8> = Vec::new();
        // 跨 chunk 维护未结束的 SSE 行。
        let mut line_buffer = String::new();

        while let Some(chunk_result) = stream.next().await {
            let chunk = match chunk_result {
                Ok(c) => c,
                Err(e) => {
                    on_event(StreamEvent::Error {
                        message: format!("流式读取错误: {e}"),
                    });
                    return Err(AiError::Network(format!("流式读取错误: {e}")));
                }
            };

            byte_buffer.extend_from_slice(&chunk);

            // 只解码到最后一个完整的 UTF-8 字符边界，末尾不完整的字节留在 byte_buffer 里。
            let safe_len = floor_utf8_boundary(&byte_buffer);
            if safe_len == 0 && !byte_buffer.is_empty() {
                // 整段都无法解码（开头即非法字节，非 chunk 边界问题）。
                // 降级为有损解码并清空，避免 buffer 无限增长导致死循环。
                line_buffer.push_str(&String::from_utf8_lossy(&byte_buffer));
                byte_buffer.clear();
            } else if safe_len > 0 {
                // floor_utf8_boundary 已保证 buf[..safe_len] 是合法 UTF-8。
                let decoded = std::str::from_utf8(&byte_buffer[..safe_len])
                    .expect("floor_utf8_boundary 保证安全前缀可解码");
                line_buffer.push_str(decoded);
                byte_buffer = byte_buffer[safe_len..].to_vec();
            }

            // Process complete lines from buffer
            while let Some(line_end) = line_buffer.find('\n') {
                let line = line_buffer[..line_end].trim().to_string();
                line_buffer = line_buffer[line_end + 1..].to_string();

                if line.is_empty() {
                    continue;
                }

                // SSE format: "data: <json>"
                if let Some(data) = line.strip_prefix("data: ") {
                    let data = data.trim();
                    if data == "[DONE]" {
                        // Stream complete — adapter should have already tracked full_text
                        continue;
                    }
                    if let Some(event) = adapter.parse_stream_chunk(data) {
                        on_event(event);
                    }
                }
            }
        }

        Ok(())
    }
}

// ============================================================================
// 辅助：UTF-8 字节缓冲的安全解码边界
// ============================================================================

/// 返回 `buf` 中最后一个完整 UTF-8 字符的结束字节位置。
///
/// SSE/HTTP chunk 的边界可能与 UTF-8 字符边界不对齐（中文占 3 字节）。
/// 直接对每个 chunk 调用 `String::from_utf8_lossy` 会把切断的尾部字节
/// 替换成 U+FFFD（���）并丢弃，造成永久乱码。
/// 本函数找到一个安全的前缀长度，使 `buf[..safe_len]` 恰好是若干完整字符，
/// 剩余 `buf[safe_len..]` 留待下一个 chunk 拼接后再解码。
fn floor_utf8_boundary(buf: &[u8]) -> usize {
    if buf.is_empty() {
        return 0;
    }
    match std::str::from_utf8(buf) {
        Ok(_) => buf.len(),
        // valid_up_to() 返回开头连续合法字节数，恰好是安全解码边界。
        Err(e) => e.valid_up_to(),
    }
}

// ============================================================================
// 辅助：从环境变量读 Key
// ============================================================================

/// 从 API 错误响应体提取可读错误消息
fn parse_api_error_body(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| {
                    e.get("message")
                        .and_then(|m| m.as_str())
                        .or_else(|| e.as_str())
                })
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| {
            // 按字节切片 body[..300] 会切断多字节字符（如中文）并 panic；
            // floor_char_boundary 返回 ≤ 300 的最大字符边界，保证安全。
            let limit = 300;
            if body.len() > limit {
                body[..body.floor_char_boundary(limit)].to_string()
            } else {
                body.to_string()
            }
        })
}

/// 从环境变量读取 API Key，不存在则返回 Auth 错误
pub fn api_key_from_env(var_name: &str) -> Result<String, AiError> {
    std::env::var(var_name).map_err(|_| AiError::Auth)
}

#[cfg(test)]
mod tests {
    use super::{apply_overrides, floor_utf8_boundary, round_temperature, ThinkingConfig};
    use crate::provider::{AiError, GenerateResponse};

    /// 测试用的最小 ProviderAdapter 实现
    struct MockAdapter {
        id: &'static str,
        thinking: Option<ThinkingConfig>,
        extra_body: Option<serde_json::Value>,
        dialect: Option<super::ThinkingDialect>,
    }

    impl super::ProviderAdapter for MockAdapter {
        fn id(&self) -> &str {
            self.id
        }
        fn endpoint(&self) -> String {
            String::new()
        }
        fn api_key(&self) -> Result<String, AiError> {
            Ok(String::new())
        }
        fn to_request_body(
            &self,
            _request: &crate::provider::GenerateRequest,
        ) -> serde_json::Value {
            serde_json::json!({})
        }
        fn extra_headers(&self) -> Vec<(String, String)> {
            vec![]
        }
        fn custom_body(&self) -> Option<&serde_json::Map<String, serde_json::Value>> {
            self.extra_body.as_ref().and_then(|v| v.as_object())
        }
        fn thinking_config(&self) -> Option<&ThinkingConfig> {
            self.thinking.as_ref()
        }
        fn thinking_dialect(&self) -> super::ThinkingDialect {
            self.dialect.unwrap_or_default()
        }
        fn parse_response(&self, _body: &serde_json::Value) -> Result<GenerateResponse, AiError> {
            unreachable!()
        }
        fn parse_stream_chunk(&self, _line: &str) -> Option<crate::provider::StreamEvent> {
            None
        }
    }

    #[test]
    fn apply_overrides_auto_sends_no_thinking_params() {
        // Auto：不发送任何思考参数，跟随模型默认。
        // 思考型模型（R1/GLM-Z1/QwQ）不接受 minimal，兼容服务可能拒绝 reasoning_effort。
        for id in ["openai", "openai-responses", "anthropic"] {
            let adapter = MockAdapter {
                id,
                thinking: Some(ThinkingConfig::Auto),
                extra_body: None,
                dialect: None,
            };
            let mut body = serde_json::json!({"model": "deepseek-reasoner", "temperature": 0.7, "max_tokens": 8192});
            apply_overrides(&mut body, &adapter, 8192);
            assert!(
                body.get("reasoning_effort").is_none(),
                "{id} Auto 不应发送 reasoning_effort"
            );
            assert!(
                body.get("reasoning").is_none(),
                "{id} Auto 不应发送 reasoning"
            );
            assert!(
                body.get("thinking").is_none(),
                "{id} Auto 不应发送 thinking"
            );
            // 不应因思考改动 temperature / max_tokens
            assert!((body["temperature"].as_f64().unwrap() - 0.7).abs() < 0.001);
            assert_eq!(body["max_tokens"].as_u64(), Some(8192));
        }
    }

    #[test]
    fn apply_overrides_zhipu_dialect_uses_native_thinking_switch() {
        // 智谱 GLM：思考是独立开关 thinking.type，reasoning_effort 不被识别
        for (config, expected) in [
            (ThinkingConfig::Off, "disabled"),
            (ThinkingConfig::Low, "enabled"),
            (ThinkingConfig::Medium, "enabled"),
            (ThinkingConfig::High, "enabled"),
        ] {
            let adapter = MockAdapter {
                id: "openai",
                thinking: Some(config.clone()),
                extra_body: None,
                dialect: Some(super::ThinkingDialect::ZhipuSwitch),
            };
            let mut body = serde_json::json!({"model": "glm-4.6"});
            apply_overrides(&mut body, &adapter, 8192);
            assert_eq!(
                body["thinking"]["type"].as_str(),
                Some(expected),
                "智谱 thinking={:?} 应映射为 thinking.type={expected}",
                config
            );
            assert!(
                body.get("reasoning_effort").is_none(),
                "智谱不应发 reasoning_effort"
            );
        }
    }

    #[test]
    fn apply_overrides_qwen_dialect_uses_enable_thinking() {
        // 通义千问：思考是独立开关 enable_thinking
        for (config, expected) in [(ThinkingConfig::Off, false), (ThinkingConfig::Medium, true)] {
            let adapter = MockAdapter {
                id: "openai",
                thinking: Some(config.clone()),
                extra_body: None,
                dialect: Some(super::ThinkingDialect::QwenSwitch),
            };
            let mut body = serde_json::json!({"model": "qwen-plus"});
            apply_overrides(&mut body, &adapter, 8192);
            assert_eq!(
                body["enable_thinking"].as_bool(),
                Some(expected),
                "通义 thinking={:?} 应映射为 enable_thinking={expected}",
                config
            );
            assert!(body.get("reasoning_effort").is_none());
        }
    }

    #[test]
    fn apply_overrides_anthropic_zhipu_sends_type_only() {
        // 智谱 Anthropic 兼容层：只发 thinking.type，不带 budget_tokens、不抬高 max_tokens
        let adapter = MockAdapter {
            id: "anthropic",
            thinking: Some(ThinkingConfig::Medium),
            extra_body: None,
            dialect: Some(super::ThinkingDialect::ZhipuSwitch),
        };
        let mut body =
            serde_json::json!({"model": "glm-4.6", "temperature": 0.7, "max_tokens": 8192});
        apply_overrides(&mut body, &adapter, 8192);
        assert_eq!(body["thinking"]["type"].as_str(), Some("enabled"));
        assert!(
            body["thinking"].get("budget_tokens").is_none(),
            "智谱兼容层不应带 budget_tokens"
        );
        assert!((body["temperature"].as_f64().unwrap() - 1.0).abs() < 0.001);
        assert_eq!(
            body["max_tokens"].as_u64(),
            Some(8192),
            "智谱路径不应抬高 max_tokens"
        );
    }

    #[test]
    fn apply_overrides_anthropic_zhipu_off_disables_thinking() {
        // GLM 默认开启思考，「不思考」必须显式 disabled 才真正关掉（省思维链 token），
        // 且关思考时不强制 temperature=1.0
        let adapter = MockAdapter {
            id: "anthropic",
            thinking: Some(ThinkingConfig::Off),
            extra_body: None,
            dialect: Some(super::ThinkingDialect::ZhipuSwitch),
        };
        let mut body =
            serde_json::json!({"model": "glm-4.6", "temperature": 0.7, "max_tokens": 8192});
        apply_overrides(&mut body, &adapter, 8192);
        assert_eq!(body["thinking"]["type"].as_str(), Some("disabled"));
        assert!((body["temperature"].as_f64().unwrap() - 0.7).abs() < 0.001);
    }

    #[test]
    fn thinking_dialect_detected_from_url() {
        use super::{thinking_dialect_for_url, ThinkingDialect};
        assert_eq!(
            thinking_dialect_for_url(Some(
                "https://open.bigmodel.cn/api/paas/v4/chat/completions"
            )),
            ThinkingDialect::ZhipuSwitch
        );
        assert_eq!(
            thinking_dialect_for_url(Some("https://dashscope.aliyuncs.com/compatible-mode")),
            ThinkingDialect::QwenSwitch
        );
        assert_eq!(
            thinking_dialect_for_url(Some("https://api.deepseek.com")),
            ThinkingDialect::OpenAiEffort
        );
        assert_eq!(
            thinking_dialect_for_url(None),
            ThinkingDialect::OpenAiEffort
        );
    }

    #[test]
    fn apply_overrides_openai_thinking_maps_to_reasoning_effort() {
        for (config, expected) in [
            (ThinkingConfig::Off, "minimal"),
            (ThinkingConfig::Low, "low"),
            (ThinkingConfig::Medium, "medium"),
            (ThinkingConfig::High, "high"),
        ] {
            let adapter = MockAdapter {
                id: "openai",
                dialect: None,
                thinking: Some(config.clone()),
                extra_body: None,
            };
            let mut body = serde_json::json!({"model": "o3", "temperature": 0.7});
            apply_overrides(&mut body, &adapter, 8192);
            assert_eq!(
                body["reasoning_effort"].as_str(),
                Some(expected),
                "OpenAI thinking={:?} 应映射为 reasoning_effort={expected}",
                config
            );
            // OpenAI 不应改动 temperature
            assert!((body["temperature"].as_f64().unwrap() - 0.7).abs() < 0.001);
        }
    }

    #[test]
    fn apply_overrides_responses_thinking_maps_to_nested_reasoning() {
        // Responses API 的 reasoning 是嵌套对象 {effort:...}，而非顶层 reasoning_effort
        for (config, expected) in [
            (ThinkingConfig::Off, "minimal"),
            (ThinkingConfig::Low, "low"),
            (ThinkingConfig::Medium, "medium"),
            (ThinkingConfig::High, "high"),
        ] {
            let adapter = MockAdapter {
                id: "openai-responses",
                dialect: None,
                thinking: Some(config.clone()),
                extra_body: None,
            };
            let mut body = serde_json::json!({"model": "gpt-5", "temperature": 0.7});
            apply_overrides(&mut body, &adapter, 8192);
            // 嵌套 reasoning.effort
            assert_eq!(
                body["reasoning"]["effort"].as_str(),
                Some(expected),
                "Responses thinking={:?} 应映射为 reasoning.effort={expected}",
                config
            );
            // 不应有顶层 reasoning_effort（那是 Chat 模式的字段）
            assert!(body.get("reasoning_effort").is_none());
            // 不应改动 temperature
            assert!((body["temperature"].as_f64().unwrap() - 0.7).abs() < 0.001);
        }
    }

    #[test]
    fn apply_overrides_anthropic_thinking_forces_temperature_one() {
        let adapter = MockAdapter {
            id: "anthropic",
            dialect: None,
            thinking: Some(ThinkingConfig::Medium),
            extra_body: None,
        };
        let mut body = serde_json::json!({"model": "claude-sonnet-4-5", "temperature": 0.7, "max_tokens": 8192});
        apply_overrides(&mut body, &adapter, 8192);
        // thinking 字段应被注入
        assert_eq!(body["thinking"]["type"].as_str(), Some("enabled"));
        assert_eq!(body["thinking"]["budget_tokens"].as_u64(), Some(8000));
        // temperature 必须被强制为 1.0
        assert!((body["temperature"].as_f64().unwrap() - 1.0).abs() < 0.001);
    }

    #[test]
    fn apply_overrides_anthropic_thinking_bumps_max_tokens() {
        // 开启 thinking 时，budget 不再被 clamp 到 max_tokens - 1，
        // 而是 max_tokens 被抬高到 原值 + budget，让思考与输出各有空间。
        let adapter = MockAdapter {
            id: "anthropic",
            dialect: None,
            thinking: Some(ThinkingConfig::High), // 24000
            extra_body: None,
        };
        let mut body = serde_json::json!({"max_tokens": 4096});
        apply_overrides(&mut body, &adapter, 4096);
        // budget 保持原值
        assert_eq!(body["thinking"]["budget_tokens"].as_u64(), Some(24000));
        // max_tokens 抬高到 4096 + 24000
        assert_eq!(body["max_tokens"].as_u64(), Some(4096 + 24000));
        // temperature 强制为 1.0
        assert!((body["temperature"].as_f64().unwrap() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn apply_overrides_anthropic_budget_explicit() {
        let adapter = MockAdapter {
            id: "anthropic",
            dialect: None,
            thinking: Some(ThinkingConfig::Budget(5000)),
            extra_body: None,
        };
        let mut body = serde_json::json!({"max_tokens": 8192});
        apply_overrides(&mut body, &adapter, 8192);
        // budget 保持请求值
        assert_eq!(body["thinking"]["budget_tokens"].as_u64(), Some(5000));
        // max_tokens 抬高到 8192 + 5000
        assert_eq!(body["max_tokens"].as_u64(), Some(8192 + 5000));
    }

    #[test]
    fn apply_overrides_anthropic_budget_floor_1024() {
        // budget 低于 1024 时应被抬到 1024（Anthropic 最低要求）
        let adapter = MockAdapter {
            id: "anthropic",
            dialect: None,
            thinking: Some(ThinkingConfig::Budget(500)),
            extra_body: None,
        };
        let mut body = serde_json::json!({"max_tokens": 8192});
        apply_overrides(&mut body, &adapter, 8192);
        assert_eq!(body["thinking"]["budget_tokens"].as_u64(), Some(1024));
        // max_tokens = 8192 + 1024（floor 后的 budget）
        assert_eq!(body["max_tokens"].as_u64(), Some(8192 + 1024));
    }

    #[test]
    fn apply_overrides_extra_body_overrides_thinking() {
        // extra_body 优先级最高，应覆盖 thinking 映射的字段
        let adapter = MockAdapter {
            id: "openai",
            dialect: None,
            thinking: Some(ThinkingConfig::High),
            extra_body: Some(serde_json::json!({"reasoning_effort": "low"})),
        };
        let mut body = serde_json::json!({"temperature": 0.7});
        apply_overrides(&mut body, &adapter, 8192);
        // thinking 先写入 high，extra_body 再覆盖为 low
        assert_eq!(body["reasoning_effort"].as_str(), Some("low"));
    }

    #[test]
    fn apply_overrides_no_thinking_no_extra_body_is_noop() {
        let adapter = MockAdapter {
            id: "openai",
            dialect: None,
            thinking: None,
            extra_body: None,
        };
        let mut body = serde_json::json!({"model": "gpt-4o", "temperature": 0.5});
        apply_overrides(&mut body, &adapter, 8192);
        assert_eq!(body["model"].as_str(), Some("gpt-4o"));
        assert!((body["temperature"].as_f64().unwrap() - 0.5).abs() < 0.001);
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn apply_overrides_anthropic_off_does_not_inject_thinking() {
        let adapter = MockAdapter {
            id: "anthropic",
            dialect: None,
            thinking: Some(ThinkingConfig::Off),
            extra_body: None,
        };
        let mut body = serde_json::json!({"temperature": 0.3, "max_tokens": 4096});
        apply_overrides(&mut body, &adapter, 4096);
        // Off 时不应注入 thinking，也不应改 temperature
        assert!(body.get("thinking").is_none());
        assert!((body["temperature"].as_f64().unwrap() - 0.3).abs() < 0.001);
    }

    #[test]
    fn round_temperature_truncates_to_two_decimals() {
        assert!((round_temperature(0.7) - 0.7).abs() < 0.001);
        assert!((round_temperature(0.8) - 0.8).abs() < 0.001);
        // 超过 2 位小数的值应被舍入
        assert!((round_temperature(0.234_567) - 0.23).abs() < 0.001);
        assert!((round_temperature(0.876) - 0.88).abs() < 0.001);
        assert!((round_temperature(0.0) - 0.0).abs() < 0.001);
        assert!((round_temperature(1.0) - 1.0).abs() < 0.001);
    }

    /// 关键回归测试：serde_json 序列化后 temperature 字段必须只含 ≤2 位小数，
    /// 不能出现 `0.699999988079071` 这种 f32→f64 展开噪声（GLM 会拒绝）。
    #[test]
    fn round_temperature_serializes_cleanly_in_json() {
        for &raw in &[0.7_f32, 0.3_f32, 0.8_f32, 0.2_f32, 0.6_f32] {
            let json = serde_json::json!({ "temperature": round_temperature(raw) });
            let s = serde_json::to_string(&json).unwrap();
            let val_str = s
                .trim_start_matches("{\"temperature\":")
                .trim_end_matches('}');
            // 解析回 f64 检查：舍入后的值与原始期望值误差 < 0.001
            let val: f64 = val_str.parse().unwrap();
            assert!(
                (val - raw as f64).abs() < 0.001,
                "temperature {s} 偏离期望值 {raw}"
            );
            // 字符串形式最多 2 位小数（不含科学计数法）
            assert!(!val_str.contains('e'), "temperature 出现科学计数法: {s}");
            let frac = val_str.split('.').nth(1).unwrap_or("");
            assert!(frac.len() <= 2, "temperature 小数位超过 2 位: {s}");
        }
    }

    /// 回归测试：UTF-8 字节缓冲的安全解码边界。
    /// 这是修复中文流式输出乱码（U+FFFD / ���）的核心——chunk 边界
    /// 落在汉字（3 字节）中间时，floor_utf8_boundary 必须回退到字符起始，
    /// 让切断的尾部字节留到下一个 chunk 拼接后再解码。
    #[test]
    fn floor_utf8_boundary_finds_safe_prefix() {
        // 空缓冲
        assert_eq!(floor_utf8_boundary(b""), 0);
        // 纯 ASCII，全部可解码
        assert_eq!(floor_utf8_boundary(b"data: hello\n"), 12);
        // 完整的中文（每个字符 3 字节），全部可解码
        let full = "苏格在屋里";
        assert_eq!(floor_utf8_boundary(full.as_bytes()), full.len());
    }

    #[test]
    fn floor_utf8_boundary_handles_split_multibyte() {
        // 模拟 chunk 在「屋」(E5 B1 8B) 中间断开：只有前 1 字节 E5。
        // safe_len 应为 9（「苏格在」3 个完整字符），尾部 E5 留待下一轮。
        let s = "苏格在屋";
        let bytes = s.as_bytes();
        let cut_at = bytes.len() - 2; // 砍掉「屋」的后 2 字节
        let chunk1 = &bytes[..cut_at];
        assert_eq!(cut_at, 10); // 3*3 + 1 = 10
        let safe = floor_utf8_boundary(chunk1);
        assert_eq!(
            safe, 9,
            "切断的字节应回退到「在」之后(9)，而非保留半截「屋」"
        );
        // 安全前缀必须是合法 UTF-8，且不含替换字符
        let decoded = std::str::from_utf8(&chunk1[..safe]).unwrap();
        assert_eq!(decoded, "苏格在");
        assert!(!decoded.contains('\u{FFFD}'));
    }

    #[test]
    fn floor_utf8_boundary_handles_trailing_partial_bytes() {
        // 尾部连续多个不完整序列（模拟极端 chunk 切分）
        let s = "铜板酒馆成了";
        let bytes = s.as_bytes();
        // 切到「了」的中间，只留前 1 字节；前缀应为 5 个完整字符「铜板酒馆成」。
        let cut = bytes.len() - 2;
        let chunk = &bytes[..cut];
        let safe = floor_utf8_boundary(chunk);
        let decoded = std::str::from_utf8(&chunk[..safe]).unwrap();
        assert_eq!(decoded, "铜板酒馆成");
        assert!(!decoded.contains('\u{FFFD}'));
    }
}

#[cfg(test)]
mod models_tests {
    use super::*;

    #[test]
    fn models_endpoint_default_bases() {
        assert_eq!(
            models_endpoint("openai", None, false),
            "https://api.openai.com/v1/models"
        );
        assert_eq!(
            models_endpoint("anthropic", None, false),
            "https://api.anthropic.com/v1/models"
        );
    }

    #[test]
    fn models_endpoint_base_mode_appends_v1() {
        // DeepSeek 等：给 base，chat 走 /v1/chat/completions，模型列表同构
        assert_eq!(
            models_endpoint("openai", Some("https://api.deepseek.com"), false),
            "https://api.deepseek.com/v1/models"
        );
        // 带版本段的 base（如 dashscope compatible-mode）
        assert_eq!(
            models_endpoint(
                "openai",
                Some("https://dashscope.aliyuncs.com/compatible-mode"),
                false
            ),
            "https://dashscope.aliyuncs.com/compatible-mode/v1/models"
        );
    }

    #[test]
    fn models_endpoint_full_url_mode() {
        // 剥 /v1/chat/completions → /v1/models
        assert_eq!(
            models_endpoint(
                "openai",
                Some("https://openrouter.ai/api/v1/chat/completions"),
                true
            ),
            "https://openrouter.ai/api/v1/models"
        );
        // 剥 /chat/completions（版本段自带 v4）→ /models
        assert_eq!(
            models_endpoint(
                "openai",
                Some("https://open.bigmodel.cn/api/paas/v4/chat/completions"),
                true
            ),
            "https://open.bigmodel.cn/api/paas/v4/models"
        );
        // anthropic 完整 messages URL
        assert_eq!(
            models_endpoint(
                "anthropic",
                Some("https://api.anthropic.com/v1/messages"),
                true
            ),
            "https://api.anthropic.com/v1/models"
        );
    }

    #[test]
    fn models_endpoint_normalizes_trailing_v1() {
        // 手填常带 /v1 结尾：必须归一为单段 /v1/models，而非 /v1/v1/models
        assert_eq!(
            models_endpoint("openai", Some("https://api.deepseek.com/v1"), false),
            "https://api.deepseek.com/v1/models"
        );
        assert_eq!(
            models_endpoint("anthropic", Some("https://api.anthropic.com/v1"), false),
            "https://api.anthropic.com/v1/models"
        );
    }

    #[test]
    fn models_endpoint_tolerates_trailing_slash() {
        assert_eq!(
            models_endpoint("openai", Some("https://api.deepseek.com/"), false),
            "https://api.deepseek.com/v1/models"
        );
    }
}
