use crate::{
    LanguageModel, LanguageModelId, LanguageModelProvider, LanguageModelProviderId,
    LanguageModelProviderState, ZED_CLOUD_PROVIDER_ID,
};
use collections::{BTreeMap, HashSet};
use gpui::{App, Context, Entity, EventEmitter, Global, prelude::*};
use std::{str::FromStr, sync::Arc};
use thiserror::Error;

/// 检查内置提供商是否需要隐藏的函数类型
/// 当安装指定扩展时返回 Some(扩展ID)，表示该内置提供商需要隐藏
pub type BuiltinProviderHidingFn = Box<dyn Fn(&str) -> Option<&'static str> + Send + Sync>;

/// 初始化大语言模型注册中心
pub fn init(cx: &mut App) {
    let registry = cx.new(|_cx| LanguageModelRegistry::default());
    cx.set_global(GlobalLanguageModelRegistry(registry));
}

/// 全局注册中心包装
struct GlobalLanguageModelRegistry(Entity<LanguageModelRegistry>);

impl Global for GlobalLanguageModelRegistry {}

/// 配置错误类型
#[derive(Error)]
pub enum ConfigurationError {
    #[error("请至少配置一个大语言模型提供商以使用面板功能。")]
    NoProvider,

    #[error("大语言模型提供商未配置或不支持当前选择的模型。")]
    ModelNotFound,

    #[error("{} 大语言模型提供商未完成认证。", .0.name().0)]
    ProviderNotAuthenticated(Arc<dyn LanguageModelProvider>),
}

impl std::fmt::Debug for ConfigurationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoProvider => write!(f, "NoProvider"),
            Self::ModelNotFound => write!(f, "ModelNotFound"),
            Self::ProviderNotAuthenticated(provider) => {
                write!(f, "ProviderNotAuthenticated({})", provider.id())
            }
        }
    }
}

/// 大语言模型注册中心
/// 负责管理所有模型提供商、默认模型、专用模型（行内助手、提交信息、对话总结）
#[derive(Default)]
pub struct LanguageModelRegistry {
    /// 用户手动设置的默认模型
    default_model: Option<ConfiguredModel>,

    /// 环境自动配置的备用模型（仅在无手动默认模型时使用）
    available_fallback_model: Option<ConfiguredModel>,

    /// 行内助手专用模型
    inline_assistant_model: Option<ConfiguredModel>,

    /// 提交信息生成专用模型
    commit_message_model: Option<ConfiguredModel>,

    /// 对话总结专用模型
    thread_summary_model: Option<ConfiguredModel>,

    /// 所有已注册的模型提供商
    providers: BTreeMap<LanguageModelProviderId, Arc<dyn LanguageModelProvider>>,

    /// 行内补全备选模型列表
    inline_alternatives: Vec<Arc<dyn LanguageModel>>,

    /// 已安装的 LLM 扩展 ID 集合
    installed_llm_extension_ids: HashSet<Arc<str>>,

    /// 内置提供商隐藏规则函数
    builtin_provider_hiding_fn: Option<BuiltinProviderHidingFn>,
}

/// 用户选择的模型（由 提供商ID/模型ID 组成）
#[derive(Debug)]
pub struct SelectedModel {
    pub provider: LanguageModelProviderId,
    pub model: LanguageModelId,
}

impl FromStr for SelectedModel {
    type Err = String;

    /// 解析 `提供商ID/模型ID` 格式的字符串为选择模型
    fn from_str(id: &str) -> Result<SelectedModel, Self::Err> {
        let parts: Vec<&str> = id.split('/').collect();
        let [provider_id, model_id] = parts.as_slice() else {
            return Err(format!(
                "无效的模型标识格式：`{}`。期望格式为 `提供商ID/模型ID`",
                id
            ));
        };

        if provider_id.is_empty() || model_id.is_empty() {
            return Err(format!("提供商和模型 ID 不能为空：`{}`", id));
        }

        Ok(SelectedModel {
            provider: LanguageModelProviderId(provider_id.to_string().into()),
            model: LanguageModelId(model_id.to_string().into()),
        })
    }
}

