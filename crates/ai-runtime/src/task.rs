//! Task 模型 (RFC-0010 §4)
//!
//! 统一 Task 抽象：简单任务走 StatelessTask，创作任务走 ConversationalTask。
//! Task 负责构建上下文 + 解析 AI 输出为 Change 序列。

use serde::{Deserialize, Serialize};
use story_ir::{EntityAnchor, StableId, StoryIR};
use thiserror::Error;

// ============================================================================
// TaskKind / TaskMode
// ============================================================================

/// 任务类型 (RFC-0010 §4.1)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskKind {
    WriteFragment,
    AddAnchors,
    UpdateCharacter,
    ExtractAssertions,
    SemanticCheck,
    Custom(String),
}

/// 执行模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskMode {
    /// 无状态单次
    Stateless,
    /// 有状态会话
    Conversational,
}

// ============================================================================
// TaskContext / TaskResult / TaskError
// ============================================================================

/// Task 构建的上下文（传给 AI Provider）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskContext {
    pub system: String,
    pub user: String,
    pub model: ModelConfig,
}

/// 模型参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub temperature: f32,
    pub max_tokens: usize,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            temperature: 0.7,
            max_tokens: 4096,
        }
    }
}

impl ModelConfig {
    pub fn creative() -> Self {
        Self {
            temperature: 0.8,
            max_tokens: 4096,
        }
    }

    pub fn extraction() -> Self {
        Self {
            temperature: 0.3,
            max_tokens: 4096,
        }
    }
}

/// Task 执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    /// 生成的 Change 描述（简化版——v0.1 用 FragmentDraft 表示）
    pub drafts: Vec<FragmentDraft>,
    /// token 消耗
    pub usage_token: usize,
}

/// AI 生成的 Fragment 草稿
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FragmentDraft {
    /// 正文
    pub text: String,
    /// 解析出的 anchors（AI 在 frontmatter 中标注或 Parser 匹配）
    pub anchors: Vec<EntityAnchor>,
}

/// Task 错误
#[derive(Debug, Error, Serialize, Deserialize)]
pub enum TaskError {
    #[error("输出解析失败: {0}")]
    Parse(String),

    #[error("Dry-Run 失败（{n} 个错误）")]
    DryRunFailed { n: usize },

    #[error("重试次数超限（{max_retries}）")]
    MaxRetriesExceeded { max_retries: usize },
}

// ============================================================================
// Task trait
// ============================================================================

/// 所有 AI 任务的统一接口 (RFC-0010 §4.1)
pub trait Task {
    fn kind(&self) -> TaskKind;
    fn mode(&self) -> TaskMode;

    /// 构建上下文
    fn build_context(&self, ir: &StoryIR) -> TaskContext;

    /// 解析 AI 输出为 FragmentDraft 序列
    fn parse_output(&self, ai_output: &str) -> Result<Vec<FragmentDraft>, TaskError>;

