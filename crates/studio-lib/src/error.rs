//! 统一错误类型 — 归一化各 crate 的错误为单一枚举

use serde::{Deserialize, Serialize};
use story_parser::ParseError;
use thiserror::Error;

/// Story Studio 库层的统一错误
#[derive(Debug, Error, Serialize, Deserialize)]
pub enum StudioError {
    /// 解析错误（可能多条）
    #[error("解析失败（{count} 个错误）")]
    Parse {
        count: usize,
        #[serde(skip)]
        errors: Vec<ParseError>,
    },

    /// 编译运行时错误
    #[error("编译错误: {0}")]
    Compile(String),

    /// 导出错误
    #[error("导出错误: {0}")]
    Export(String),

    /// AI 调用错误
    #[error("AI 错误: {0}")]
    Ai(String),

    /// 文件 IO 错误
    #[error("IO 错误: {0}")]
    Io(String),

    /// 实体 / Scene / Fragment 不存在
    #[error("未找到: {0}")]
    NotFound(String),

    /// 配置错误（如缺少 API Key）
    #[error("配置错误: {0}")]
    Config(String),
}

impl From<std::io::Error> for StudioError {
    fn from(e: std::io::Error) -> Self {
        StudioError::Io(e.to_string())
    }
}

/// 将解析错误列表转换为 StudioError
impl From<Vec<ParseError>> for StudioError {
    fn from(errors: Vec<ParseError>) -> Self {
        let count = errors.len();
        StudioError::Parse { count, errors }
    }
}
