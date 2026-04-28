use crate::{AgentMessage, AgentMessageContent, UserMessage, UserMessageContent};
use acp_thread::UserMessageId;
use agent_client_protocol::schema as acp;
use agent_settings::AgentProfileId;
use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use collections::{HashMap, IndexMap};
use futures::{FutureExt, future::Shared};
use gpui::{BackgroundExecutor, Global, Task};
use indoc::indoc;
use language_model::Speed;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sqlez::{
    bindable::{Bind, Column},
    connection::Connection,
    statement::Statement,
};
use std::sync::Arc;
use ui::{App, SharedString};
use util::path_list::PathList;
use zed_env_vars::ZED_STATELESS;

pub type DbMessage = crate::Message;
pub type DbSummary = crate::legacy_thread::DetailedSummaryState;
pub type DbLanguageModel = crate::legacy_thread::SerializedLanguageModel;

/// 数据库线程元数据
#[derive(Debug, Clone)]
pub struct DbThreadMetadata {
    pub id: acp::SessionId,
    pub parent_session_id: Option<acp::SessionId>,
    pub title: SharedString,
    pub updated_at: DateTime<Utc>,
    pub created_at: Option<DateTime<Utc>>,
    /// 创建该对话时的工作区文件夹路径，按字典序排序
    /// 用于在侧边栏中按项目分组对话
    pub folder_paths: PathList,
}

impl From<&DbThreadMetadata> for acp_thread::AgentSessionInfo {
    fn from(meta: &DbThreadMetadata) -> Self {
        Self {
            session_id: meta.id.clone(),
            work_dirs: Some(meta.folder_paths.clone()),
            title: Some(meta.title.clone()),
            updated_at: Some(meta.updated_at),
            created_at: meta.created_at,
            meta: None,
        }
    }
}

/// 数据库存储的对话数据结构
#[derive(Debug, Serialize, Deserialize)]
pub struct DbThread {
    pub title: SharedString,
    pub messages: Vec<DbMessage>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub detailed_summary: Option<SharedString>,
    #[serde(default)]
    pub initial_project_snapshot: Option<Arc<crate::ProjectSnapshot>>,
    #[serde(default)]
    pub cumulative_token_usage: language_model::TokenUsage,
    #[serde(default)]
    pub request_token_usage: HashMap<acp_thread::UserMessageId, language_model::TokenUsage>,
    #[serde(default)]
    pub model: Option<DbLanguageModel>,
    #[serde(default)]
    pub profile: Option<AgentProfileId>,
    #[serde(default)]
    pub imported: bool,
    #[serde(default)]
    pub subagent_context: Option<crate::SubagentContext>,
    #[serde(default)]
    pub speed: Option<Speed>,
    #[serde(default)]
    pub thinking_enabled: bool,
    #[serde(default)]
    pub thinking_effort: Option<String>,
    #[serde(default)]
    pub draft_prompt: Option<Vec<acp::ContentBlock>>,
    #[serde(default)]
    pub ui_scroll_position: Option<SerializedScrollPosition>,
}

/// 序列化的滚动位置
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SerializedScrollPosition {
    pub item_ix: usize,
    pub offset_in_item: f32,
}

/// 可共享的对话数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedThread {
    pub title: SharedString,
    pub messages: Vec<DbMessage>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub model: Option<DbLanguageModel>,
    pub version: String,
}

impl SharedThread {
    pub const VERSION: &'static str = "1.0.0";

    /// 从数据库对话转换为共享对话
    pub fn from_db_thread(thread: &DbThread) -> Self {
        Self {
            title: thread.title.clone(),
            messages: thread.messages.clone(),
            updated_at: thread.updated_at,
            model: thread.model.clone(),
            version: Self::VERSION.to_string(),
        }
    }

    /// 转换为数据库对话格式
    pub fn to_db_thread(self) -> DbThread {
        DbThread {
            title: format!("🔗 {}", self.title).into(),
            messages: self.messages,
            updated_at: self.updated_at,
            detailed_summary: None,
            initial_project_snapshot: None,
            cumulative_token_usage: Default::default(),
            request_token_usage: Default::default(),
            model: self.model,
            profile: None,
            imported: true,
            subagent_context: None,
            speed: None,
            thinking_enabled: false,
            thinking_effort: None,
            draft_prompt: None,
            ui_scroll_position: None,
        }
    }

