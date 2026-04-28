use super::*;
use agent_settings::AgentSettings;
use gpui::{App, SharedString, Task};
use std::future;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// 一个流式回显工具，用于测试流式工具的生命周期
/// （例如，在 `is_input_complete` 之前 LLM 流结束时部分传递和清理）。
#[derive(JsonSchema, Serialize, Deserialize)]
pub struct StreamingEchoToolInput {
    /// 要回显的文本。
    pub text: String,
}

pub struct StreamingEchoTool {
    wait_until_complete_rx: Mutex<Option<oneshot::Receiver<()>>>,
}

impl StreamingEchoTool {
    pub fn new() -> Self {
        Self {
            wait_until_complete_rx: Mutex::new(None),
        }
    }

    pub fn with_wait_until_complete(mut self, receiver: oneshot::Receiver<()>) -> Self {
        self.wait_until_complete_rx = Mutex::new(Some(receiver));
        self
    }
}

impl AgentTool for StreamingEchoTool {
    type Input = StreamingEchoToolInput;
    type Output = String;

    const NAME: &'static str = "streaming_echo";

    fn supports_input_streaming() -> bool {
        true
    }

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Other
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "流式回显".into()
    }

    fn run(
        self: Arc<Self>,
        input: ToolInput<Self::Input>,
        _event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<String, String>> {
        let wait_until_complete_rx = self.wait_until_complete_rx.lock().unwrap().take();
        cx.spawn(async move |_cx| {
            let input = input
                .recv()
                .await
                .map_err(|e| format!("无法接收工具输入：{e}"))?;
            if let Some(rx) = wait_until_complete_rx {
                rx.await.ok();
            }
            Ok(input.text)
        })
    }
}

#[derive(JsonSchema, Serialize, Deserialize)]
pub struct StreamingJsonErrorContextToolInput {
    /// 要回显的文本。
    pub text: String,
}

pub struct StreamingJsonErrorContextTool;

impl AgentTool for StreamingJsonErrorContextTool {
    type Input = StreamingJsonErrorContextToolInput;
    type Output = String;

    const NAME: &'static str = "streaming_json_error_context";

    fn supports_input_streaming() -> bool {
        true
    }

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Other
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "流式 JSON 错误上下文".into()
    }

    fn run(
        self: Arc<Self>,
        mut input: ToolInput<Self::Input>,
        _event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<String, String>> {
        cx.spawn(async move |_cx| {
            let mut last_partial_text = None;

            loop {
                match input.next().await {
                    Ok(ToolInputPayload::Partial(partial)) => {
                        if let Some(text) = partial.get("text").and_then(|value| value.as_str()) {
                            last_partial_text = Some(text.to_string());
                        }
                    }
                    Ok(ToolInputPayload::Full(input)) => return Ok(input.text),
                    Ok(ToolInputPayload::InvalidJson { error_message }) => {
                        let partial_text = last_partial_text.unwrap_or_default();
                        return Err(format!(
                            "在无效的 JSON 之前看到部分文本 '{partial_text}'：{error_message}"
                        ));
                    }
                    Err(error) => {
                        return Err(format!("无法接收工具输入：{error}"));
                    }
                }
            }
        })
    }
}

/// 一个流式回显工具，用于测试流式工具的生命周期
/// （例如，在 `is_input_complete` 之前 LLM 流结束时部分传递和清理）。
#[derive(JsonSchema, Serialize, Deserialize)]
pub struct StreamingFailingEchoToolInput {
    /// 要回显的文本。
    pub text: String,
}

pub struct StreamingFailingEchoTool {
    pub receive_chunks_until_failure: usize,
}

impl AgentTool for StreamingFailingEchoTool {
    type Input = StreamingFailingEchoToolInput;

    type Output = String;

    const NAME: &'static str = "streaming_failing_echo";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Other
    }

    fn supports_input_streaming() -> bool {
        true
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "echo".into()
    }

    fn run(
        self: Arc<Self>,
        mut input: ToolInput<Self::Input>,
        _event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<Self::Output, Self::Output>> {
        cx.spawn(async move |_cx| {
            for _ in 0..self.receive_chunks_until_failure {
                let _ = input.next().await;
            }
            Err("failed".into())
        })
    }
}

/// 一个回显其输入的工具
#[derive(JsonSchema, Serialize, Deserialize)]
pub struct EchoToolInput {
    /// 要回显的文本。
    pub text: String,
}

pub struct EchoTool;

impl AgentTool for EchoTool {
    type Input = EchoToolInput;
    type Output = String;

    const NAME: &'static str = "echo";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Other
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "回显".into()
    }

    fn run(
        self: Arc<Self>,
        input: ToolInput<Self::Input>,
        _event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<String, String>> {
        cx.spawn(async move |_cx| {
            let input = input
                .recv()
                .await
                .map_err(|e| format!("无法接收工具输入：{e}"))?;
            Ok(input.text)
        })
    }
}

/// 一个等待指定延迟的工具
#[derive(JsonSchema, Serialize, Deserialize)]
pub struct DelayToolInput {
    /// 延迟时间，单位毫秒。
    ms: u64,
}

pub struct DelayTool;

impl AgentTool for DelayTool {
    type Input = DelayToolInput;
    type Output = String;

    const NAME: &'static str = "delay";