/// 已配置完成的模型（包含提供商 + 模型实例）
#[derive(Clone)]
pub struct ConfiguredModel {
    pub provider: Arc<dyn LanguageModelProvider>,
    pub model: Arc<dyn LanguageModel>,
}

impl ConfiguredModel {
    /// 判断是否为同一个模型
    pub fn is_same_as(&self, other: &ConfiguredModel) -> bool {
        self.model.id() == other.model.id() && self.provider.id() == other.provider.id()
    }

    /// 是否由 Zed 官方云提供
    pub fn is_provided_by_zed(&self) -> bool {
        self.provider.id() == ZED_CLOUD_PROVIDER_ID
    }
}

/// 注册中心事件
pub enum Event {
    /// 默认模型已变更
    DefaultModelChanged,
    /// 行内助手模型已变更
    InlineAssistantModelChanged,
    /// 提交信息模型已变更
    CommitMessageModelChanged,
    /// 对话总结模型已变更
    ThreadSummaryModelChanged,
    /// 提供商状态已变更
    ProviderStateChanged(LanguageModelProviderId),
    /// 新增提供商
    AddedProvider(LanguageModelProviderId),
    /// 移除提供商
    RemovedProvider(LanguageModelProviderId),
    /// 提供商可见性发生变化（由扩展安装/卸载触发）
    ProvidersChanged,
}

impl EventEmitter<Event> for LanguageModelRegistry {}