    /// 序列化为压缩字节数据
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        const COMPRESSION_LEVEL: i32 = 3;
        let json = serde_json::to_vec(self)?;
        let compressed = zstd::encode_all(json.as_slice(), COMPRESSION_LEVEL)?;
        Ok(compressed)
    }

    /// 从压缩字节数据反序列化
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        let decompressed = zstd::decode_all(data)?;
        Ok(serde_json::from_slice(&decompressed)?)
    }
}

impl DbThread {
    pub const VERSION: &'static str = "0.3.0";

    /// 从JSON数据解析对话
    pub fn from_json(json: &[u8]) -> Result<Self> {
        let saved_thread_json = serde_json::from_slice::<serde_json::Value>(json)?;
        match saved_thread_json.get("version") {
            Some(serde_json::Value::String(version)) => match version.as_str() {
                Self::VERSION => Ok(serde_json::from_value(saved_thread_json)?),
                _ => Self::upgrade_from_agent_1(crate::legacy_thread::SerializedThread::from_json(
                    json,
                )?),
            },
            _ => {
                Self::upgrade_from_agent_1(crate::legacy_thread::SerializedThread::from_json(json)?)
            }
        }
    }

    /// 从Agent 1.0版本格式升级数据
    fn upgrade_from_agent_1(thread: crate::legacy_thread::SerializedThread) -> Result<Self> {
        let mut messages = Vec::new();
        let mut request_token_usage = HashMap::default();

        let mut last_user_message_id = None;
        for (ix, msg) in thread.messages.into_iter().enumerate() {
            let message = match msg.role {
                language_model::Role::User => {
                    let mut content = Vec::new();

                    // 将消息片段转换为内容格式
                    for segment in msg.segments {
                        match segment {
                            crate::legacy_thread::SerializedMessageSegment::Text { text } => {
                                content.push(UserMessageContent::Text(text));
                            }
                            crate::legacy_thread::SerializedMessageSegment::Thinking {
                                text,
                                ..
                            } => {
                                // 用户消息不包含思考片段，优雅处理
                                content.push(UserMessageContent::Text(text));
                            }
                            crate::legacy_thread::SerializedMessageSegment::RedactedThinking {
                                ..
                            } => {
                                // 用户消息不包含已编辑思考内容，跳过
                            }
                        }
                    }

                    // 如果未添加任何内容，且上下文可用，则将上下文作为文本添加
                    if content.is_empty() && !msg.context.is_empty() {
                        content.push(UserMessageContent::Text(msg.context));
                    }

                    let id = UserMessageId::new();
                    last_user_message_id = Some(id.clone());

                    crate::Message::User(UserMessage {
                        // 旧格式的消息ID无法有效转换，因此生成新ID
                        id,
                        content,
                    })
                }
                language_model::Role::Assistant => {
                    let mut content = Vec::new();

                    // 将消息片段转换为内容格式
                    for segment in msg.segments {
                        match segment {
                            crate::legacy_thread::SerializedMessageSegment::Text { text } => {
                                content.push(AgentMessageContent::Text(text));
                            }
                            crate::legacy_thread::SerializedMessageSegment::Thinking {
                                text,
                                signature,
                            } => {
                                content.push(AgentMessageContent::Thinking { text, signature });
                            }
                            crate::legacy_thread::SerializedMessageSegment::RedactedThinking {
                                data,
                            } => {
                                content.push(AgentMessageContent::RedactedThinking(data));
                            }
                        }
                    }

                    // 转换工具调用
                    let mut tool_names_by_id = HashMap::default();
                    for tool_use in msg.tool_uses {
                        tool_names_by_id.insert(tool_use.id.clone(), tool_use.name.clone());
                        content.push(AgentMessageContent::ToolUse(
                            language_model::LanguageModelToolUse {
                                id: tool_use.id,
                                name: tool_use.name.into(),
                                raw_input: serde_json::to_string(&tool_use.input)
                                    .unwrap_or_default(),
                                input: tool_use.input,
                                is_input_complete: true,
                                thought_signature: None,
                            },
                        ));
                    }

                    // 转换工具结果
                    let mut tool_results = IndexMap::default();
                    for tool_result in msg.tool_results {
                        let name = tool_names_by_id
                            .remove(&tool_result.tool_use_id)
                            .unwrap_or_else(|| SharedString::from("unknown"));
                        tool_results.insert(
                            tool_result.tool_use_id.clone(),
                            language_model::LanguageModelToolResult {
                                tool_use_id: tool_result.tool_use_id,
                                tool_name: name.into(),
                                is_error: tool_result.is_error,
                                content: tool_result.content,
                                output: tool_result.output,
                            },
                        );
                    }

                    if let Some(last_user_message_id) = &last_user_message_id
                        && let Some(token_usage) = thread.request_token_usage.get(ix).copied()
                    {
                        request_token_usage.insert(last_user_message_id.clone(), token_usage);
                    }

                    crate::Message::Agent(AgentMessage {
                        content,
                        tool_results,
                        reasoning_details: None,
                    })
                }
                language_model::Role::System => {
                    // 新格式不支持系统消息，直接跳过
                    continue;
                }
            };

            messages.push(message);
        }

        Ok(Self {
            title: thread.summary,
            messages,
            updated_at: thread.updated_at,
            detailed_summary: match thread.detailed_summary_state {
                crate::legacy_thread::DetailedSummaryState::NotGenerated
                | crate::legacy_thread::DetailedSummaryState::Generating => None,
                crate::legacy_thread::DetailedSummaryState::Generated { text, .. } => Some(text),
            },
            initial_project_snapshot: thread.initial_project_snapshot,
            cumulative_token_usage: thread.cumulative_token_usage,
            request_token_usage,
            model: thread.model,
            profile: thread.profile,
            imported: false,
            subagent_context: None,
            speed: None,
            thinking_enabled: false,
            thinking_effort: None,
            draft_prompt: None,
            ui_scroll_position: None,
        })
    }
}