    /// Dry-Run 失败时的反馈构建
    fn build_feedback(&self, errors: &[String]) -> String {
        if errors.is_empty() {
            return String::new();
        }
        format!(
            "上次尝试存在问题：\n{}\n请修正后重试。",
            errors
                .iter()
                .map(|e| format!("- {e}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    }
}

// ============================================================================
// 共享 draft 解析
// ============================================================================

/// 解析 AI 输出为单个 FragmentDraft。
///
/// AI 可能输出带 frontmatter 的 Fragment（anchors 在 frontmatter 中），
/// 也可能只输出纯正文。frontmatter 解析失败时降级为纯正文。
pub fn parse_fragment_draft(ai_output: &str) -> FragmentDraft {
    use story_project_format::FragmentFile;

    if ai_output.trim_start().starts_with("---") {
        match FragmentFile::parse(ai_output) {
            Ok(file) => {
                let anchors = file
                    .frontmatter
                    .anchors
                    .into_iter()
                    .filter_map(|a| {
                        let entity = StableId::new(&a.entity).ok()?;
                        let role = match a.role.as_str() {
                            "appears" => story_ir::AnchorRole::Appears,
                            "mentioned" => story_ir::AnchorRole::Mentioned,
                            "referenced" => story_ir::AnchorRole::Referenced,
                            _ => return None,
                        };
                        Some(EntityAnchor { entity, role })
                    })
                    .collect();
                FragmentDraft {
                    text: file.text,
                    anchors,
                }
            }
            Err(_) => FragmentDraft {
                text: ai_output.to_string(),
                anchors: vec![],
            },
        }
    } else {
        FragmentDraft {
            text: ai_output.to_string(),
            anchors: vec![],
        }
    }
}

// ============================================================================
// WriteFragmentTask — 续写 Fragment
// ============================================================================

/// 续写 Fragment 任务（RFC-0010 §5.2）
#[derive(Debug)]
pub struct WriteFragmentTask {
    /// 目标 Scene ID
    pub scene_id: StableId,
    /// 前 N 个 Fragment 的正文（提供连贯性）
    pub recent_text: String,
    /// POV 角色名
    pub pov_name: Option<String>,
    /// 场景标题
    pub scene_title: Option<String>,
}

impl Task for WriteFragmentTask {
    fn kind(&self) -> TaskKind {
        TaskKind::WriteFragment
    }
    fn mode(&self) -> TaskMode {
        TaskMode::Stateless
    }

    fn build_context(&self, _ir: &StoryIR) -> TaskContext {
        let mut system = String::from(
            "你是一位小说作者。请以自然语言续写故事正文。\n\
             只输出正文，不要输出元信息。\n\
             如涉及角色出场，在正文前用 frontmatter 标注 anchors。",
        );

        if let Some(ref pov) = self.pov_name {
            system.push_str(&format!("\n本场景视角：{pov}。"));
        }
        if let Some(ref title) = self.scene_title {
            system.push_str(&format!("\n场景：{title}。"));
        }

        let user = if self.recent_text.is_empty() {
            "请写一段新的故事片段。".to_string()
        } else {
            format!("前文：\n{}\n\n请续写下一个片段。", self.recent_text)
        };

        TaskContext {
            system,
            user,
            model: ModelConfig::creative(),
        }
    }

    fn parse_output(&self, ai_output: &str) -> Result<Vec<FragmentDraft>, TaskError> {
        Ok(vec![parse_fragment_draft(ai_output)])
    }
}

// ============================================================================
// WriteChapterTask — 整章生文（逐章流水线用）
// ============================================================================

/// 整章正文生成任务的上下文材料。
///
/// 材料由编排层（studio-lib workflow）从项目文件 + IR 组装完毕后注入，
/// 本 Task 只负责格式化为 prompt 与解析输出。
/// 配方参考逐章生文工作流：本章卡 + 前章结尾 + 下章衔接 + 角色卡 + 大纲梗概。
#[derive(Debug, Default)]
pub struct ChapterMaterials {
    /// 大纲梗概（outline.toml 的故事简介，一两句话）
    pub outline_brief: String,
    /// 本章卡：标题 + 梗概 + 目标 + 节拍 + 章末钩子（已格式化）
    pub chapter_card: String,
    /// 出场角色卡（已格式化，一行一角色）
    pub character_cards: String,
    /// 上一章正文结尾（约 600 字，首章为空）
    pub previous_tail: String,
    /// 下一章摘要（仅用于衔接，明确告知不要写进本章）
    pub next_summary: String,
    /// 目标字数
    pub target_words: u32,
    /// 中断续写时的已写部分（从中断处继续）
    pub partial_text: String,
    /// 重写时的原稿参考（不照抄）+ 质检失败原因（已格式化）
    pub rewrite_reference: String,
}

/// 整章正文生成任务（逐章生文流水线）
#[derive(Debug, Default)]
pub struct WriteChapterTask {
    pub materials: ChapterMaterials,
}

impl Task for WriteChapterTask {
    fn kind(&self) -> TaskKind {
        TaskKind::WriteFragment
    }
    fn mode(&self) -> TaskMode {
        TaskMode::Stateless
    }

    fn build_context(&self, _ir: &StoryIR) -> TaskContext {
        let m = &self.materials;
        let system = String::from(
            "你是一位深耕网文连载的小说作者，擅长在既定节拍内写出信息密度高、\n\
             有画面感的正文。要求：\n\
             - 只输出正文本身，不要输出大纲、节拍编号、标题或任何元信息。\n\
             - 如有角色出场，可在正文前用 frontmatter 标注 anchors。\n\
             - 严格遵守给定的节拍顺序与章末钩子，不要跳写后面的剧情。\n\
             - 目标字数是硬性要求，正文字数应达到目标的 90% 以上。",
        );

        let mut user = String::new();
        if !m.outline_brief.is_empty() {
            user.push_str(&format!("【大纲梗概】\n{}\n\n", m.outline_brief));
        }
        if !m.character_cards.is_empty() {
            user.push_str(&format!("【出场人物】\n{}\n\n", m.character_cards));
        }
        user.push_str(&format!("【本章设定】\n{}\n\n", m.chapter_card));
        if !m.previous_tail.is_empty() {
            user.push_str(&format!(
                "【前文】（上一章结尾，衔接文气，不要重复）\n{}\n\n",
                m.previous_tail
            ));
        }
        if !m.next_summary.is_empty() {
            user.push_str(&format!(
                "【下一章摘要】（仅用于收尾衔接，不要把下一章的内容写进本章）\n{}\n\n",
                m.next_summary
            ));
        }
        user.push_str(&format!(
            "【任务】\n写本章完整正文，目标 {} 字左右。\n",
            m.target_words
        ));

        if !m.partial_text.is_empty() {
            user.push_str(&format!(
                "\n【已写部分】（这是本章的开头，从中断处自然继续，写完剩余部分；不要重复已写内容）\n{}\n",
                m.partial_text
            ));
        }
        if !m.rewrite_reference.is_empty() {
            user.push_str(&format!("\n【重写说明】\n{}\n", m.rewrite_reference));
        }

        TaskContext {
            system,
            user,
            // 整章生成需要足够的输出空间：目标字数 × 2（中文一字约 1~2 token），
            // 下限 16384 — 思考模式会占用输出配额且兼容端点不严格受 budget 约束，
            // 配额不足时思考耗尽 max_tokens 而正文为空
            model: ModelConfig {
                temperature: 0.8,
                max_tokens: 16384usize.max(m.target_words as usize * 8),
            },
        }
    }

    fn parse_output(&self, ai_output: &str) -> Result<Vec<FragmentDraft>, TaskError> {
        Ok(vec![parse_fragment_draft(ai_output)])
    }
}

// ============================================================================
// ExtractAssertionsTask — 知识抽取 (RFC-0010 §9, RFC-0015 §6)
// ============================================================================

/// 从 Fragment 正文抽取知识断言的 Task
///
/// 走与 WriteFragment 相同的管线：构建上下文 → AI 生成 → 解析 → Dry-Run。
/// AI 输出 YAML 格式的断言，系统解析为 AssertionDraft。
#[derive(Debug)]
pub struct ExtractAssertionsTask {
    /// 要抽取的 Fragment 正文
    pub fragment_text: String,
    /// Fragment 中涉及的角色 Display Name 列表（辅助 AI 理解）
    pub known_characters: Vec<String>,
    /// 要抽取的 predicate 类型（如 ["alive", "age"]）
    pub predicates: Vec<String>,
}

/// AI 抽取的断言草稿（尚未提交到 KB）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssertionDraft {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub quote: String,
    pub from_year: Option<i32>,
    pub until_year: Option<i32>,
}

impl Task for ExtractAssertionsTask {
    fn kind(&self) -> TaskKind {
        TaskKind::ExtractAssertions
    }
    fn mode(&self) -> TaskMode {
        TaskMode::Stateless
    }

    fn build_context(&self, _ir: &StoryIR) -> TaskContext {
        let system = format!(
            "你是知识抽取器。从以下故事文本中提取可验证的断言。\n\
             只抽取这些类型的断言：{:?}\n\
             每条断言必须包含：subject（角色名）、predicate、object（值）、quote（原文摘录）。\n\
             如果能确定时间范围，附带 from_year / until_year。\n\n\
             输出 YAML 格式，例如：\n\
             ```yaml\n\
             - subject: 林风\n\
               predicate: alive\n\
               object: true\n\
               quote: 林风还活着\n\
               from_year: 1086\n\
             ```\n\
             如果文本中没有可抽取的断言，输出空列表 `[]`。\n\n\
             已知角色：{}",
            self.predicates,
            self.known_characters.join("、")
        );

        let user = format!("请从以下文本中抽取断言：\n\n{}", self.fragment_text);

        TaskContext {
            system,
            user,
            model: ModelConfig::extraction(),
        }
    }

    fn parse_output(&self, ai_output: &str) -> Result<Vec<FragmentDraft>, TaskError> {
        // Extraction 的输出是 YAML 断言列表，不是 Fragment 正文
        // 这里不解析为 FragmentDraft——Extraction 的输出在 Pipeline 中特殊处理
        // v0.1 简化：返回空 FragmentDraft（Extraction 不走 Fragment 管线）
        // 完整实现需要 ExtractionPipeline 单独处理（见 §9）
        let _ = ai_output;
        Ok(vec![])
    }
}

impl ExtractAssertionsTask {
    /// 解析 AI 输出的 YAML 为 AssertionDraft 列表
    pub fn parse_assertions(&self, ai_output: &str) -> Result<Vec<AssertionDraft>, TaskError> {
        // 尝试从 ```yaml ... ``` 代码块中提取
        let yaml_text = if let Some(start) = ai_output.find("```yaml") {
            let after = &ai_output[start + 7..];
            if let Some(end) = after.find("```") {
                &after[..end]
            } else {
                ai_output
            }
        } else if let Some(start) = ai_output.find("```") {
            let after = &ai_output[start + 3..];
            if let Some(end) = after.find("```") {
                &after[..end]
            } else {
                ai_output
            }
        } else {
            ai_output
        };

        let yaml_text = yaml_text.trim();

        // 空列表
        if yaml_text == "[]" || yaml_text.is_empty() {
            return Ok(vec![]);
        }

        // 解析 YAML
        let drafts: Vec<YamlAssertion> = serde_yaml::from_str(yaml_text)
            .map_err(|e| TaskError::Parse(format!("YAML 解析失败: {e}\n原文: {yaml_text}")))?;

        Ok(drafts
            .into_iter()
            .map(|y| AssertionDraft {
                subject: y.subject,
                predicate: y.predicate,
                object: y.object,
                quote: y.quote,
                from_year: y.from_year,
                until_year: y.until_year,
            })
            .collect())
    }
}

/// YAML 解析用的临时结构
#[derive(Debug, serde::Deserialize)]
struct YamlAssertion {
    subject: String,
    predicate: String,
    object: String,
    quote: String,
    #[serde(default)]
    from_year: Option<i32>,
    #[serde(default)]
    until_year: Option<i32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_fragment_build_context() {
        let task = WriteFragmentTask {
            scene_id: StableId::new_unchecked("sc_01"),
            recent_text: "林风推开门。".to_string(),
            pov_name: Some("林风".to_string()),
            scene_title: Some("渡口".to_string()),
        };

        let ir = StoryIR::new();
        let ctx = task.build_context(&ir);

        assert!(ctx.system.contains("小说作者"));
        assert!(ctx.system.contains("林风"));
        assert!(ctx.user.contains("林风推开门"));
        assert!(ctx.model.temperature > 0.7); // creative
    }

    #[test]
    fn parse_output_with_frontmatter() {
        let task = WriteFragmentTask {
            scene_id: StableId::new_unchecked("sc_01"),
            recent_text: String::new(),
            pov_name: None,
            scene_title: None,
        };

        let ai_output = "---\nid: f_999\nanchors:\n  - entity: c_001\n    role: appears\n---\n\n林风走进客栈。\n";
        let drafts = task.parse_output(ai_output).unwrap();

        assert_eq!(drafts.len(), 1);
        assert!(drafts[0].text.contains("林风走进客栈"));
        assert_eq!(drafts[0].anchors.len(), 1);
        assert_eq!(drafts[0].anchors[0].entity.as_str(), "c_001");
    }

    #[test]
    fn parse_output_plain_text() {
        let task = WriteFragmentTask {
            scene_id: StableId::new_unchecked("sc_01"),
            recent_text: String::new(),
            pov_name: None,
            scene_title: None,
        };

        let drafts = task.parse_output("纯粹的正文文本。").unwrap();
        assert_eq!(drafts.len(), 1);
        assert!(drafts[0].anchors.is_empty());
        assert_eq!(drafts[0].text, "纯粹的正文文本。");
    }

    #[test]
    fn extract_build_context() {
        let task = ExtractAssertionsTask {
            fragment_text: "林风今年十七岁。".to_string(),
            known_characters: vec!["林风".to_string()],
            predicates: vec!["alive".to_string(), "age".to_string()],
        };

        let ir = StoryIR::new();
        let ctx = task.build_context(&ir);

        assert!(ctx.system.contains("知识抽取器"));
        assert!(ctx.system.contains("alive"));
        assert!(ctx.system.contains("林风"));
        assert!(ctx.user.contains("林风今年十七岁"));
        assert!(ctx.model.temperature < 0.5); // extraction 用低温度
    }

    #[test]
    fn extract_parse_assertions_yaml_block() {
        let task = ExtractAssertionsTask {
            fragment_text: String::new(),
            known_characters: vec![],
            predicates: vec![],
        };

        let ai_output = "好的，以下是从文本中抽取的断言：\n\
             ```yaml\n\
             - subject: 林风\n  \
               predicate: age\n  \
               object: \"17\"\n  \
               quote: 林风今年十七岁\n  \
               from_year: 1086\n\
             ```";

        let drafts = task.parse_assertions(ai_output).unwrap();
        assert_eq!(drafts.len(), 1);
        assert_eq!(drafts[0].subject, "林风");
        assert_eq!(drafts[0].predicate, "age");
        assert_eq!(drafts[0].object, "17");
        assert_eq!(drafts[0].from_year, Some(1086));
    }

    #[test]
    fn extract_parse_empty_output() {
        let task = ExtractAssertionsTask {
            fragment_text: String::new(),
            known_characters: vec![],
            predicates: vec![],
        };

        let drafts = task.parse_assertions("[]").unwrap();
        assert!(drafts.is_empty());
    }

    #[test]
    fn write_chapter_build_context_sections() {
        let task = WriteChapterTask {
            materials: ChapterMaterials {
                outline_brief: "少年林风的江湖行".into(),
                chapter_card: "第3章《夜雨》：目标是引出旧案".into(),
                character_cards: "林风：17 岁，剑客".into(),
                previous_tail: "……他消失在雨幕里。".into(),
                next_summary: "第4章：渡口遇袭".into(),
                target_words: 2000,
                partial_text: "林风踏进客栈。".into(),
                rewrite_reference: "字数不足，原稿如下（不要照抄）。".into(),
            },
        };

        let ctx = task.build_context(&StoryIR::new());

        for section in [
            "【大纲梗概】",
            "【出场人物】",
            "【本章设定】",
            "【前文】",
            "【下一章摘要】",
            "【任务】",
            "【已写部分】",
            "【重写说明】",
        ] {
            assert!(ctx.user.contains(section), "缺少分段: {section}");
        }
        assert!(ctx.user.contains("2000"));
        // max_tokens = max(16384, target×8)
        assert_eq!(ctx.model.max_tokens, 16384);
    }

    #[test]
    fn write_chapter_max_tokens_scales_with_target() {
        let task = WriteChapterTask {
            materials: ChapterMaterials {
                target_words: 4000,
                ..ChapterMaterials::default()
            },
        };
        let ctx = task.build_context(&StoryIR::new());
        assert_eq!(ctx.model.max_tokens, 32000);
        // 空材料时 user 仍应有【本章设定】与【任务】段
        assert!(ctx.user.contains("【任务】"));
    }

    #[test]
    fn write_chapter_parse_output_frontmatter_and_plain() {
        let task = WriteChapterTask::default();

        let with_fm =
            "---\nid: f_0101\nanchors:\n  - entity: c_001\n    role: appears\n---\n\n夜雨滂沱。\n";
        let drafts = task.parse_output(with_fm).unwrap();
        assert_eq!(drafts[0].anchors.len(), 1);
        assert!(drafts[0].text.contains("夜雨滂沱"));

        let plain = "夜雨滂沱。\n第二段。";
        let drafts = task.parse_output(plain).unwrap();
        assert!(drafts[0].anchors.is_empty());
        assert_eq!(drafts[0].text, plain);
    }
}