impl LanguageModelRegistry {
    /// 获取全局注册中心实体
    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalLanguageModelRegistry>().0.clone()
    }

    /// 读取全局注册中心
    pub fn read_global(cx: &App) -> &Self {
        cx.global::<GlobalLanguageModelRegistry>().0.read(cx)
    }

    /// 测试环境：创建测试用注册中心与假提供商
    #[cfg(any(test, feature = "test-support"))]
    pub fn test(cx: &mut App) -> Arc<crate::fake_provider::FakeLanguageModelProvider> {
        let fake_provider = Arc::new(crate::fake_provider::FakeLanguageModelProvider::default());
        let registry = cx.new(|cx| {
            let mut registry = Self::default();
            registry.register_provider(fake_provider.clone(), cx);
            let model = fake_provider.provided_models(cx)[0].clone();
            let configured_model = ConfiguredModel {
                provider: fake_provider.clone(),
                model,
            };
            registry.set_default_model(Some(configured_model), cx);
            registry
        });
        cx.set_global(GlobalLanguageModelRegistry(registry));
        fake_provider
    }

    /// 测试环境：获取假模型
    #[cfg(any(test, feature = "test-support"))]
    pub fn fake_model(&self) -> Arc<dyn LanguageModel> {
        self.default_model.as_ref().unwrap().model.clone()
    }

    /// 注册模型提供商
    pub fn register_provider<T: LanguageModelProvider + LanguageModelProviderState>(
        &mut self,
        provider: Arc<T>,
        cx: &mut Context<Self>,
    ) {
        let id = provider.id();

        // 订阅提供商状态变化
        let subscription = provider.subscribe(cx, {
            let id = id.clone();
            move |_, cx| {
                cx.emit(Event::ProviderStateChanged(id.clone()));
            }
        });
        if let Some(subscription) = subscription {
            subscription.detach();
        }

        self.providers.insert(id.clone(), provider);
        cx.emit(Event::AddedProvider(id));
    }

    /// 注销模型提供商
    pub fn unregister_provider(&mut self, id: LanguageModelProviderId, cx: &mut Context<Self>) {
        if self.providers.remove(&id).is_some() {
            cx.emit(Event::RemovedProvider(id));
        }
    }

    /// 获取所有已注册的提供商（zed.dev 优先）
    pub fn providers(&self) -> Vec<Arc<dyn LanguageModelProvider>> {
        let zed_provider_id = LanguageModelProviderId("zed.dev".into());
        let mut providers = Vec::with_capacity(self.providers.len());

        // 优先加入 Zed 官方提供商
        if let Some(provider) = self.providers.get(&zed_provider_id) {
            providers.push(provider.clone());
        }

        // 加入其他所有提供商
        providers.extend(self.providers.values().filter_map(|p| {
            if p.id() != zed_provider_id {
                Some(p.clone())
            } else {
                None
            }
        }));
        providers
    }

    /// 获取可见的提供商（过滤掉被扩展隐藏的内置提供商）
    pub fn visible_providers(&self) -> Vec<Arc<dyn LanguageModelProvider>> {
        self.providers()
            .into_iter()
            .filter(|p| !self.should_hide_provider(&p.id()))
            .collect()
    }

    /// 设置内置提供商隐藏规则函数
    pub fn set_builtin_provider_hiding_fn(&mut self, hiding_fn: BuiltinProviderHidingFn) {
        self.builtin_provider_hiding_fn = Some(hiding_fn);
    }

    /// 扩展已安装：如果是 LLM 扩展，记录并更新可见性
    pub fn extension_installed(&mut self, extension_id: Arc<str>, cx: &mut Context<Self>) {
        if self.installed_llm_extension_ids.insert(extension_id) {
            cx.emit(Event::ProvidersChanged);
            cx.notify();
        }
    }

    /// 扩展已卸载：移除记录并更新可见性
    pub fn extension_uninstalled(&mut self, extension_id: &str, cx: &mut Context<Self>) {
        if self.installed_llm_extension_ids.remove(extension_id) {
            cx.emit(Event::ProvidersChanged);
            cx.notify();
        }
    }

    /// 同步已安装的 LLM 扩展列表
    pub fn sync_installed_llm_extensions(
        &mut self,
        extension_ids: HashSet<Arc<str>>,
        cx: &mut Context<Self>,
    ) {
        if extension_ids != self.installed_llm_extension_ids {
            self.installed_llm_extension_ids = extension_ids;
            cx.emit(Event::ProvidersChanged);
            cx.notify();
        }
    }

    /// 判断某个提供商是否应该在 UI 中隐藏
    pub fn should_hide_provider(&self, provider_id: &LanguageModelProviderId) -> bool {
        if let Some(ref hiding_fn) = self.builtin_provider_hiding_fn {
            if let Some(extension_id) = hiding_fn(&provider_id.0) {
                return self.installed_llm_extension_ids.contains(extension_id);
            }
        }
        false
    }

    /// 获取模型配置错误（如果有）
    pub fn configuration_error(
        &self,
        model: Option<ConfiguredModel>,
        cx: &App,
    ) -> Option<ConfigurationError> {
        let Some(model) = model else {
            if !self.has_authenticated_provider(cx) {
                return Some(ConfigurationError::NoProvider);
            }
            return Some(ConfigurationError::ModelNotFound);
        };

        if !model.provider.is_authenticated(cx) {
            return Some(ConfigurationError::ProviderNotAuthenticated(model.provider));
        }

        None
    }

    /// 是否存在至少一个已认证的提供商
    pub fn has_authenticated_provider(&self, cx: &App) -> bool {
        self.providers.values().any(|p| p.is_authenticated(cx))
    }

    /// 获取所有可用模型（已认证的提供商提供的模型）
    pub fn available_models<'a>(
        &'a self,
        cx: &'a App,
    ) -> impl Iterator<Item = Arc<dyn LanguageModel>> + 'a {
        self.providers
            .values()
            .filter(|provider| provider.is_authenticated(cx))
            .flat_map(|provider| provider.provided_models(cx))
    }

    /// 根据 ID 获取提供商
    pub fn provider(&self, id: &LanguageModelProviderId) -> Option<Arc<dyn LanguageModelProvider>> {
        self.providers.get(id).cloned()
    }

    // -------------------------------------------------------------------------
    // 选择/设置各类模型
    // -------------------------------------------------------------------------

    /// 选择默认模型
    pub fn select_default_model(&mut self, model: Option<&SelectedModel>, cx: &mut Context<Self>) {
        let configured_model = model.and_then(|model| self.select_model(model, cx));
        self.set_default_model(configured_model, cx);
    }

    /// 选择行内助手模型
    pub fn select_inline_assistant_model(
        &mut self,
        model: Option<&SelectedModel>,
        cx: &mut Context<Self>,
    ) {
        let configured_model = model.and_then(|model| self.select_model(model, cx));
        self.set_inline_assistant_model(configured_model, cx);
    }

    /// 选择提交信息模型
    pub fn select_commit_message_model(
        &mut self,
        model: Option<&SelectedModel>,
        cx: &mut Context<Self>,
    ) {
        let configured_model = model.and_then(|model| self.select_model(model, cx));
        self.set_commit_message_model(configured_model, cx);
    }

    /// 选择对话总结模型
    pub fn select_thread_summary_model(
        &mut self,
        model: Option<&SelectedModel>,
        cx: &mut Context<Self>,
    ) {
        let configured_model = model.and_then(|model| self.select_model(model, cx));
        self.set_thread_summary_model(configured_model, cx);
    }

    /// 设置行内补全备选模型
    pub fn select_inline_alternative_models(
        &mut self,
        alternatives: impl IntoIterator<Item = SelectedModel>,
        cx: &mut Context<Self>,
    ) {
        self.inline_alternatives = alternatives
            .into_iter()
            .flat_map(|alternative| {
                self.select_model(&alternative, cx)
                    .map(|configured_model| configured_model.model)
            })
            .collect::<Vec<_>>();
    }

    /// 根据选择项获取完整的配置模型
    pub fn select_model(
        &mut self,
        selected_model: &SelectedModel,
        cx: &mut Context<Self>,
    ) -> Option<ConfiguredModel> {
        let provider = self.provider(&selected_model.provider)?;
        let model = provider
            .provided_models(cx)
            .iter()
            .find(|model| model.id() == selected_model.model)?
            .clone();
        Some(ConfiguredModel { provider, model })
    }

    // -------------------------------------------------------------------------
    // 设置各类模型（内部方法）
    // -------------------------------------------------------------------------

    /// 设置默认模型
    pub fn set_default_model(&mut self, model: Option<ConfiguredModel>, cx: &mut Context<Self>) {
        match (self.default_model(), model.as_ref()) {
            (Some(old), Some(new)) if old.is_same_as(new) => {}
            (None, None) => {}
            _ => cx.emit(Event::DefaultModelChanged),
        }
        self.default_model = model;
    }

    /// 设置环境备用模型
    pub fn set_environment_fallback_model(
        &mut self,
        model: Option<ConfiguredModel>,
        cx: &mut Context<Self>,
    ) {
        if self.default_model.is_none() {
            match (self.available_fallback_model.as_ref(), model.as_ref()) {
                (Some(old), Some(new)) if old.is_same_as(new) => {}
                (None, None) => {}
                _ => cx.emit(Event::DefaultModelChanged),
            }
        }
        self.available_fallback_model = model;
    }

    /// 设置行内助手模型
    pub fn set_inline_assistant_model(
        &mut self,
        model: Option<ConfiguredModel>,
        cx: &mut Context<Self>,
    ) {
        match (self.inline_assistant_model.as_ref(), model.as_ref()) {
            (Some(old), Some(new)) if old.is_same_as(new) => {}
            (None, None) => {}
            _ => cx.emit(Event::InlineAssistantModelChanged),
        }
        self.inline_assistant_model = model;
    }

    /// 设置提交信息模型
    pub fn set_commit_message_model(
        &mut self,
        model: Option<ConfiguredModel>,
        cx: &mut Context<Self>,
    ) {
        match (self.commit_message_model.as_ref(), model.as_ref()) {
            (Some(old), Some(new)) if old.is_same_as(new) => {}
            (None, None) => {}
            _ => cx.emit(Event::CommitMessageModelChanged),
        }
        self.commit_message_model = model;
    }

    /// 设置对话总结模型
    pub fn set_thread_summary_model(
        &mut self,
        model: Option<ConfiguredModel>,
        cx: &mut Context<Self>,
    ) {
        match (self.thread_summary_model.as_ref(), model.as_ref()) {
            (Some(old), Some(new)) if old.is_same_as(new) => {}
            (None, None) => {}
            _ => cx.emit(Event::ThreadSummaryModelChanged),
        }
        self.thread_summary_model = model;
    }

    // -------------------------------------------------------------------------
    // 获取各类模型（对外接口）
    // -------------------------------------------------------------------------

    /// 获取默认模型（手动设置 > 环境备用）
    pub fn default_model(&self) -> Option<ConfiguredModel> {
        #[cfg(debug_assertions)]
        if std::env::var("ZED_SIMULATE_NO_LLM_PROVIDER").is_ok() {
            return None;
        }

        self.default_model
            .clone()
            .or_else(|| self.available_fallback_model.clone())
    }

    /// 获取默认快速模型（用于轻量任务）
    pub fn default_fast_model(&self, cx: &App) -> Option<ConfiguredModel> {
        let configured = self.default_model()?;
        let fast_model = configured.provider.default_fast_model(cx)?;
        Some(ConfiguredModel {
            provider: configured.provider,
            model: fast_model,
        })
    }

    /// 获取行内助手模型（专用模型 > 默认模型）
    pub fn inline_assistant_model(&self) -> Option<ConfiguredModel> {
        #[cfg(debug_assertions)]
        if std::env::var("ZED_SIMULATE_NO_LLM_PROVIDER").is_ok() {
            return None;
        }

        self.inline_assistant_model
            .clone()
            .or_else(|| self.default_model.clone())
    }

    /// 获取提交信息模型（专用 > 快速 > 默认）
    pub fn commit_message_model(&self, cx: &App) -> Option<ConfiguredModel> {
        #[cfg(debug_assertions)]
        if std::env::var("ZED_SIMULATE_NO_LLM_PROVIDER").is_ok() {
            return None;
        }

        self.commit_message_model
            .clone()
            .or_else(|| self.default_fast_model(cx))
            .or_else(|| self.default_model())
    }

    /// 获取对话总结模型（专用 > 快速 > 默认）
    pub fn thread_summary_model(&self, cx: &App) -> Option<ConfiguredModel> {
        #[cfg(debug_assertions)]
        if std::env::var("ZED_SIMULATE_NO_LLM_PROVIDER").is_ok() {
            return None;
        }

        self.thread_summary_model
            .clone()
            .or_else(|| self.default_fast_model(cx))
            .or_else(|| self.default_model())
    }

    /// 获取行内备选模型列表
    pub fn inline_alternative_models(&self) -> &[Arc<dyn LanguageModel>] {
        &self.inline_alternatives
    }
}