/// 数据存储类型
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataType {
    #[serde(rename = "json")]
    Json,
    #[serde(rename = "zstd")]
    Zstd,
}

impl Bind for DataType {
    fn bind(&self, statement: &Statement, start_index: i32) -> Result<i32> {
        let value = match self {
            DataType::Json => "json",
            DataType::Zstd => "zstd",
        };
        value.bind(statement, start_index)
    }
}

impl Column for DataType {
    fn column(statement: &mut Statement, start_index: i32) -> Result<(Self, i32)> {
        let (value, next_index) = String::column(statement, start_index)?;
        let data_type = match value.as_str() {
            "json" => DataType::Json,
            "zstd" => DataType::Zstd,
            _ => anyhow::bail!("未知的数据类型：{}", value),
        };
        Ok((data_type, next_index))
    }
}

/// 对话数据库
pub(crate) struct ThreadsDatabase {
    executor: BackgroundExecutor,
    connection: Arc<Mutex<Connection>>,
}

/// 全局对话数据库实例
struct GlobalThreadsDatabase(Shared<Task<Result<Arc<ThreadsDatabase>, Arc<anyhow::Error>>>>);

impl Global for GlobalThreadsDatabase {}

impl ThreadsDatabase {
    /// 连接数据库（全局单例）
    pub fn connect(cx: &mut App) -> Shared<Task<Result<Arc<ThreadsDatabase>, Arc<anyhow::Error>>>> {
        if cx.has_global::<GlobalThreadsDatabase>() {
            return cx.global::<GlobalThreadsDatabase>().0.clone();
        }
        let executor = cx.background_executor().clone();
        let task = executor
            .spawn({
                let executor = executor.clone();
                async move {
                    match ThreadsDatabase::new(executor) {
                        Ok(db) => Ok(Arc::new(db)),
                        Err(err) => Err(Arc::new(err)),
                    }
                }
            })
            .shared();

        cx.set_global(GlobalThreadsDatabase(task.clone()));
        task
    }

