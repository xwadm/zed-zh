mod prompts;

use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use collections::HashMap;
use futures::FutureExt as _;
use futures::future::Shared;
use fuzzy::StringMatchCandidate;
use gpui::{
    App, AppContext, Context, Entity, EventEmitter, Global, ReadGlobal, SharedString, Task,
};
use heed::{
    Database, RoTxn,
    types::{SerdeBincode, SerdeJson, Str},
};
use parking_lot::RwLock;
pub use prompts::*;
use rope::Rope;
use serde::{Deserialize, Serialize};
use std::{
    cmp::Reverse,
    future::Future,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};
use strum::{EnumIter, IntoEnumIterator as _};
use text::LineEnding;
use util::ResultExt;
use uuid::Uuid;

/// 初始化在后台加载 PromptStore，并将一个共享的 future 赋值到全局。
pub fn init(cx: &mut App) {
    let db_path = paths::prompts_dir().join("prompts-library-db.0.mdb");
    let prompt_store_task = PromptStore::new(db_path, cx);
    let prompt_store_entity_task = cx
        .spawn(async move |cx| {
            prompt_store_task
                .await
                .map(|prompt_store| cx.new(|_cx| prompt_store))
                .map_err(Arc::new)
        })
        .shared();
    cx.set_global(GlobalPromptStore(prompt_store_entity_task))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PromptMetadata {
    pub id: PromptId,
    pub title: Option<SharedString>,
    pub default: bool,
    pub saved_at: DateTime<Utc>,
}

impl PromptMetadata {
    fn builtin(builtin: BuiltInPrompt) -> Self {
        Self {
            id: PromptId::BuiltIn(builtin),
            title: Some(builtin.title().into()),
            default: false,
            saved_at: DateTime::default(),
        }
    }
}

/// 内置提示，拥有默认内容并可由用户自定义。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, EnumIter)]
pub enum BuiltInPrompt {
    CommitMessage,
}

impl BuiltInPrompt {
    pub fn title(&self) -> &'static str {
        match self {
            Self::CommitMessage => "Commit message",
        }
    }

    /// 返回此内置提示的默认内容。
    pub fn default_content(&self) -> &'static str {
        match self {
            Self::CommitMessage => include_str!("../../git_ui/src/commit_message_prompt.txt"),
        }
    }
}

impl std::fmt::Display for BuiltInPrompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CommitMessage => write!(f, "Commit message"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum PromptId {
    User { uuid: UserPromptId },
    BuiltIn(BuiltInPrompt),
}

impl PromptId {
    pub fn new() -> PromptId {
        UserPromptId::new().into()
    }

    pub fn as_user(&self) -> Option<UserPromptId> {
        match self {
            Self::User { uuid } => Some(*uuid),
            Self::BuiltIn { .. } => None,
        }
    }

    pub fn as_built_in(&self) -> Option<BuiltInPrompt> {
        match self {
            Self::User { .. } => None,
            Self::BuiltIn(builtin) => Some(*builtin),
        }
    }

    pub fn is_built_in(&self) -> bool {
        matches!(self, Self::BuiltIn { .. })
    }

    pub fn can_edit(&self) -> bool {
        match self {
            Self::User { .. } => true,
            Self::BuiltIn(builtin) => match builtin {
                BuiltInPrompt::CommitMessage => true,
            },
        }
    }
}

impl From<BuiltInPrompt> for PromptId {
    fn from(builtin: BuiltInPrompt) -> Self {
        PromptId::BuiltIn(builtin)
    }
}

impl From<UserPromptId> for PromptId {
    fn from(uuid: UserPromptId) -> Self {
        PromptId::User { uuid }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UserPromptId(pub Uuid);

impl UserPromptId {
    pub fn new() -> UserPromptId {
        UserPromptId(Uuid::new_v4())
    }
}

impl From<Uuid> for UserPromptId {
    fn from(uuid: Uuid) -> Self {
        UserPromptId(uuid)
    }
}

impl std::fmt::Display for PromptId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PromptId::User { uuid } => write!(f, "{}", uuid.0),
            PromptId::BuiltIn(builtin) => write!(f, "{}", builtin),
        }
    }
}