    fn initial_title(
        &self,
        input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        if let Ok(input) = input {
            format!("延迟 {}ms", input.ms).into()
        } else {
            "延迟".into()
        }
    }

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Other
    }

    fn run(
        self: Arc<Self>,
        input: ToolInput<Self::Input>,
        _event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<String, String>>
    where
        Self: Sized,
    {
        let executor = cx.background_executor().clone();
        cx.foreground_executor().spawn(async move {
            let input = input
                .recv()
                .await
                .map_err(|e| format!("无法接收工具输入：{e}"))?;
            executor.timer(Duration::from_millis(input.ms)).await;
            Ok("Ding".to_string())
        })
    }
}

#[derive(JsonSchema, Serialize, Deserialize)]
pub struct ToolRequiringPermissionInput {}

pub struct ToolRequiringPermission;

impl AgentTool for ToolRequiringPermission {
    type Input = ToolRequiringPermissionInput;
    type Output = String;

    const NAME: &'static str = "tool_requiring_permission";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Other
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "此工具需要权限".into()
    }

    fn run(
        self: Arc<Self>,
        input: ToolInput<Self::Input>,
        event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<String, String>> {
        cx.spawn(async move |cx| {
            let _input = input
                .recv()
                .await
                .map_err(|e| format!("无法接收工具输入：{e}"))?;

            let decision = cx.update(|cx| {
                decide_permission_from_settings(
                    Self::NAME,
                    &[String::new()],
                    AgentSettings::get_global(cx),
                )
            });

            let authorize = match decision {
                ToolPermissionDecision::Allow => None,
                ToolPermissionDecision::Deny(reason) => {
                    return Err(reason);
                }
                ToolPermissionDecision::Confirm => Some(cx.update(|cx| {
                    let context = crate::ToolPermissionContext::new(
                        "tool_requiring_permission",
                        vec![String::new()],
                    );
                    event_stream.authorize("授权吗？", context, cx)
                })),
            };

            if let Some(authorize) = authorize {
                authorize.await.map_err(|e| e.to_string())?;
            }
            Ok("已允许".to_string())
        })
    }
}

#[derive(JsonSchema, Serialize, Deserialize)]
pub struct InfiniteToolInput {}

pub struct InfiniteTool;

impl AgentTool for InfiniteTool {
    type Input = InfiniteToolInput;
    type Output = String;

    const NAME: &'static str = "infinite";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Other
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "无限工具".into()
    }

    fn run(
        self: Arc<Self>,
        input: ToolInput<Self::Input>,
        _event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<String, String>> {
        cx.foreground_executor().spawn(async move {
            let _input = input
                .recv()
                .await
                .map_err(|e| format!("无法接收工具输入：{e}"))?;
            future::pending::<()>().await;
            unreachable!()
        })
    }
}

/// 一个永远循环但通过 `select!` 正确处理取消的工具，
/// 类似于 edit_file_tool 处理取消的方式。
#[derive(JsonSchema, Serialize, Deserialize)]
pub struct CancellationAwareToolInput {}

pub struct CancellationAwareTool {
    pub was_cancelled: Arc<AtomicBool>,
}

impl CancellationAwareTool {
    pub fn new() -> (Self, Arc<AtomicBool>) {
        let was_cancelled = Arc::new(AtomicBool::new(false));
        (
            Self {
                was_cancelled: was_cancelled.clone(),
            },
            was_cancelled,
        )
    }
}

impl AgentTool for CancellationAwareTool {
    type Input = CancellationAwareToolInput;
    type Output = String;

    const NAME: &'static str = "cancellation_aware";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Other
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "可感知取消的工具".into()
    }

    fn run(
        self: Arc<Self>,
        input: ToolInput<Self::Input>,
        event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<String, String>> {
        cx.foreground_executor().spawn(async move {
            let _input = input
                .recv()
                .await
                .map_err(|e| format!("无法接收工具输入：{e}"))?;
            // 等待取消 - 此工具除了等待被取消之外什么也不做
            event_stream.cancelled_by_user().await;
            self.was_cancelled.store(true, Ordering::SeqCst);
            Err("工具已被用户取消".to_string())
        })
    }
}

/// 一个接受对象作为输入的工具，该对象是从字母到以该字母开头的随机单词的映射。
/// 所有字段都是必填的！每个字母都要提供一个单词！
#[derive(JsonSchema, Serialize, Deserialize)]
pub struct WordListInput {
    /// 提供一个以 A 开头的随机单词。
    a: Option<String>,
    /// 提供一个以 B 开头的随机单词。
    b: Option<String>,
    /// 提供一个以 C 开头的随机单词。
    c: Option<String>,
    /// 提供一个以 D 开头的随机单词。
    d: Option<String>,
    /// 提供一个以 E 开头的随机单词。
    e: Option<String>,
    /// 提供一个以 F 开头的随机单词。
    f: Option<String>,
    /// 提供一个以 G 开头的随机单词。
    g: Option<String>,
}

pub struct WordListTool;

impl AgentTool for WordListTool {
    type Input = WordListInput;
    type Output = String;

    const NAME: &'static str = "word_list";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Other
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "随机单词列表".into()
    }

    fn run(
        self: Arc<Self>,
        input: ToolInput<Self::Input>,
        _event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<String, String>> {
        cx.spawn(async move |_cx| {
            let _input = input
                .recv()
                .await
                .map_err(|e| format!("无法接收工具输入：{e}"))?;
            Ok("ok".to_string())
        })
    }
}