// -----------------------------------------------------------------------------
// 测试用例（已汉化注释）
// -----------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_provider::FakeLanguageModelProvider;

    #[gpui::test]
    fn test_register_providers(cx: &mut App) {
        let registry = cx.new(|_| LanguageModelRegistry::default());

        let provider = Arc::new(FakeLanguageModelProvider::default());
        registry.update(cx, |registry, cx| {
            registry.register_provider(provider.clone(), cx);
        });

        let providers = registry.read(cx).providers();
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].id(), provider.id());

        registry.update(cx, |registry, cx| {
            registry.unregister_provider(provider.id(), cx);
        });

        let providers = registry.read(cx).providers();
        assert!(providers.is_empty());
    }

    #[gpui::test]
    fn test_provider_hiding_on_extension_install(cx: &mut App) {
        let registry = cx.new(|_| LanguageModelRegistry::default());

        let provider = Arc::new(FakeLanguageModelProvider::default());
        let provider_id = provider.id();

        registry.update(cx, |registry, cx| {
            registry.register_provider(provider.clone(), cx);

            registry.set_builtin_provider_hiding_fn(Box::new(|id| {
                if id == "fake" {
                    Some("fake-extension")
                } else {
                    None
                }
            }));
        });

        let visible = registry.read(cx).visible_providers();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id(), provider_id);

        registry.update(cx, |registry, cx| {
            registry.extension_installed("fake-extension".into(), cx);
        });

        let visible = registry.read(cx).visible_providers();
        assert!(visible.is_empty());

        let all = registry.read(cx).providers();
        assert_eq!(all.len(), 1);
    }

    #[gpui::test]
    fn test_provider_unhiding_on_extension_uninstall(cx: &mut App) {
        let registry = cx.new(|_| LanguageModelRegistry::default());

        let provider = Arc::new(FakeLanguageModelProvider::default());
        let provider_id = provider.id();

        registry.update(cx, |registry, cx| {
            registry.register_provider(provider.clone(), cx);

            registry.set_builtin_provider_hiding_fn(Box::new(|id| {
                if id == "fake" {
                    Some("fake-extension")
                } else {
                    None
                }
            }));

            registry.extension_installed("fake-extension".into(), cx);
        });

        let visible = registry.read(cx).visible_providers();
        assert!(visible.is_empty());

        registry.update(cx, |registry, cx| {
            registry.extension_uninstalled("fake-extension", cx);
        });

        let visible = registry.read(cx).visible_providers();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id(), provider_id);
    }

    #[gpui::test]
    fn test_should_hide_provider(cx: &mut App) {
        let registry = cx.new(|_| LanguageModelRegistry::default());

        registry.update(cx, |registry, cx| {
            registry.set_builtin_provider_hiding_fn(Box::new(|id| {
                if id == "anthropic" {
                    Some("anthropic")
                } else if id == "openai" {
                    Some("openai")
                } else {
                    None
                }
            }));

            registry.extension_installed("anthropic".into(), cx);
        });

        let registry_read = registry.read(cx);

        assert!(registry_read.should_hide_provider(&LanguageModelProviderId("anthropic".into())));
        assert!(!registry_read.should_hide_provider(&LanguageModelProviderId("openai".into())));
        assert!(!registry_read.should_hide_provider(&LanguageModelProviderId("unknown".into())));
    }

    #[gpui::test]
    async fn test_configure_environment_fallback_model(cx: &mut gpui::TestAppContext) {
        let registry = cx.new(|_| LanguageModelRegistry::default());

        let provider = Arc::new(FakeLanguageModelProvider::default());
        registry.update(cx, |registry, cx| {
            registry.register_provider(provider.clone(), cx);
        });

        cx.update(|cx| provider.authenticate(cx)).await.unwrap();

        registry.update(cx, |registry, cx| {
            let provider = registry.provider(&provider.id()).unwrap();
            let model = provider.default_model(cx).unwrap();

            registry.set_environment_fallback_model(
                Some(ConfiguredModel {
                    provider: provider.clone(),
                    model: model.clone(),
                }),
                cx,
            );

            let default_model = registry.default_model().unwrap();
            assert_eq!(default_model.model.id(), model.id());
            assert_eq!(default_model.provider.id(), provider.id());
        });
    }

    #[gpui::test]
    fn test_sync_installed_llm_extensions(cx: &mut App) {
        let registry = cx.new(|_| LanguageModelRegistry::default());

        let provider = Arc::new(FakeLanguageModelProvider::default());

        registry.update(cx, |registry, cx| {
            registry.register_provider(provider.clone(), cx);

            registry.set_builtin_provider_hiding_fn(Box::new(|id| {
                if id == "fake" {
                    Some("fake-extension")
                } else {
                    None
                }
            }));
        });

        let mut extension_ids = HashSet::default();
        extension_ids.insert(Arc::from("fake-extension"));

        registry.update(cx, |registry, cx| {
            registry.sync_installed_llm_extensions(extension_ids, cx);
        });

        assert!(registry.read(cx).visible_providers().is_empty());

        registry.update(cx, |registry, cx| {
            registry.sync_installed_llm_extensions(HashSet::default(), cx);
        });

        assert_eq!(registry.read(cx).visible_providers().len(), 1);
    }
}