pub struct PromptStore {
    env: heed::Env,
    metadata_cache: RwLock<MetadataCache>,
    metadata: Database<SerdeJson<PromptId>, SerdeJson<PromptMetadata>>,
    bodies: Database<SerdeJson<PromptId>, Str>,
}

pub struct PromptsUpdatedEvent;

impl EventEmitter<PromptsUpdatedEvent> for PromptStore {}

#[derive(Default)]
struct MetadataCache {
    metadata: Vec<PromptMetadata>,
    metadata_by_id: HashMap<PromptId, PromptMetadata>,
}

impl MetadataCache {
    fn from_db(
        db: Database<SerdeJson<PromptId>, SerdeJson<PromptMetadata>>,
        txn: &RoTxn,
    ) -> Result<Self> {
        let mut cache = MetadataCache::default();
        for result in db.iter(txn)? {
            // Fail-open: 对于无法解码的记录（例如来自不同分支的记录），将其跳过，而不是导致整个提示仓库初始化失败。
            let Ok((prompt_id, metadata)) = result else {
                log::warn!(
                    "跳过数据库中无法读取的提示记录: {:?}",
                    result.err()
                );
                continue;
            };
            cache.metadata.push(metadata.clone());
            cache.metadata_by_id.insert(prompt_id, metadata);
        }

        // 插入所有未被用户自定义的内置提示
        for builtin in BuiltInPrompt::iter() {
            let builtin_id = PromptId::BuiltIn(builtin);
            if !cache.metadata_by_id.contains_key(&builtin_id) {
                let metadata = PromptMetadata::builtin(builtin);
                cache.metadata.push(metadata.clone());
                cache.metadata_by_id.insert(builtin_id, metadata);
            }
        }
        cache.sort();
        Ok(cache)
    }

    fn insert(&mut self, metadata: PromptMetadata) {
        self.metadata_by_id.insert(metadata.id, metadata.clone());
        if let Some(old_metadata) = self.metadata.iter_mut().find(|m| m.id == metadata.id) {
            *old_metadata = metadata;
        } else {
            self.metadata.push(metadata);
        }
        self.sort();
    }

    fn remove(&mut self, id: PromptId) {
        self.metadata.retain(|metadata| metadata.id != id);
        self.metadata_by_id.remove(&id);
    }

    fn sort(&mut self) {
        self.metadata.sort_unstable_by(|a, b| {
            a.title
                .cmp(&b.title)
                .then_with(|| b.saved_at.cmp(&a.saved_at))
        });
    }
}

impl PromptStore {
    pub fn global(cx: &App) -> impl Future<Output = Result<Entity<Self>>> + use<> {
        let store = GlobalPromptStore::global(cx).0.clone();
        async move { store.await.map_err(|err| anyhow!(err)) }
    }

    pub fn new(db_path: PathBuf, cx: &App) -> Task<Result<Self>> {
        cx.background_spawn(async move {
            std::fs::create_dir_all(&db_path)?;

            let db_env = unsafe {
                heed::EnvOpenOptions::new()
                    .map_size(1024 * 1024 * 1024) // 1GB
                    .max_dbs(4) // 元数据和正文（可能还包含两者的 v1 版本）
                    .open(db_path)?
            };

            let mut txn = db_env.write_txn()?;
            let metadata = db_env.create_database(&mut txn, Some("metadata.v2"))?;
            let bodies = db_env.create_database(&mut txn, Some("bodies.v2"))?;
            txn.commit()?;

            Self::upgrade_dbs(&db_env, metadata, bodies).log_err();

            let txn = db_env.read_txn()?;
            let metadata_cache = MetadataCache::from_db(metadata, &txn)?;
            txn.commit()?;

            Ok(PromptStore {
                env: db_env,
                metadata_cache: RwLock::new(metadata_cache),
                metadata,
                bodies,
            })
        })
    }

