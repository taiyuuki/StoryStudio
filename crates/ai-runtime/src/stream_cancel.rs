//! 流式生成的可取消包装（独立于 Agent 会话使用）。
//!
//! Agent 内部有自己的 `stream_with_cancel`（会话 + 工具循环语境）；
//! 逐章生文流水线等无会话调用方需要同样的能力：长时间流式生成期间
//! 响应取消、并把 StreamEvent 原样转发给调用方。此模块提供该公共包装。
//!
//! 实现与 agent.rs 相同的 Arc + 'static 线程 + mpsc 轮询模式：
//! `stream_generate` 放独立线程执行，主线程 100ms 轮询 cancel_token，
//! 取消时立即返回 `AiError::Cancelled`，不等待后台线程（它自然结束后
//! 结果被丢弃）。因此 provider 以 `Arc` 传入以获得 'static 生命周期。

use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::Duration;

use crate::cancel::CancelToken;
use crate::provider::{AiError, AiProvider, GenerateRequest, GenerateResponse, StreamEvent};

/// 可取消的流式生成。
///
/// - 所有 `StreamEvent`（Delta / Done / Error / ToolUse*）转发给 `on_event`；
/// - 返回聚合后的 `GenerateResponse`（全文 + usage + stop_reason）；
/// - `cancel_token` 触发时立即返回 `Err(AiError::Cancelled)`，已生成的
///   增量已经通过 `on_event` 交给调用方，由调用方决定如何保存半成品。
///
/// `provider` 与 `on_event` 均以 `Arc` 传入：生成线程是 'static 的，
/// 取消返回时不等待它结束。
pub fn stream_generate_with_cancel(
    provider: Arc<dyn AiProvider>,
    request: &GenerateRequest,
    cancel_token: &CancelToken,
    on_event: Arc<dyn Fn(StreamEvent) + Send + Sync>,
) -> Result<GenerateResponse, AiError> {
    let request = request.clone();
    let (tx, rx) = mpsc::channel::<Result<GenerateResponse, AiError>>();

    std::thread::spawn(move || {
        let full_text = std::sync::Mutex::new(String::new());
        let stream_error = std::sync::Mutex::new(None::<AiError>);
        let stream_usage = std::sync::Mutex::new(crate::provider::TokenUsage::default());
        let final_stop_reason = std::sync::Mutex::new(None::<crate::provider::StopReason>);

        let result = provider.stream_generate(&request, &|event| {
            on_event(event.clone());
            match event {
                StreamEvent::Delta { delta } => {
                    if let Ok(mut t) = full_text.lock() {
                        t.push_str(&delta);
                    }
                }
                StreamEvent::Done {
                    usage, stop_reason, ..
                } => {
                    if let Ok(mut u) = stream_usage.lock() {
                        *u = usage;
                    }
                    if let Some(sr) = stop_reason {
                        if let Ok(mut r) = final_stop_reason.lock() {
                            *r = Some(sr);
                        }
                    }
                }
                StreamEvent::Error { message } => {
                    if let Ok(mut e) = stream_error.lock() {
                        *e = Some(AiError::Network(message));
                    }
                }
                _ => {}
            }
        });

        let response = result.and_then(|_| {
            if let Some(err) = stream_error.lock().unwrap().take() {
                return Err(err);
            }
            Ok(GenerateResponse {
                text: full_text.lock().unwrap().clone(),
                usage: stream_usage.lock().unwrap().clone(),
                stop_reason: final_stop_reason
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or(crate::provider::StopReason::EndTurn),
                tool_calls: Vec::new(),
            })
        });
        let _ = tx.send(response);
    });

    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(result) => return result,
            Err(RecvTimeoutError::Timeout) => {
                if cancel_token.is_cancelled() {
                    return Err(AiError::Cancelled);
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(AiError::Network(
                    "stream_generate 线程异常退出（channel 断开）".to_string(),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{GenerateResponse, StopReason, TokenUsage, ToolDef};

    /// 立即完成的 mock provider：一次 Delta + Done
    #[derive(Debug)]
    struct ImmediateProvider;

    impl AiProvider for ImmediateProvider {
        fn id(&self) -> &str {
            "mock-immediate"
        }
        fn generate(&self, _request: &GenerateRequest) -> Result<GenerateResponse, AiError> {
            Ok(GenerateResponse {
                text: "hello".into(),
                usage: TokenUsage::default(),
                stop_reason: StopReason::EndTurn,
                tool_calls: vec![],
            })
        }
        fn stream_generate(
            &self,
            _request: &GenerateRequest,
            on_event: &(dyn Fn(StreamEvent) + Send + Sync),
        ) -> Result<(), AiError> {
            on_event(StreamEvent::Delta {
                delta: "你".into()
            });
            on_event(StreamEvent::Delta {
                delta: "好".into()
            });
            on_event(StreamEvent::Done {
                full_text: "你好".into(),
                usage: TokenUsage::default(),
                stop_reason: Some(StopReason::EndTurn),
            });
            Ok(())
        }
    }

    /// 阻塞的 mock provider：模拟长时间生成
    #[derive(Debug)]
    struct BlockingProvider;

    fn empty_response() -> GenerateResponse {
        GenerateResponse {
            text: String::new(),
            usage: TokenUsage::default(),
            stop_reason: StopReason::EndTurn,
            tool_calls: vec![],
        }
    }

    impl AiProvider for BlockingProvider {
        fn id(&self) -> &str {
            "mock-blocking"
        }
        fn generate(&self, _request: &GenerateRequest) -> Result<GenerateResponse, AiError> {
            std::thread::sleep(Duration::from_secs(2));
            Ok(empty_response())
        }
        fn stream_generate(
            &self,
            _request: &GenerateRequest,
            on_event: &(dyn Fn(StreamEvent) + Send + Sync),
        ) -> Result<(), AiError> {
            std::thread::sleep(Duration::from_secs(2));
            on_event(StreamEvent::Done {
                full_text: "late".into(),
                usage: TokenUsage::default(),
                stop_reason: Some(StopReason::EndTurn),
            });
            Ok(())
        }
    }

    fn sample_request() -> GenerateRequest {
        GenerateRequest {
            system: "s".into(),
            messages: vec![crate::provider::Message::user("hi")],
            model: "m".into(),
            temperature: 0.7,
            max_tokens: 100,
            tools: vec![ToolDef {
                name: "t".into(),
                description: String::new(),
                parameters: serde_json::json!({}),
            }],
        }
    }

    #[test]
    fn forwards_events_and_aggregates() {
        let provider: Arc<dyn AiProvider> = Arc::new(ImmediateProvider);
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink: Arc<dyn Fn(StreamEvent) + Send + Sync> = {
            let events = Arc::clone(&events);
            Arc::new(move |e| events.lock().unwrap().push(e))
        };
        let resp =
            stream_generate_with_cancel(provider, &sample_request(), &CancelToken::new(), sink)
                .unwrap();

        assert_eq!(resp.text, "你好");
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
        assert_eq!(events.lock().unwrap().len(), 3);
    }

    #[test]
    fn cancel_returns_immediately() {
        let provider: Arc<dyn AiProvider> = Arc::new(BlockingProvider);
        let token = CancelToken::new();
        token.cancel();

        let started = std::time::Instant::now();
        let result =
            stream_generate_with_cancel(provider, &sample_request(), &token, Arc::new(|_| {}));
        assert!(matches!(result, Err(AiError::Cancelled)));
        // 未等满 2s 即返回（'static 线程不阻塞返回）
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