    /// 创建数据库实例
    pub fn new(executor: BackgroundExecutor) -> Result<Self> {
        let connection = if *ZED_STATELESS {
            Connection::open_memory(Some("THREAD_FALLBACK_DB"))
        } else if cfg!(any(feature = "test-support", test)) {
            // Rust在当前线程存储测试名称
            // 用于自动创建在测试内共享（适配test_retrieve_old_thread）
            // 但不与并发测试共享的数据库
            let thread = std::thread::current();
            let test_name = thread.name();
            Connection::open_memory(Some(&format!(
                "THREAD_FALLBACK_{}",
                test_name.unwrap_or_default()
            )))
        } else {
            let threads_dir = paths::data_dir().join("threads");
            std::fs::create_dir_all(&threads_dir)?;
            let sqlite_path = threads_dir.join("threads.db");
            Connection::open_file(&sqlite_path.to_string_lossy())
        };

        // 创建主表
        connection.exec(indoc! {"
            CREATE TABLE IF NOT EXISTS threads (
                id TEXT PRIMARY KEY,
                summary TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                data_type TEXT NOT NULL,
                data BLOB NOT NULL
            )
        "})?()
        .map_err(|e| anyhow!("创建threads表失败：{}", e))?;

        // 兼容升级：添加父ID字段
        if let Ok(mut s) = connection.exec(indoc! {"
            ALTER TABLE threads ADD COLUMN parent_id TEXT
        "})
        {
            s().ok();
        }

        // 兼容升级：添加文件夹路径字段
        if let Ok(mut s) = connection.exec(indoc! {"
            ALTER TABLE threads ADD COLUMN folder_paths TEXT;
            ALTER TABLE threads ADD COLUMN folder_paths_order TEXT;
        "})
        {
            s().ok();
        }

        // 兼容升级：添加创建时间字段
        if let Ok(mut s) = connection.exec(indoc! {"
            ALTER TABLE threads ADD COLUMN created_at TEXT;
        "})
        {
            if s().is_ok() {
                connection.exec(indoc! {"
                    UPDATE threads SET created_at = updated_at WHERE created_at IS NULL
                "})?()?;
            }
        }

        let db = Self {
            executor,
            connection: Arc::new(Mutex::new(connection)),
        };

        Ok(db)
    }

    /// 同步保存对话
    fn save_thread_sync(
        connection: &Arc<Mutex<Connection>>,
        id: acp::SessionId,
        thread: DbThread,
        folder_paths: &PathList,
    ) -> Result<()> {
        const COMPRESSION_LEVEL: i32 = 3;

        #[derive(Serialize)]
        struct SerializedThread {
            #[serde(flatten)]
            thread: DbThread,
            version: &'static str,
        }

        let title = thread.title.to_string();
        let updated_at = thread.updated_at.to_rfc3339();
        let parent_id = thread
            .subagent_context
            .as_ref()
            .map(|ctx| ctx.parent_thread_id.0.clone());
        let serialized_folder_paths = folder_paths.serialize();
        let (folder_paths_str, folder_paths_order_str): (Option<String>, Option<String>) =
            if folder_paths.is_empty() {
                (None, None)
            } else {
                (
                    Some(serialized_folder_paths.paths),
                    Some(serialized_folder_paths.order),
                )
            };
        let json_data = serde_json::to_string(&SerializedThread {
            thread,
            version: DbThread::VERSION,
        })?;

        let connection = connection.lock();

        let compressed = zstd::encode_all(json_data.as_bytes(), COMPRESSION_LEVEL)?;
        let data_type = DataType::Zstd;
        let data = compressed;

        // 新对话使用updated_at作为created_at
        // 确保创建时间反映对话的概念创建时间，而非数据库保存时间
        let created_at = updated_at.clone();

        let mut insert = connection.exec_bound::<(Arc<str>, Option<Arc<str>>, Option<String>, Option<String>, String, String, DataType, Vec<u8>, String)>(indoc! {"
            INSERT INTO threads (id, parent_id, folder_paths, folder_paths_order, summary, updated_at, data_type, data, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(id) DO UPDATE SET
                parent_id = excluded.parent_id,
                folder_paths = excluded.folder_paths,
                folder_paths_order = excluded.folder_paths_order,
                summary = excluded.summary,
                updated_at = excluded.updated_at,
                data_type = excluded.data_type,
                data = excluded.data
        "})?;

        insert((
            id.0,
            parent_id,
            folder_paths_str,
            folder_paths_order_str,
            title,
            updated_at,
            data_type,
            data,
            created_at,
        ))?;

        Ok(())
    }

    /// 获取所有对话列表
    pub fn list_threads(&self) -> Task<Result<Vec<DbThreadMetadata>>> {
        let connection = self.connection.clone();

        self.executor.spawn(async move {
            let connection = connection.lock();

            let mut select = connection
                .select_bound::<(), (Arc<str>, Option<Arc<str>>, Option<String>, Option<String>, String, String, Option<String>)>(indoc! {"
                SELECT id, parent_id, folder_paths, folder_paths_order, summary, updated_at, created_at FROM threads ORDER BY updated_at DESC, created_at DESC
            "})?;

            let rows = select(())?;
            let mut threads = Vec::new();

            for (id, parent_id, folder_paths, folder_paths_order, summary, updated_at, created_at) in rows {
                let folder_paths = folder_paths
                    .map(|paths| {
                        PathList::deserialize(&util::path_list::SerializedPathList {
                            paths,
                            order: folder_paths_order.unwrap_or_default(),
                        })
                    })
                    .unwrap_or_default();
                let created_at = created_at
                    .as_deref()
                    .map(DateTime::parse_from_rfc3339)
                    .transpose()?
                    .map(|dt| dt.with_timezone(&Utc));

                threads.push(DbThreadMetadata {
                    id: acp::SessionId::new(id),
                    parent_session_id: parent_id.map(acp::SessionId::new),
                    title: summary.into(),
                    updated_at: DateTime::parse_from_rfc3339(&updated_at)?.with_timezone(&Utc),
                    created_at,
                    folder_paths,
                });
            }

            Ok(threads)
        })
    }

    /// 加载指定ID的对话
    pub fn load_thread(&self, id: acp::SessionId) -> Task<Result<Option<DbThread>>> {
        let connection = self.connection.clone();

        self.executor.spawn(async move {
            let connection = connection.lock();
            let mut select = connection.select_bound::<Arc<str>, (DataType, Vec<u8>)>(indoc! {"
                SELECT data_type, data FROM threads WHERE id = ? LIMIT 1
            "})?;

            let rows = select(id.0)?;
            if let Some((data_type, data)) = rows.into_iter().next() {
                let json_data = match data_type {
                    DataType::Zstd => {
                        let decompressed = zstd::decode_all(&data[..])?;
                        String::from_utf8(decompressed)?
                    }
                    DataType::Json => String::from_utf8(data)?,
                };
                let thread = DbThread::from_json(json_data.as_bytes())?;
                Ok(Some(thread))
            } else {
                Ok(None)
            }
        })
    }

    /// 保存对话
    pub fn save_thread(
        &self,
        id: acp::SessionId,
        thread: DbThread,
        folder_paths: PathList,
    ) -> Task<Result<()>> {
        let connection = self.connection.clone();

        self.executor
            .spawn(async move { Self::save_thread_sync(&connection, id, thread, &folder_paths) })
    }

    /// 删除指定ID的对话
    pub fn delete_thread(&self, id: acp::SessionId) -> Task<Result<()>> {
        let connection = self.connection.clone();

        self.executor.spawn(async move {
            let connection = connection.lock();

            let mut delete = connection.exec_bound::<Arc<str>>(indoc! {"
                DELETE FROM threads WHERE id = ?
            "})?;

            delete(id.0)?;

            Ok(())
        })
    }

    /// 删除所有对话
    pub fn delete_threads(&self) -> Task<Result<()>> {
        let connection = self.connection.clone();

        self.executor.spawn(async move {
            let connection = connection.lock();

            let mut delete = connection.exec_bound::<()>(indoc! {"
                DELETE FROM threads
            "})?;

            delete(())?;

            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, TimeZone, Utc};
    use collections::HashMap;
    use gpui::TestAppContext;
    use std::sync::Arc;

    #[test]
    /// 测试共享对话序列化往返
    fn test_shared_thread_roundtrip() {
        let original = SharedThread {
            title: "Test Thread".into(),
            messages: vec![],
            updated_at: Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
            model: None,
            version: SharedThread::VERSION.to_string(),
        };

        let bytes = original.to_bytes().expect("序列化失败");
        let restored = SharedThread::from_bytes(&bytes).expect("反序列化失败");

        assert_eq!(restored.title, original.title);
        assert_eq!(restored.version, original.version);
        assert_eq!(restored.updated_at, original.updated_at);
    }

    #[test]
    /// 测试导入标记默认值为false
    fn test_imported_flag_defaults_to_false() {
        // 模拟反序列化无imported字段的旧版对话（向后兼容）
        let json = r#"{
            "title": "Old Thread",
            "messages": [],
            "updated_at": "2024-01-01T00:00:00Z"
        }"#;

        let db_thread: DbThread = serde_json::from_str(json).expect("反序列化失败");

        assert!(
            !db_thread.imported,
            "无imported字段的旧版对话应默认为false"
        );
    }

    /// 创建会话ID工具函数
    fn session_id(value: &str) -> acp::SessionId {
        acp::SessionId::new(Arc::<str>::from(value))
    }

    /// 创建测试对话
    fn make_thread(title: &str, updated_at: DateTime<Utc>) -> DbThread {
        DbThread {
            title: title.to_string().into(),
            messages: Vec::new(),
            updated_at,
            detailed_summary: None,
            initial_project_snapshot: None,
            cumulative_token_usage: Default::default(),
            request_token_usage: HashMap::default(),
            model: None,
            profile: None,
            imported: false,
            subagent_context: None,
            speed: None,
            thinking_enabled: false,
            thinking_effort: None,
            draft_prompt: None,
            ui_scroll_position: None,
        }
    }

    #[gpui::test]
    /// 测试对话列表按创建时间排序
    async fn test_list_threads_orders_by_created_at(cx: &mut TestAppContext) {
        let database = ThreadsDatabase::new(cx.executor()).unwrap();

        let older_id = session_id("thread-a");
        let newer_id = session_id("thread-b");

        let older_thread = make_thread(
            "Thread A",
            Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        );
        let newer_thread = make_thread(
            "Thread B",
            Utc.with_ymd_and_hms(2024, 1, 2, 0, 0, 0).unwrap(),
        );

        database
            .save_thread(older_id.clone(), older_thread, PathList::default())
            .await
            .unwrap();
        database
            .save_thread(newer_id.clone(), newer_thread, PathList::default())
            .await
            .unwrap();

        let entries = database.list_threads().await.unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, newer_id);
        assert_eq!(entries[1].id, older_id);
    }

    #[gpui::test]
    /// 测试保存对话会覆盖元数据
    async fn test_save_thread_replaces_metadata(cx: &mut TestAppContext) {
        let database = ThreadsDatabase::new(cx.executor()).unwrap();

        let thread_id = session_id("thread-a");
        let original_thread = make_thread(
            "Thread A",
            Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        );
        let updated_thread = make_thread(
            "Thread B",
            Utc.with_ymd_and_hms(2024, 1, 2, 0, 0, 0).unwrap(),
        );

        database
            .save_thread(thread_id.clone(), original_thread, PathList::default())
            .await
            .unwrap();
        database
            .save_thread(thread_id.clone(), updated_thread, PathList::default())
            .await
            .unwrap();

        let entries = database.list_threads().await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, thread_id);
        assert_eq!(entries[0].title.as_ref(), "Thread B");
        assert_eq!(
            entries[0].updated_at,
            Utc.with_ymd_and_hms(2024, 1, 2, 0, 0, 0).unwrap()
        );
        assert!(
            entries[0].created_at.is_some(),
            "created_at字段应被赋值"
        );
    }

    #[test]
    /// 测试子代理上下文默认值为None
    fn test_subagent_context_defaults_to_none() {
        let json = r#"{
            "title": "Old Thread",
            "messages": [],
            "updated_at": "2024-01-01T00:00:00Z"
        }"#;

        let db_thread: DbThread = serde_json::from_str(json).expect("反序列化失败");

        assert!(
            db_thread.subagent_context.is_none(),
            "无subagent_context字段的旧版对话应默认为None"
        );
    }

    #[test]
    /// 测试草稿提示默认值为None
    fn test_draft_prompt_defaults_to_none() {
        let json = r#"{
            "title": "Old Thread",
            "messages": [],
            "updated_at": "2024-01-01T00:00:00Z"
        }"#;

        let db_thread: DbThread = serde_json::from_str(json).expect("反序列化失败");

        assert!(
            db_thread.draft_prompt.is_none(),
            "无draft_prompt字段的旧版对话应默认为None"
        );
    }

    #[gpui::test]
    /// 测试子代理上下文在保存加载后正常往返
    async fn test_subagent_context_roundtrips_through_save_load(cx: &mut TestAppContext) {
        let database = ThreadsDatabase::new(cx.executor()).unwrap();

        let parent_id = session_id("parent-thread");
        let child_id = session_id("child-thread");

        let mut child_thread = make_thread(
            "Subagent Thread",
            Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        );
        child_thread.subagent_context = Some(crate::SubagentContext {
            parent_thread_id: parent_id.clone(),
            depth: 2,
        });

        database
            .save_thread(child_id.clone(), child_thread, PathList::default())
            .await
            .unwrap();

        let loaded = database
            .load_thread(child_id)
            .await
            .unwrap()
            .expect("对话应存在");

        let context = loaded
            .subagent_context
            .expect("subagent_context应被恢复");
        assert_eq!(context.parent_thread_id, parent_id);
        assert_eq!(context.depth, 2);
    }

    #[gpui::test]
    /// 测试普通对话无子代理上下文
    async fn test_non_subagent_thread_has_no_subagent_context(cx: &mut TestAppContext) {
        let database = ThreadsDatabase::new(cx.executor()).unwrap();

        let thread_id = session_id("regular-thread");
        let thread = make_thread(
            "Regular Thread",
            Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        );

        database
            .save_thread(thread_id.clone(), thread, PathList::default())
            .await
            .unwrap();

        let loaded = database
            .load_thread(thread_id)
            .await
            .unwrap()
            .expect("对话应存在");

        assert!(
            loaded.subagent_context.is_none(),
            "普通对话不应有subagent_context"
        );
    }

    #[gpui::test]
    /// 测试文件夹路径正常往返
    async fn test_folder_paths_roundtrip(cx: &mut TestAppContext) {
        let database = ThreadsDatabase::new(cx.executor()).unwrap();

        let thread_id = session_id("folder-thread");
        let thread = make_thread(
            "Folder Thread",
            Utc.with_ymd_and_hms(2024, 6, 15, 12, 0, 0).unwrap(),
        );

        let folder_paths = PathList::new(&[
            std::path::PathBuf::from("/home/user/project-a"),
            std::path::PathBuf::from("/home/user/project-b"),
        ]);

        database
            .save_thread(thread_id.clone(), thread, folder_paths.clone())
            .await
            .unwrap();

        let threads = database.list_threads().await.unwrap();
        assert_eq!(threads.len(), 1);
    }

    #[gpui::test]
    /// 测试未设置文件夹路径时为空
    async fn test_folder_paths_empty_when_not_set(cx: &mut TestAppContext) {
        let database = ThreadsDatabase::new(cx.executor()).unwrap();

        let thread_id = session_id("no-folder-thread");
        let thread = make_thread(
            "No Folder Thread",
            Utc.with_ymd_and_hms(2024, 6, 15, 12, 0, 0).unwrap(),
        );

        database
            .save_thread(thread_id.clone(), thread, PathList::default())
            .await
            .unwrap();

        let threads = database.list_threads().await.unwrap();
        assert_eq!(threads.len(), 1);
    }

    #[test]
    /// 测试滚动位置默认值为None
    fn test_scroll_position_defaults_to_none() {
        let json = r#"{
            "title": "Old Thread",
            "messages": [],
            "updated_at": "2024-01-01T00:00:00Z"
        }"#;

        let db_thread: DbThread = serde_json::from_str(json).expect("反序列化失败");

        assert!(
            db_thread.ui_scroll_position.is_none(),
            "无scroll_position字段的旧版对话应默认为None"
        );
    }

    #[gpui::test]
    /// 测试滚动位置在保存加载后正常往返
    async fn test_scroll_position_roundtrips_through_save_load(cx: &mut TestAppContext) {
        let database = ThreadsDatabase::new(cx.executor()).unwrap();

        let thread_id = session_id("thread-with-scroll");

        let mut thread = make_thread(
            "Thread With Scroll",
            Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap(),
        );
        thread.ui_scroll_position = Some(SerializedScrollPosition {
            item_ix: 42,
            offset_in_item: 13.5,
        });

        database
            .save_thread(thread_id.clone(), thread, PathList::default())
            .await
            .unwrap();

        let loaded = database
            .load_thread(thread_id)
            .await
            .unwrap()
            .expect("对话应存在");

        let scroll = loaded
            .ui_scroll_position
            .expect("scroll_position应被恢复");
        assert_eq!(scroll.item_ix, 42);
        assert!((scroll.offset_in_item - 13.5).abs() < f32::EPSILON);
    }
}