    fn upgrade_dbs(
        env: &heed::Env,
        metadata_db: heed::Database<SerdeJson<PromptId>, SerdeJson<PromptMetadata>>,
        bodies_db: heed::Database<SerdeJson<PromptId>, Str>,
    ) -> Result<()> {
        let mut txn = env.write_txn()?;
        let Some(bodies_v1_db) = env
            .open_database::<SerdeBincode<PromptIdV1>, SerdeBincode<String>>(
                &txn,
                Some("bodies"),
            )?
        else {
            return Ok(());
        };
        let mut bodies_v1 = bodies_v1_db
            .iter(&txn)?
            .collect::<heed::Result<HashMap<_, _>>>()?;

        let Some(metadata_v1_db) = env
            .open_database::<SerdeBincode<PromptIdV1>, SerdeBincode<PromptMetadataV1>>(
                &txn,
                Some("metadata"),
            )?
        else {
            return Ok(());
        };
        let metadata_v1 = metadata_v1_db
            .iter(&txn)?
            .collect::<heed::Result<HashMap<_, _>>>()?;

        for (prompt_id_v1, metadata_v1) in metadata_v1 {
            let prompt_id_v2 = UserPromptId(prompt_id_v1.0).into();
            let Some(body_v1) = bodies_v1.remove(&prompt_id_v1) else {
                continue;
            };

            if metadata_db
                .get(&txn, &prompt_id_v2)?
                .is_none_or(|metadata_v2| metadata_v1.saved_at > metadata_v2.saved_at)
            {
                metadata_db.put(
                    &mut txn,
                    &prompt_id_v2,
                    &PromptMetadata {
                        id: prompt_id_v2,
                        title: metadata_v1.title.clone(),
                        default: metadata_v1.default,
                        saved_at: metadata_v1.saved_at,
                    },
                )?;
                bodies_db.put(&mut txn, &prompt_id_v2, &body_v1)?;
            }
        }

        txn.commit()?;

        Ok(())
    }

    pub fn load(&self, id: PromptId, cx: &App) -> Task<Result<String>> {
        let env = self.env.clone();
        let bodies = self.bodies;
        cx.background_spawn(async move {
            let txn = env.read_txn()?;
            let mut prompt: String = match bodies.get(&txn, &id)? {
                Some(body) => body.into(),
                None => {
                    if let Some(built_in) = id.as_built_in() {
                        built_in.default_content().into()
                    } else {
                        anyhow::bail!("提示未找到")
                    }
                }
            };
            LineEnding::normalize(&mut prompt);
            Ok(prompt)
        })
    }

    pub fn all_prompt_metadata(&self) -> Vec<PromptMetadata> {
        self.metadata_cache.read().metadata.clone()
    }

    pub fn default_prompt_metadata(&self) -> Vec<PromptMetadata> {
        return self
            .metadata_cache
            .read()
            .metadata
            .iter()
            .filter(|metadata| metadata.default)
            .cloned()
            .collect::<Vec<_>>();
    }

    pub fn delete(&self, id: PromptId, cx: &Context<Self>) -> Task<Result<()>> {
        self.metadata_cache.write().remove(id);

        let db_connection = self.env.clone();
        let bodies = self.bodies;
        let metadata = self.metadata;

        let task = cx.background_spawn(async move {
            let mut txn = db_connection.write_txn()?;

            metadata.delete(&mut txn, &id)?;
            bodies.delete(&mut txn, &id)?;

            if let PromptId::User { uuid } = id {
                let prompt_id_v1 = PromptIdV1::from(uuid);

                if let Some(metadata_v1_db) = db_connection
                    .open_database::<SerdeBincode<PromptIdV1>, SerdeBincode<()>>(
                        &txn,
                        Some("metadata"),
                    )?
                {
                    metadata_v1_db.delete(&mut txn, &prompt_id_v1)?;
                }

                if let Some(bodies_v1_db) = db_connection
                    .open_database::<SerdeBincode<PromptIdV1>, SerdeBincode<()>>(
                        &txn,
                        Some("bodies"),
                    )?
                {
                    bodies_v1_db.delete(&mut txn, &prompt_id_v1)?;
                }
            }

            txn.commit()?;
            anyhow::Ok(())
        });

        cx.spawn(async move |this, cx| {
            task.await?;
            this.update(cx, |_, cx| cx.emit(PromptsUpdatedEvent)).ok();
            anyhow::Ok(())
        })
    }

    pub fn metadata(&self, id: PromptId) -> Option<PromptMetadata> {
        self.metadata_cache.read().metadata_by_id.get(&id).cloned()
    }

    pub fn first(&self) -> Option<PromptMetadata> {
        self.metadata_cache.read().metadata.first().cloned()
    }

    pub fn id_for_title(&self, title: &str) -> Option<PromptId> {
        let metadata_cache = self.metadata_cache.read();
        let metadata = metadata_cache
            .metadata
            .iter()
            .find(|metadata| metadata.title.as_ref().map(|title| &***title) == Some(title))?;
        Some(metadata.id)
    }

    pub fn search(
        &self,
        query: String,
        cancellation_flag: Arc<AtomicBool>,
        cx: &App,
    ) -> Task<Vec<PromptMetadata>> {
        let cached_metadata = self.metadata_cache.read().metadata.clone();
        let executor = cx.background_executor().clone();
        cx.background_spawn(async move {
            let mut matches = if query.is_empty() {
                cached_metadata
            } else {
                let candidates = cached_metadata
                    .iter()
                    .enumerate()
                    .filter_map(|(ix, metadata)| {
                        Some(StringMatchCandidate::new(ix, metadata.title.as_ref()?))
                    })
                    .collect::<Vec<_>>();
                let matches = fuzzy::match_strings(
                    &candidates,
                    &query,
                    false,
                    true,
                    100,
                    &cancellation_flag,
                    executor,
                )
                .await;
                matches
                    .into_iter()
                    .map(|mat| cached_metadata[mat.candidate_id].clone())
                    .collect()
            };
            matches.sort_by_key(|metadata| Reverse(metadata.default));
            matches
        })
    }

    pub fn save(
        &self,
        id: PromptId,
        title: Option<SharedString>,
        default: bool,
        body: Rope,
        cx: &Context<Self>,
    ) -> Task<Result<()>> {
        if !id.can_edit() {
            return Task::ready(Err(anyhow!("此提示无法编辑")));
        }

        let body = body.to_string();
        let is_default_content = id
            .as_built_in()
            .is_some_and(|builtin| body.trim() == builtin.default_content().trim());

        let metadata = if let Some(builtin) = id.as_built_in() {
            PromptMetadata::builtin(builtin)
        } else {
            PromptMetadata {
                id,
                title,
                default,
                saved_at: Utc::now(),
            }
        };

        self.metadata_cache.write().insert(metadata.clone());

        let db_connection = self.env.clone();
        let bodies = self.bodies;
        let metadata_db = self.metadata;

        let task = cx.background_spawn(async move {
            let mut txn = db_connection.write_txn()?;

            if is_default_content {
                metadata_db.delete(&mut txn, &id)?;
                bodies.delete(&mut txn, &id)?;
            } else {
                metadata_db.put(&mut txn, &id, &metadata)?;
                bodies.put(&mut txn, &id, &body)?;
            }

            txn.commit()?;

            anyhow::Ok(())
        });

        cx.spawn(async move |this, cx| {
            task.await?;
            this.update(cx, |_, cx| cx.emit(PromptsUpdatedEvent)).ok();
            anyhow::Ok(())
        })
    }

    pub fn save_metadata(
        &self,
        id: PromptId,
        mut title: Option<SharedString>,
        default: bool,
        cx: &Context<Self>,
    ) -> Task<Result<()>> {
        let mut cache = self.metadata_cache.write();

        if !id.can_edit() {
            title = cache
                .metadata_by_id
                .get(&id)
                .and_then(|metadata| metadata.title.clone());
        }

        let prompt_metadata = PromptMetadata {
            id,
            title,
            default,
            saved_at: Utc::now(),
        };

        cache.insert(prompt_metadata.clone());

        let db_connection = self.env.clone();
        let metadata = self.metadata;

        let task = cx.background_spawn(async move {
            let mut txn = db_connection.write_txn()?;
            metadata.put(&mut txn, &id, &prompt_metadata)?;
            txn.commit()?;

            anyhow::Ok(())
        });

        cx.spawn(async move |this, cx| {
            task.await?;
            this.update(cx, |_, cx| cx.emit(PromptsUpdatedEvent)).ok();
            anyhow::Ok(())
        })
    }
}

/// 已弃用: 旧版 V1 提示 ID 格式，仅用于从旧数据库迁移数据。请使用 `PromptId` 代替。
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Hash)]
struct PromptIdV1(Uuid);

impl From<UserPromptId> for PromptIdV1 {
    fn from(id: UserPromptId) -> Self {
        PromptIdV1(id.0)
    }
}

/// 已弃用: 旧版 V1 提示元数据格式，仅用于从旧数据库迁移数据。请使用 `PromptMetadata` 代替。
#[derive(Clone, Debug, Serialize, Deserialize)]
struct PromptMetadataV1 {
    id: PromptIdV1,
    title: Option<SharedString>,
    default: bool,
    saved_at: DateTime<Utc>,
}

/// 将一个共享的 future 包装为提示仓库，以便其作为上下文全局变量使用。
pub struct GlobalPromptStore(Shared<Task<Result<Entity<PromptStore>, Arc<anyhow::Error>>>>);

impl Global for GlobalPromptStore {}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    async fn test_built_in_prompt_load_save(cx: &mut TestAppContext) {
        cx.executor().allow_parking();

        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("prompts-db");

        let store = cx.update(|cx| PromptStore::new(db_path, cx)).await.unwrap();
        let store = cx.new(|_cx| store);

        let commit_message_id = PromptId::BuiltIn(BuiltInPrompt::CommitMessage);

        let loaded_content = store
            .update(cx, |store, cx| store.load(commit_message_id, cx))
            .await
            .unwrap();

        let mut expected_content = BuiltInPrompt::CommitMessage.default_content().to_string();
        LineEnding::normalize(&mut expected_content);
        assert_eq!(
            loaded_content.trim(),
            expected_content.trim(),
            "加载不在数据库中的内置提示应返回默认内容"
        );

        let metadata = store.read_with(cx, |store, _| store.metadata(commit_message_id));
        assert!(
            metadata.is_some(),
            "内置提示应始终拥有元数据"
        );
        assert!(
            store.read_with(cx, |store, _| {
                store
                    .metadata_cache
                    .read()
                    .metadata_by_id
                    .contains_key(&commit_message_id)
            }),
            "内置提示应始终在缓存中"
        );

        let custom_content = "Custom commit message prompt";
        store
            .update(cx, |store, cx| {
                store.save(
                    commit_message_id,
                    Some("Commit message".into()),
                    false,
                    Rope::from(custom_content),
                    cx,
                )
            })
            .await
            .unwrap();

        let loaded_custom = store
            .update(cx, |store, cx| store.load(commit_message_id, cx))
            .await
            .unwrap();
        assert_eq!(
            loaded_custom.trim(),
            custom_content.trim(),
            "保存后应加载自定义内容"
        );

        assert!(
            store
                .read_with(cx, |store, _| store.metadata(commit_message_id))
                .is_some(),
            "自定义后内置提示应拥有元数据"
        );

        store
            .update(cx, |store, cx| {
                store.save(
                    commit_message_id,
                    Some("Commit message".into()),
                    false,
                    Rope::from(BuiltInPrompt::CommitMessage.default_content()),
                    cx,
                )
            })
            .await
            .unwrap();

        let metadata_after_reset =
            store.read_with(cx, |store, _| store.metadata(commit_message_id));
        assert!(
            metadata_after_reset.is_some(),
            "重置后内置提示应仍拥有元数据"
        );
        assert_eq!(
            metadata_after_reset
                .as_ref()
                .and_then(|m| m.title.as_ref().map(|t| t.as_ref())),
            Some("Commit message"),
            "重置后内置提示应有默认标题"
        );

        let loaded_after_reset = store
            .update(cx, |store, cx| store.load(commit_message_id, cx))
            .await
            .unwrap();
        let mut expected_content_after_reset =
            BuiltInPrompt::CommitMessage.default_content().to_string();
        LineEnding::normalize(&mut expected_content_after_reset);
        assert_eq!(
            loaded_after_reset.trim(),
            expected_content_after_reset.trim(),
            "保存默认内容后内容应恢复为默认值"
        );
    }

    /// 测试即使数据库包含不兼容/无法解码的 PromptId 键（例如来自使用不同序列化格式的分支），提示仓库也能成功初始化。
    ///
    /// 这是对 "fail-open" 行为的回归测试：我们应该跳过坏记录，而不是导致整个仓库初始化失败。
    #[gpui::test]
    async fn test_prompt_store_handles_incompatible_db_records(cx: &mut TestAppContext) {
        cx.executor().allow_parking();

        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("prompts-db-with-bad-records");
        std::fs::create_dir_all(&db_path).unwrap();

        // 首先，创建数据库并直接写入一条不兼容的记录。
        // 我们模拟一条由不同分支写入的记录，该分支使用了
        // `{"kind":"CommitMessage"}` 而非 `{"kind":"BuiltIn", ...}`。
        {
            let db_env = unsafe {
                heed::EnvOpenOptions::new()
                    .map_size(1024 * 1024 * 1024)
                    .max_dbs(4)
                    .open(&db_path)
                    .unwrap()
            };

            let mut txn = db_env.write_txn().unwrap();
            // 使用原始字节创建 metadata.v2 数据库，以便写入
            // 不兼容的键格式。
            let metadata_db: Database<heed::types::Bytes, heed::types::Bytes> = db_env
                .create_database(&mut txn, Some("metadata.v2"))
                .unwrap();

            // 写入一个不兼容的 PromptId 键: `{"kind":"CommitMessage"}`
            // 这是当前代码无法解码的旧/分支格式。
            let bad_key = br#"{"kind":"CommitMessage"}"#;
            let dummy_metadata = br#"{"id":{"kind":"CommitMessage"},"title":"Bad Record","default":false,"saved_at":"2024-01-01T00:00:00Z"}"#;
            metadata_db.put(&mut txn, bad_key, dummy_metadata).unwrap();

            // 同时写入一条有效记录，以确保我们仍能读取正确的数据。
            let good_key = br#"{"kind":"User","uuid":"550e8400-e29b-41d4-a716-446655440000"}"#;
            let good_metadata = br#"{"id":{"kind":"User","uuid":"550e8400-e29b-41d4-a716-446655440000"},"title":"Good Record","default":false,"saved_at":"2024-01-01T00:00:00Z"}"#;
            metadata_db.put(&mut txn, good_key, good_metadata).unwrap();

            txn.commit().unwrap();
        }

        // 现在尝试从这个数据库创建 PromptStore。
        // 凭借 fail-open 行为，这次操作应该成功并跳过坏记录。
        // 如果没有 fail-open，将会返回错误。
        let store_result = cx.update(|cx| PromptStore::new(db_path, cx)).await;

        assert!(
            store_result.is_ok(),
            "即使存在不兼容的数据库记录，PromptStore 也应该成功初始化。\n     遇到错误: {:?}",
            store_result.err()
        );

        let store = cx.new(|_cx| store_result.unwrap());

        // 验证有效记录已加载。
        let good_id = PromptId::User {
            uuid: UserPromptId("550e8400-e29b-41d4-a716-446655440000".parse().unwrap()),
        };
        let metadata = store.read_with(cx, |store, _| store.metadata(good_id));
        assert!(
            metadata.is_some(),
            "跳过坏记录后，有效记录仍应被加载"
        );
        assert_eq!(
            metadata
                .as_ref()
                .and_then(|m| m.title.as_ref().map(|t| t.as_ref())),
            Some("Good Record"),
            "有效记录应具有正确的标题"
        );
    }

    #[gpui::test]
    async fn test_deleted_prompt_does_not_reappear_after_migration(cx: &mut TestAppContext) {
        cx.executor().allow_parking();

        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("prompts-db-v1-migration");
        std::fs::create_dir_all(&db_path).unwrap();

        let prompt_uuid: Uuid = "550e8400-e29b-41d4-a716-446655440001".parse().unwrap();
        let prompt_id_v1 = PromptIdV1(prompt_uuid);
        let prompt_id_v2 = PromptId::User {
            uuid: UserPromptId(prompt_uuid),
        };

        // 创建带有提示的 V1 数据库
        {
            let db_env = unsafe {
                heed::EnvOpenOptions::new()
                    .map_size(1024 * 1024 * 1024)
                    .max_dbs(4)
                    .open(&db_path)
                    .unwrap()
            };

            let mut txn = db_env.write_txn().unwrap();

            let metadata_v1_db: Database<SerdeBincode<PromptIdV1>, SerdeBincode<PromptMetadataV1>> =
                db_env.create_database(&mut txn, Some("metadata")).unwrap();

            let bodies_v1_db: Database<SerdeBincode<PromptIdV1>, SerdeBincode<String>> =
                db_env.create_database(&mut txn, Some("bodies")).unwrap();

            let metadata_v1 = PromptMetadataV1 {
                id: prompt_id_v1.clone(),
                title: Some("V1 Prompt".into()),
                default: false,
                saved_at: Utc::now(),
            };

            metadata_v1_db
                .put(&mut txn, &prompt_id_v1, &metadata_v1)
                .unwrap();
            bodies_v1_db
                .put(&mut txn, &prompt_id_v1, &"V1 prompt body".to_string())
                .unwrap();

            txn.commit().unwrap();
        }

        // 通过创建 PromptStore 将 V1 迁移到 V2
        let store = cx
            .update(|cx| PromptStore::new(db_path.clone(), cx))
            .await
            .unwrap();
        let store = cx.new(|_cx| store);

        // 验证提示已迁移
        let metadata = store.read_with(cx, |store, _| store.metadata(prompt_id_v2));
        assert!(metadata.is_some(), "V1 提示应已迁移到 V2");
        assert_eq!(
            metadata
                .as_ref()
                .and_then(|m| m.title.as_ref().map(|t| t.as_ref())),
            Some("V1 Prompt"),
            "迁移后的提示应具有正确的标题"
        );

        // 删除提示
        store
            .update(cx, |store, cx| store.delete(prompt_id_v2, cx))
            .await
            .unwrap();

        // 验证提示已删除
        let metadata_after_delete = store.read_with(cx, |store, _| store.metadata(prompt_id_v2));
        assert!(
            metadata_after_delete.is_none(),
            "提示应从 V2 中删除"
        );

        drop(store);

        // 通过从相同路径创建新的 PromptStore 来“重启”
        let store_after_restart = cx.update(|cx| PromptStore::new(db_path, cx)).await.unwrap();
        let store_after_restart = cx.new(|_cx| store_after_restart);

        // 测试提示不会重新出现
        let metadata_after_restart =
            store_after_restart.read_with(cx, |store, _| store.metadata(prompt_id_v2));
        assert!(
            metadata_after_restart.is_none(),
            "已删除的提示不应在重启/迁移后重新出现"
        );
    }
}