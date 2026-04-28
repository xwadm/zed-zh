use std::pin::Pin;
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow};
use aws_config::stalled_stream_protection::StalledStreamProtectionConfig;
use aws_config::{BehaviorVersion, Region};
use aws_credential_types::{Credentials, Token};
use aws_http_client::AwsHttpClient;
use bedrock::bedrock_client::Client as BedrockClient;
use bedrock::bedrock_client::config::timeout::TimeoutConfig;
use bedrock::bedrock_client::types::{
    CachePointBlock, CachePointType, ContentBlockDelta, ContentBlockStart, ConverseStreamOutput,
    ReasoningContentBlockDelta, StopReason,
};
use bedrock::{
    BedrockAnyToolChoice, BedrockAutoToolChoice, BedrockBlob, BedrockError, BedrockImageBlock,
    BedrockImageFormat, BedrockImageSource, BedrockInnerContent, BedrockMessage, BedrockModelMode,
    BedrockStreamingResponse, BedrockThinkingBlock, BedrockThinkingTextBlock, BedrockTool,
    BedrockToolChoice, BedrockToolConfig, BedrockToolInputSchema, BedrockToolResultBlock,
    BedrockToolResultContentBlock, BedrockToolResultStatus, BedrockToolSpec, BedrockToolUseBlock,
    Model, value_to_aws_document,
};
use collections::{BTreeMap, HashMap};
use credentials_provider::CredentialsProvider;
use futures::{FutureExt, Stream, StreamExt, future::BoxFuture, stream::BoxStream};
use gpui::{
    AnyView, App, AsyncApp, Context, Entity, FocusHandle, Subscription, Task, Window, actions,
};
use gpui_tokio::Tokio;
use http_client::HttpClient;
use language_model::{
    AuthenticateError, EnvVar, IconOrSvg, LanguageModel, LanguageModelCacheConfiguration,
    LanguageModelCompletionError, LanguageModelCompletionEvent, LanguageModelId, LanguageModelName,
    LanguageModelProvider, LanguageModelProviderId, LanguageModelProviderName,
    LanguageModelProviderState, LanguageModelRequest, LanguageModelToolChoice,
    LanguageModelToolResultContent, LanguageModelToolUse, MessageContent, RateLimiter, Role,
    TokenUsage, env_var,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use settings::{BedrockAvailableModel as AvailableModel, Settings, SettingsStore};
use smol::lock::OnceCell;
use std::sync::LazyLock;
use strum::{EnumIter, IntoEnumIterator, IntoStaticStr};
use ui::{ButtonLink, ConfiguredApiCard, Divider, List, ListBulletItem, prelude::*};
use ui_input::InputField;
use util::ResultExt;

use crate::AllLanguageModelSettings;
use language_model::util::{fix_streamed_json, parse_tool_arguments};

actions!(bedrock, [Tab, TabPrev]);

const PROVIDER_ID: LanguageModelProviderId = LanguageModelProviderId::new("amazon-bedrock");
const PROVIDER_NAME: LanguageModelProviderName = LanguageModelProviderName::new("Amazon Bedrock");

/// 用于静态身份验证的存储在钥匙串中的凭据。
/// 区域单独处理，因为它与身份验证方法无关。
#[derive(Default, Clone, Deserialize, Serialize, PartialEq, Debug)]
pub struct BedrockCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
    pub bearer_token: Option<String>,
}

/// Bedrock 的身份验证配置解析结果。
/// 设置中的优先级高于用户界面提供的凭据。
#[derive(Clone, Debug, PartialEq)]
pub enum BedrockAuth {
    /// 使用默认的 AWS 凭据提供程序链（IMDSv2、PodIdentity、环境变量等）
    Automatic,
    /// 使用来自 ~/.aws/credentials 或 ~/.aws/config 的 AWS 命名配置文件
    NamedProfile { profile_name: String },
    /// 使用 AWS SSO 配置文件
    SingleSignOn { profile_name: String },
    /// 使用 IAM 凭据（访问密钥 + 秘密访问密钥 + 可选的会话令牌）
    IamCredentials {
        access_key_id: String,
        secret_access_key: String,
        session_token: Option<String>,
    },
    /// 使用 Bedrock API 密钥（Bearer 令牌身份验证）
    ApiKey { api_key: String },
}

impl BedrockCredentials {
    /// 将存储的凭据转换为相应的身份验证变体。
    /// 如果存在 API 密钥，则优先使用 API 密钥，否则使用 IAM 凭据。
    fn into_auth(self) -> Option<BedrockAuth> {
        if let Some(api_key) = self.bearer_token.filter(|t| !t.is_empty()) {
            Some(BedrockAuth::ApiKey { api_key })
        } else if !self.access_key_id.is_empty() && !self.secret_access_key.is_empty() {
            Some(BedrockAuth::IamCredentials {
                access_key_id: self.access_key_id,
                secret_access_key: self.secret_access_key,
                session_token: self.session_token.filter(|t| !t.is_empty()),
            })
        } else {
            None
        }
    }
}

#[derive(Default, Clone, Debug, PartialEq)]
pub struct AmazonBedrockSettings {
    pub available_models: Vec<AvailableModel>,
    pub region: Option<String>,
    pub endpoint: Option<String>,
    pub profile_name: Option<String>,
    pub role_arn: Option<String>,
    pub authentication_method: Option<BedrockAuthMethod>,
    pub allow_global: Option<bool>,
    pub allow_extended_context: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, EnumIter, IntoStaticStr, JsonSchema)]
pub enum BedrockAuthMethod {
    #[serde(rename = "named_profile")]
    NamedProfile,
    #[serde(rename = "sso")]
    SingleSignOn,
    #[serde(rename = "api_key")]
    ApiKey,
    /// IMDSv2、PodIdentity、环境变量等。
    #[serde(rename = "default")]
    Automatic,
}

impl From<settings::BedrockAuthMethodContent> for BedrockAuthMethod {
    fn from(value: settings::BedrockAuthMethodContent) -> Self {
        match value {
            settings::BedrockAuthMethodContent::SingleSignOn => BedrockAuthMethod::SingleSignOn,
            settings::BedrockAuthMethodContent::Automatic => BedrockAuthMethod::Automatic,
            settings::BedrockAuthMethodContent::NamedProfile => BedrockAuthMethod::NamedProfile,
            settings::BedrockAuthMethodContent::ApiKey => BedrockAuthMethod::ApiKey,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ModelMode {
    #[default]
    Default,
    Thinking {
        /// 用于推理的最大令牌数。必须低于模型的 `max_output_tokens`。
        budget_tokens: Option<u64>,
    },
    AdaptiveThinking {
        effort: bedrock::BedrockAdaptiveThinkingEffort,
    },
}

impl From<ModelMode> for BedrockModelMode {
    fn from(value: ModelMode) -> Self {
        match value {
            ModelMode::Default => BedrockModelMode::Default,
            ModelMode::Thinking { budget_tokens } => BedrockModelMode::Thinking { budget_tokens },
            ModelMode::AdaptiveThinking { effort } => BedrockModelMode::AdaptiveThinking { effort },
        }
    }
}

impl From<BedrockModelMode> for ModelMode {
    fn from(value: BedrockModelMode) -> Self {
        match value {
            BedrockModelMode::Default => ModelMode::Default,
            BedrockModelMode::Thinking { budget_tokens } => ModelMode::Thinking { budget_tokens },
            BedrockModelMode::AdaptiveThinking { effort } => ModelMode::AdaptiveThinking { effort },
        }
    }
}

/// 基础 AWS 服务的 URL。
///
/// 现在我们只是将其用作在钥匙串中存储 AWS 凭据的键。
const AMAZON_AWS_URL: &str = "https://amazonaws.com";

// 这些环境变量都使用 `ZED_` 前缀，因为我们不想覆盖用户自己的 AWS 凭据。
static ZED_BEDROCK_ACCESS_KEY_ID_VAR: LazyLock<EnvVar> = env_var!("ZED_ACCESS_KEY_ID");
static ZED_BEDROCK_SECRET_ACCESS_KEY_VAR: LazyLock<EnvVar> = env_var!("ZED_SECRET_ACCESS_KEY");
static ZED_BEDROCK_SESSION_TOKEN_VAR: LazyLock<EnvVar> = env_var!("ZED_SESSION_TOKEN");
static ZED_AWS_PROFILE_VAR: LazyLock<EnvVar> = env_var!("ZED_AWS_PROFILE");
static ZED_BEDROCK_REGION_VAR: LazyLock<EnvVar> = env_var!("ZED_AWS_REGION");
static ZED_AWS_ENDPOINT_VAR: LazyLock<EnvVar> = env_var!("ZED_AWS_ENDPOINT");
static ZED_BEDROCK_BEARER_TOKEN_VAR: LazyLock<EnvVar> = env_var!("ZED_BEDROCK_BEARER_TOKEN");

pub struct State {
    /// 已解析的身份验证方法。设置优先于用户界面凭据。
    auth: Option<BedrockAuth>,
    /// settings.json 中的原始设置
    settings: Option<AmazonBedrockSettings>,
    /// 凭据是否来自环境变量（仅与静态凭据相关）
    credentials_from_env: bool,
    credentials_provider: Arc<dyn CredentialsProvider>,
    _subscription: Subscription,
}

impl State {
    fn reset_auth(&self, cx: &mut Context<Self>) -> Task<Result<()>> {
        let credentials_provider = self.credentials_provider.clone();
        cx.spawn(async move |this, cx| {
            credentials_provider
                .delete_credentials(AMAZON_AWS_URL, cx)
                .await
                .log_err();
            this.update(cx, |this, cx| {
                this.auth = None;
                this.credentials_from_env = false;
                cx.notify();
            })
        })
    }

    fn set_static_credentials(
        &mut self,
        credentials: BedrockCredentials,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        let auth = credentials.clone().into_auth();
        let credentials_provider = self.credentials_provider.clone();
        cx.spawn(async move |this, cx| {
            credentials_provider
                .write_credentials(
                    AMAZON_AWS_URL,
                    "Bearer",
                    &serde_json::to_vec(&credentials)?,
                    cx,
                )
                .await?;
            this.update(cx, |this, cx| {
                this.auth = auth;
                this.credentials_from_env = false;
                cx.notify();
            })
        })
    }

    fn is_authenticated(&self) -> bool {
        self.auth.is_some()
    }

    /// 解析身份验证。设置优先于用户界面提供的凭据。
    fn authenticate(&self, cx: &mut Context<Self>) -> Task<Result<(), AuthenticateError>> {
        if self.is_authenticated() {
            return Task::ready(Ok(()));
        }

        // 步骤 1：检查设置是否指定了身份验证方法（企业控制）
        if let Some(settings) = &self.settings {
            if let Some(method) = &settings.authentication_method {
                let profile_name = settings
                    .profile_name
                    .clone()
                    .unwrap_or_else(|| "default".to_string());

                let auth = match method {
                    BedrockAuthMethod::Automatic => BedrockAuth::Automatic,
                    BedrockAuthMethod::NamedProfile => BedrockAuth::NamedProfile { profile_name },
                    BedrockAuthMethod::SingleSignOn => BedrockAuth::SingleSignOn { profile_name },
                    BedrockAuthMethod::ApiKey => {
                        // ApiKey 方法意味着“使用来自钥匙串/环境变量的静态凭据”
                        // 继续向下加载它们
                        return self.load_static_credentials(cx);
                    }
                };

                return cx.spawn(async move |this, cx| {
                    this.update(cx, |this, cx| {
                        this.auth = Some(auth);
                        this.credentials_from_env = false;
                        cx.notify();
                    })?;
                    Ok(())
                });
            }
        }

        // 步骤 2：没有设置身份验证方法——尝试加载静态凭据
        self.load_static_credentials(cx)
    }

    /// 从环境变量或钥匙串加载静态凭据。
    fn load_static_credentials(
        &self,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), AuthenticateError>> {
        let credentials_provider = self.credentials_provider.clone();
        cx.spawn(async move |this, cx| {
            // 首先尝试环境变量
            let (auth, from_env) = if let Some(bearer_token) = &ZED_BEDROCK_BEARER_TOKEN_VAR.value {
                if !bearer_token.is_empty() {
                    (
                        Some(BedrockAuth::ApiKey {
                            api_key: bearer_token.to_string(),
                        }),
                        true,
                    )
                } else {
                    (None, false)
                }
            } else if let Some(access_key_id) = &ZED_BEDROCK_ACCESS_KEY_ID_VAR.value {
                if let Some(secret_access_key) = &ZED_BEDROCK_SECRET_ACCESS_KEY_VAR.value {
                    if !access_key_id.is_empty() && !secret_access_key.is_empty() {
                        let session_token = ZED_BEDROCK_SESSION_TOKEN_VAR
                            .value
                            .as_deref()
                            .filter(|s| !s.is_empty())
                            .map(|s| s.to_string());
                        (
                            Some(BedrockAuth::IamCredentials {
                                access_key_id: access_key_id.to_string(),
                                secret_access_key: secret_access_key.to_string(),
                                session_token,
                            }),
                            true,
                        )
                    } else {
                        (None, false)
                    }
                } else {
                    (None, false)
                }
            } else {
                (None, false)
            };

            // 如果我们从环境变量中获得了身份验证，就使用它
            if let Some(auth) = auth {
                this.update(cx, |this, cx| {
                    this.auth = Some(auth);
                    this.credentials_from_env = from_env;
                    cx.notify();
                })?;
                return Ok(());
            }

            // 尝试钥匙串
            let (_, credentials_bytes) = credentials_provider
                .read_credentials(AMAZON_AWS_URL, cx)
                .await?
                .ok_or(AuthenticateError::CredentialsNotFound)?;

            let credentials_str = String::from_utf8(credentials_bytes)
                .with_context(|| format!("无效的 {PROVIDER_NAME} 凭据"))?;

            let credentials: BedrockCredentials =
                serde_json::from_str(&credentials_str).context("解析凭据失败")?;

            let auth = credentials
                .into_auth()
                .ok_or(AuthenticateError::CredentialsNotFound)?;

            this.update(cx, |this, cx| {
                this.auth = Some(auth);
                this.credentials_from_env = false;
                cx.notify();
            })?;

            Ok(())
        })
    }

    /// 获取解析后的区域。检查环境变量，然后设置，最后默认为 us-east-1。
    fn get_region(&self) -> String {
        // 优先级：环境变量 > 设置 > 默认
        if let Some(region) = ZED_BEDROCK_REGION_VAR.value.as_deref() {
            if !region.is_empty() {
                return region.to_string();
            }
        }

        self.settings
            .as_ref()
            .and_then(|s| s.region.clone())
            .unwrap_or_else(|| "us-east-1".to_string())
    }

    fn get_allow_global(&self) -> bool {
        self.settings
            .as_ref()
            .and_then(|s| s.allow_global)
            .unwrap_or(false)
    }

    fn get_allow_extended_context(&self) -> bool {
        self.settings
            .as_ref()
            .and_then(|s| s.allow_extended_context)
            .unwrap_or(false)
    }
}

pub struct BedrockLanguageModelProvider {
    http_client: AwsHttpClient,
    handle: tokio::runtime::Handle,
    state: Entity<State>,
}

impl BedrockLanguageModelProvider {
    pub fn new(
        http_client: Arc<dyn HttpClient>,
        credentials_provider: Arc<dyn CredentialsProvider>,
        cx: &mut App,
    ) -> Self {
        let state = cx.new(|cx| State {
            auth: None,
            settings: Some(AllLanguageModelSettings::get_global(cx).bedrock.clone()),
            credentials_from_env: false,
            credentials_provider,
            _subscription: cx.observe_global::<SettingsStore>(|_, cx| {
                cx.notify();
            }),
        });

        Self {
            http_client: AwsHttpClient::new(http_client),
            handle: Tokio::handle(cx),
            state,
        }
    }

    fn create_language_model(&self, model: bedrock::Model) -> Arc<dyn LanguageModel> {
        Arc::new(BedrockModel {
            id: LanguageModelId::from(model.id().to_string()),
            model,
            http_client: self.http_client.clone(),
            handle: self.handle.clone(),
            state: self.state.clone(),
            client: OnceCell::new(),
            request_limiter: RateLimiter::new(4),
        })
    }
}

impl LanguageModelProvider for BedrockLanguageModelProvider {
    fn id(&self) -> LanguageModelProviderId {
        PROVIDER_ID
    }

    fn name(&self) -> LanguageModelProviderName {
        PROVIDER_NAME
    }

    fn icon(&self) -> IconOrSvg {
        IconOrSvg::Icon(IconName::AiBedrock)
    }

    fn default_model(&self, _cx: &App) -> Option<Arc<dyn LanguageModel>> {
        Some(self.create_language_model(bedrock::Model::default()))
    }

    fn default_fast_model(&self, cx: &App) -> Option<Arc<dyn LanguageModel>> {
        let region = self.state.read(cx).get_region();
        Some(self.create_language_model(bedrock::Model::default_fast(region.as_str())))
    }

    fn provided_models(&self, cx: &App) -> Vec<Arc<dyn LanguageModel>> {
        let mut models = BTreeMap::default();

        for model in bedrock::Model::iter() {
            if !matches!(model, bedrock::Model::Custom { .. }) {
                models.insert(model.id().to_string(), model);
            }
        }

        // 使用设置中的可用模型进行覆盖
        for model in AllLanguageModelSettings::get_global(cx)
            .bedrock
            .available_models
            .iter()
        {
            models.insert(
                model.name.clone(),
                bedrock::Model::Custom {
                    name: model.name.clone(),
                    display_name: model.display_name.clone(),
                    max_tokens: model.max_tokens,
                    max_output_tokens: model.max_output_tokens,
                    default_temperature: model.default_temperature,
                    cache_configuration: model.cache_configuration.as_ref().map(|config| {
                        bedrock::BedrockModelCacheConfiguration {
                            max_cache_anchors: config.max_cache_anchors,
                            min_total_token: config.min_total_token,
                        }
                    }),
                },
            );
        }

        models
            .into_values()
            .map(|model| self.create_language_model(model))
            .collect()
    }

    fn is_authenticated(&self, cx: &App) -> bool {
        self.state.read(cx).is_authenticated()
    }

    fn authenticate(&self, cx: &mut App) -> Task<Result<(), AuthenticateError>> {
        self.state.update(cx, |state, cx| state.authenticate(cx))
    }

    fn configuration_view(
        &self,
        _target_agent: language_model::ConfigurationViewTargetAgent,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyView {
        cx.new(|cx| ConfigurationView::new(self.state.clone(), window, cx))
            .into()
    }

    fn reset_credentials(&self, cx: &mut App) -> Task<Result<()>> {
        self.state.update(cx, |state, cx| state.reset_auth(cx))
    }
}

impl LanguageModelProviderState for BedrockLanguageModelProvider {
    type ObservableEntity = State;

    fn observable_entity(&self) -> Option<Entity<Self::ObservableEntity>> {
        Some(self.state.clone())
    }
}

struct BedrockModel {
    id: LanguageModelId,
    model: Model,
    http_client: AwsHttpClient,
    handle: tokio::runtime::Handle,
    client: OnceCell<BedrockClient>,
    state: Entity<State>,
    request_limiter: RateLimiter,
}

impl BedrockModel {
    fn get_or_init_client(&self, cx: &AsyncApp) -> anyhow::Result<&BedrockClient> {
        self.client
            .get_or_try_init_blocking(|| {
                let (auth, endpoint, region) = cx.read_entity(&self.state, |state, _cx| {
                    let endpoint = state.settings.as_ref().and_then(|s| s.endpoint.clone());
                    let region = state.get_region();
                    (state.auth.clone(), endpoint, region)
                });

                let mut config_builder = aws_config::defaults(BehaviorVersion::latest())
                    .stalled_stream_protection(StalledStreamProtectionConfig::disabled())
                    .http_client(self.http_client.clone())
                    .region(Region::new(region))
                    .timeout_config(TimeoutConfig::disabled());

                if let Some(endpoint_url) = endpoint
                    && !endpoint_url.is_empty()
                {
                    config_builder = config_builder.endpoint_url(endpoint_url);
                }

                match auth {
                    Some(BedrockAuth::Automatic) | None => {
                        // 使用默认的 AWS 凭据提供程序链
                    }
                    Some(BedrockAuth::NamedProfile { profile_name })
                    | Some(BedrockAuth::SingleSignOn { profile_name }) => {
                        if !profile_name.is_empty() {
                            config_builder = config_builder.profile_name(profile_name);
                        }
                    }
                    Some(BedrockAuth::IamCredentials {
                        access_key_id,
                        secret_access_key,
                        session_token,
                    }) => {
                        let aws_creds = Credentials::new(
                            access_key_id,
                            secret_access_key,
                            session_token,
                            None,
                            "zed-bedrock-provider",
                        );
                        config_builder = config_builder.credentials_provider(aws_creds);
                    }
                    Some(BedrockAuth::ApiKey { api_key }) => {
                        config_builder = config_builder
                            .auth_scheme_preference(["httpBearerAuth".into()]) // https://github.com/smithy-lang/smithy-rs/pull/4241
                            .token_provider(Token::new(api_key, None));
                    }
                }

                let config = self.handle.block_on(config_builder.load());

                anyhow::Ok(BedrockClient::new(&config))
            })
            .context("初始化 Bedrock 客户端")?;

        self.client.get().context("Bedrock 客户端未初始化")
    }

    fn stream_completion(
        &self,
        request: bedrock::Request,
        cx: &AsyncApp,
    ) -> BoxFuture<
        'static,
        Result<BoxStream<'static, Result<BedrockStreamingResponse, anyhow::Error>>, BedrockError>,
    > {
        let Ok(runtime_client) = self
            .get_or_init_client(cx)
            .cloned()
            .context("Bedrock 客户端未初始化")
        else {
            return futures::future::ready(Err(BedrockError::Other(anyhow!("应用状态已丢失"))))
                .boxed();
        };

        let task = Tokio::spawn(cx, bedrock::stream_completion(runtime_client, request));
        async move { task.await.map_err(|e| BedrockError::Other(e.into()))? }.boxed()
    }
}

impl LanguageModel for BedrockModel {
    fn id(&self) -> LanguageModelId {
        self.id.clone()
    }

    fn name(&self) -> LanguageModelName {
        LanguageModelName::from(self.model.display_name().to_string())
    }

    fn provider_id(&self) -> LanguageModelProviderId {
        PROVIDER_ID
    }

    fn provider_name(&self) -> LanguageModelProviderName {
        PROVIDER_NAME
    }

    fn supports_tools(&self) -> bool {
        self.model.supports_tool_use()
    }

    fn supports_images(&self) -> bool {
        self.model.supports_images()
    }

    fn supports_thinking(&self) -> bool {
        self.model.supports_thinking()
    }

    fn supported_effort_levels(&self) -> Vec<language_model::LanguageModelEffortLevel> {
        if self.model.supports_adaptive_thinking() {
            vec![
                language_model::LanguageModelEffortLevel {
                    name: "Low".into(),
                    value: "low".into(),
                    is_default: false,
                },
                language_model::LanguageModelEffortLevel {
                    name: "Medium".into(),
                    value: "medium".into(),
                    is_default: false,
                },
                language_model::LanguageModelEffortLevel {
                    name: "High".into(),
                    value: "high".into(),
                    is_default: true,
                },
                language_model::LanguageModelEffortLevel {
                    name: "Max".into(),
                    value: "max".into(),
                    is_default: false,
                },
            ]
        } else {
            Vec::new()
        }
    }

    fn supports_tool_choice(&self, choice: LanguageModelToolChoice) -> bool {
        match choice {
            LanguageModelToolChoice::Auto | LanguageModelToolChoice::Any => {
                self.model.supports_tool_use()
            }
            // 添加对 None 的支持 - 我们将在响应中过滤工具调用
            LanguageModelToolChoice::None => self.model.supports_tool_use(),
        }
    }

    fn supports_streaming_tools(&self) -> bool {
        true
    }

    fn telemetry_id(&self) -> String {
        format!("bedrock/{}", self.model.id())
    }

    fn max_token_count(&self) -> u64 {
        self.model.max_token_count()
    }

    fn max_output_tokens(&self) -> Option<u64> {
        Some(self.model.max_output_tokens())
    }

    fn stream_completion(
        &self,
        request: LanguageModelRequest,
        cx: &AsyncApp,
    ) -> BoxFuture<
        'static,
        Result<
            BoxStream<'static, Result<LanguageModelCompletionEvent, LanguageModelCompletionError>>,
            LanguageModelCompletionError,
        >,
    > {
        let (region, allow_global, allow_extended_context) =
            cx.read_entity(&self.state, |state, _cx| {
                (
                    state.get_region(),
                    state.get_allow_global(),
                    state.get_allow_extended_context(),
                )
            });

        let model_id = match self.model.cross_region_inference_id(&region, allow_global) {
            Ok(s) => s,
            Err(e) => {
                return async move { Err(e.into()) }.boxed();
            }
        };

        let deny_tool_calls = request.tool_choice == Some(LanguageModelToolChoice::None);

        let use_extended_context = allow_extended_context && self.model.supports_extended_context();

        let request = match into_bedrock(
            request,
            model_id,
            self.model.default_temperature(),
            self.model.max_output_tokens(),
            self.model.thinking_mode(),
            self.model.supports_caching(),
            self.model.supports_tool_use(),
            use_extended_context,
        ) {
            Ok(request) => request,
            Err(err) => return futures::future::ready(Err(err.into())).boxed(),
        };

        let request = self.stream_completion(request, cx);
        let display_name = self.model.display_name().to_string();
        let future = self.request_limiter.stream(async move {
            let response = request.await.map_err(|err| match err {
                BedrockError::Validation(ref msg) => {
                    if msg.contains("model identifier is invalid") {
                        LanguageModelCompletionError::Other(anyhow!(
                            "{display_name} 在 {region} 中不可用。\
                                 尝试切换到支持此模型的区域。"
                        ))
                    } else {
                        LanguageModelCompletionError::BadRequestFormat {
                            provider: PROVIDER_NAME,
                            message: msg.clone(),
                        }
                    }
                }
                BedrockError::RateLimited => LanguageModelCompletionError::RateLimitExceeded {
                    provider: PROVIDER_NAME,
                    retry_after: None,
                },
                BedrockError::ServiceUnavailable => {
                    LanguageModelCompletionError::ServerOverloaded {
                        provider: PROVIDER_NAME,
                        retry_after: None,
                    }
                }
                BedrockError::AccessDenied(msg) => LanguageModelCompletionError::PermissionError {
                    provider: PROVIDER_NAME,
                    message: msg,
                },
                BedrockError::InternalServer(msg) => {
                    LanguageModelCompletionError::ApiInternalServerError {
                        provider: PROVIDER_NAME,
                        message: msg,
                    }
                }
                other => LanguageModelCompletionError::Other(anyhow!(other)),
            })?;
            let events = map_to_language_model_completion_events(response);

            if deny_tool_calls {
                Ok(deny_tool_use_events(events).boxed())
            } else {
                Ok(events.boxed())
            }
        });

        async move { Ok(future.await?.boxed()) }.boxed()
    }

    fn cache_configuration(&self) -> Option<LanguageModelCacheConfiguration> {
        self.model
            .cache_configuration()
            .map(|config| LanguageModelCacheConfiguration {
                max_cache_anchors: config.max_cache_anchors,
                should_speculate: false,
                min_total_token: config.min_total_token,
            })
    }
}

fn deny_tool_use_events(
    events: impl Stream<Item = Result<LanguageModelCompletionEvent, LanguageModelCompletionError>>,
) -> impl Stream<Item = Result<LanguageModelCompletionEvent, LanguageModelCompletionError>> {
    events.map(|event| {
        match event {
            Ok(LanguageModelCompletionEvent::ToolUse(tool_use)) => {
                // 如果模型决定调用工具，则将工具使用转换为错误消息
                Ok(LanguageModelCompletionEvent::Text(format!(
                    "\n\n[错误：此上下文中已禁用工具调用。尝试调用 '{}']",
                    tool_use.name
                )))
            }
            other => other,
        }
    })
}

pub fn into_bedrock(
    request: LanguageModelRequest,
    model: String,
    default_temperature: f32,
    max_output_tokens: u64,
    thinking_mode: BedrockModelMode,
    supports_caching: bool,
    supports_tool_use: bool,
    allow_extended_context: bool,
) -> Result<bedrock::Request> {
    let mut new_messages: Vec<BedrockMessage> = Vec::new();
    let mut system_message = String::new();

    // 跟踪消息是否包含工具内容 - Bedrock 在存在工具块时要求 toolConfig，
    // 因此我们可能需要添加一个虚拟工具
    let mut messages_contain_tool_content = false;

    for message in request.messages {
        if message.contents_empty() {
            continue;
        }

        match message.role {
            Role::User | Role::Assistant => {
                let mut bedrock_message_content: Vec<BedrockInnerContent> = message
                    .content
                    .into_iter()
                    .filter_map(|content| match content {
                        MessageContent::Text(text) => {
                            if !text.is_empty() {
                                Some(BedrockInnerContent::Text(text))
                            } else {
                                None
                            }
                        }
                        MessageContent::Thinking { text, signature } => {
                            if model.contains(Model::DeepSeekR1.request_id()) {
                                // DeepSeekR1 不支持思考块
                                // 并且 AWS API 要求你将其剥离
                                return None;
                            }
                            if signature.is_none() {
                                // 没有签名的思考块是无效的
                                // （例如由于中途取消思考），必须
                                // 剥离以避免 API 错误。
                                return None;
                            }
                            let thinking = BedrockThinkingTextBlock::builder()
                                .text(text)
                                .set_signature(signature)
                                .build()
                                .context("无法构建推理块")
                                .log_err()?;

                            Some(BedrockInnerContent::ReasoningContent(
                                BedrockThinkingBlock::ReasoningText(thinking),
                            ))
                        }
                        MessageContent::RedactedThinking(blob) => {
                            if model.contains(Model::DeepSeekR1.request_id()) {
                                // DeepSeekR1 不支持思考块
                                // 并且 AWS API 要求你将其剥离
                                return None;
                            }
                            let redacted =
                                BedrockThinkingBlock::RedactedContent(BedrockBlob::new(blob));

                            Some(BedrockInnerContent::ReasoningContent(redacted))
                        }
                        MessageContent::ToolUse(tool_use) => {
                            messages_contain_tool_content = true;
                            let input = if tool_use.input.is_null() {
                                // Bedrock API 要求工具使用输入为有效的 JsonValue，不能为 null
                                value_to_aws_document(&serde_json::json!({}))
                            } else {
                                value_to_aws_document(&tool_use.input)
                            };
                            BedrockToolUseBlock::builder()
                                .name(tool_use.name.to_string())
                                .tool_use_id(tool_use.id.to_string())
                                .input(input)
                                .build()
                                .context("无法构建 Bedrock 工具使用块")
                                .log_err()
                                .map(BedrockInnerContent::ToolUse)
                        }
                        MessageContent::ToolResult(tool_result) => {
                            messages_contain_tool_content = true;
                            BedrockToolResultBlock::builder()
                                .tool_use_id(tool_result.tool_use_id.to_string())
                                .content(match tool_result.content {
                                    LanguageModelToolResultContent::Text(text) => {
                                        BedrockToolResultContentBlock::Text(text.to_string())
                                    }
                                    LanguageModelToolResultContent::Image(image) => {
                                        use base64::Engine;

                                        match base64::engine::general_purpose::STANDARD
                                            .decode(image.source.as_bytes())
                                        {
                                            Ok(image_bytes) => {
                                                match BedrockImageBlock::builder()
                                                    .format(BedrockImageFormat::Png)
                                                    .source(BedrockImageSource::Bytes(
                                                        BedrockBlob::new(image_bytes),
                                                    ))
                                                    .build()
                                                {
                                                    Ok(image_block) => {
                                                        BedrockToolResultContentBlock::Image(
                                                            image_block,
                                                        )
                                                    }
                                                    Err(err) => {
                                                        BedrockToolResultContentBlock::Text(
                                                            format!(
                                                                "[无法构建图像块：{}]",
                                                                err
                                                            ),
                                                        )
                                                    }
                                                }
                                            }
                                            Err(err) => {
                                                BedrockToolResultContentBlock::Text(format!(
                                                    "[无法解码工具结果图像：{}]",
                                                    err
                                                ))
                                            }
                                        }
                                    }
                                })
                                .status({
                                    if tool_result.is_error {
                                        BedrockToolResultStatus::Error
                                    } else {
                                        BedrockToolResultStatus::Success
                                    }
                                })
                                .build()
                                .context("无法构建 Bedrock 工具结果块")
                                .log_err()
                                .map(BedrockInnerContent::ToolResult)
                        }
                        MessageContent::Image(image) => {
                            use base64::Engine;

                            let image_bytes = base64::engine::general_purpose::STANDARD
                                .decode(image.source.as_bytes())
                                .context("无法解码 base64 图像数据")
                                .log_err()?;

                            BedrockImageBlock::builder()
                                .format(BedrockImageFormat::Png)
                                .source(BedrockImageSource::Bytes(BedrockBlob::new(image_bytes)))
                                .build()
                                .context("无法构建 Bedrock 图像块")
                                .log_err()
                                .map(BedrockInnerContent::Image)
                        }
                    })
                    .collect();
                if message.cache && supports_caching {
                    bedrock_message_content.push(BedrockInnerContent::CachePoint(
                        CachePointBlock::builder()
                            .r#type(CachePointType::Default)
                            .build()
                            .context("无法构建缓存点块")?,
                    ));
                }
                let bedrock_role = match message.role {
                    Role::User => bedrock::BedrockRole::User,
                    Role::Assistant => bedrock::BedrockRole::Assistant,
                    Role::System => unreachable!("系统角色永远不应该在此处出现"),
                };
                if bedrock_message_content.is_empty() {
                    continue;
                }

                if let Some(last_message) = new_messages.last_mut()
                    && last_message.role == bedrock_role
                {
                    last_message.content.extend(bedrock_message_content);
                    continue;
                }
                new_messages.push(
                    BedrockMessage::builder()
                        .role(bedrock_role)
                        .set_content(Some(bedrock_message_content))
                        .build()
                        .context("无法构建 Bedrock 消息")?,
                );
            }
            Role::System => {
                if !system_message.is_empty() {
                    system_message.push_str("\n\n");
                }
                system_message.push_str(&message.string_contents());
            }
        }
    }

    let mut tool_spec: Vec<BedrockTool> = if supports_tool_use {
        request
            .tools
            .iter()
            .filter_map(|tool| {
                Some(BedrockTool::ToolSpec(
                    BedrockToolSpec::builder()
                        .name(tool.name.clone())
                        .description(tool.description.clone())
                        .input_schema(BedrockToolInputSchema::Json(value_to_aws_document(
                            &tool.input_schema,
                        )))
                        .build()
                        .log_err()?,
                ))
            })
            .collect()
    } else {
        Vec::new()
    };

    // Bedrock 在消息包含工具使用/结果块时需要 toolConfig。
    // 如果没有定义任何工具但消息包含工具内容（例如，在
    // 总结使用过工具的对话时），添加一个虚拟工具以满足
    // API 要求。
    if supports_tool_use && tool_spec.is_empty() && messages_contain_tool_content {
        tool_spec.push(BedrockTool::ToolSpec(
            BedrockToolSpec::builder()
                .name("_placeholder")
                .description("当对话历史包含工具使用时，用于满足 Bedrock API 要求的占位工具")
                .input_schema(BedrockToolInputSchema::Json(value_to_aws_document(
                    &serde_json::json!({"type": "object", "properties": {}}),
                )))
                .build()
                .context("无法构建占位工具规范")?,
        ));
    }

    if !tool_spec.is_empty() && supports_caching {
        tool_spec.push(BedrockTool::CachePoint(
            CachePointBlock::builder()
                .r#type(CachePointType::Default)
                .build()
                .context("无法构建缓存点块")?,
        ));
    }

    let tool_choice = match request.tool_choice {
        Some(LanguageModelToolChoice::Auto) | None => {
            BedrockToolChoice::Auto(BedrockAutoToolChoice::builder().build())
        }
        Some(LanguageModelToolChoice::Any) => {
            BedrockToolChoice::Any(BedrockAnyToolChoice::builder().build())
        }
        Some(LanguageModelToolChoice::None) => {
            // 对于 None，我们仍然使用 Auto，但会在响应中过滤掉工具调用
            BedrockToolChoice::Auto(BedrockAutoToolChoice::builder().build())
        }
    };
    let tool_config = if tool_spec.is_empty() {
        None
    } else {
        Some(
            BedrockToolConfig::builder()
                .set_tools(Some(tool_spec))
                .tool_choice(tool_choice)
                .build()?,
        )
    };

    Ok(bedrock::Request {
        model,
        messages: new_messages,
        max_tokens: max_output_tokens,
        system: Some(system_message),
        tools: tool_config,
        thinking: if request.thinking_allowed {
            match thinking_mode {
                BedrockModelMode::Thinking { budget_tokens } => {
                    Some(bedrock::Thinking::Enabled { budget_tokens })
                }
                BedrockModelMode::AdaptiveThinking {
                    effort: default_effort,
                } => {
                    let effort = request
                        .thinking_effort
                        .as_deref()
                        .and_then(|e| match e {
                            "low" => Some(bedrock::BedrockAdaptiveThinkingEffort::Low),
                            "medium" => Some(bedrock::BedrockAdaptiveThinkingEffort::Medium),
                            "high" => Some(bedrock::BedrockAdaptiveThinkingEffort::High),
                            "max" => Some(bedrock::BedrockAdaptiveThinkingEffort::Max),
                            _ => None,
                        })
                        .unwrap_or(default_effort);
                    Some(bedrock::Thinking::Adaptive { effort })
                }
                BedrockModelMode::Default => None,
            }
        } else {
            None
        },
        metadata: None,
        stop_sequences: Vec::new(),
        temperature: request.temperature.or(Some(default_temperature)),
        top_k: None,
        top_p: None,
        allow_extended_context,
    })
}

pub fn map_to_language_model_completion_events(
    events: Pin<Box<dyn Send + Stream<Item = Result<BedrockStreamingResponse, anyhow::Error>>>>,
) -> impl Stream<Item = Result<LanguageModelCompletionEvent, LanguageModelCompletionError>> {
    struct RawToolUse {
        id: String,
        name: String,
        input_json: String,
    }

    struct State {
        events: Pin<Box<dyn Send + Stream<Item = Result<BedrockStreamingResponse, anyhow::Error>>>>,
        tool_uses_by_index: HashMap<i32, RawToolUse>,
        emitted_tool_use: bool,
    }

    let initial_state = State {
        events,
        tool_uses_by_index: HashMap::default(),
        emitted_tool_use: false,
    };

    futures::stream::unfold(initial_state, |mut state| async move {
        match state.events.next().await {
            Some(event_result) => match event_result {
                Ok(event) => {
                    let result = match event {
                        ConverseStreamOutput::ContentBlockDelta(cb_delta) => match cb_delta.delta {
                            Some(ContentBlockDelta::Text(text)) => {
                                Some(Ok(LanguageModelCompletionEvent::Text(text)))
                            }
                            Some(ContentBlockDelta::ToolUse(tool_output)) => {
                                if let Some(tool_use) = state
                                    .tool_uses_by_index
                                    .get_mut(&cb_delta.content_block_index)
                                {
                                    tool_use.input_json.push_str(tool_output.input());
                                    if let Ok(input) = serde_json::from_str::<serde_json::Value>(
                                        &fix_streamed_json(&tool_use.input_json),
                                    ) {
                                        Some(Ok(LanguageModelCompletionEvent::ToolUse(
                                            LanguageModelToolUse {
                                                id: tool_use.id.clone().into(),
                                                name: tool_use.name.clone().into(),
                                                is_input_complete: false,
                                                raw_input: tool_use.input_json.clone(),
                                                input,
                                                thought_signature: None,
                                            },
                                        )))
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            }
                            Some(ContentBlockDelta::ReasoningContent(thinking)) => match thinking {
                                ReasoningContentBlockDelta::Text(thoughts) => {
                                    Some(Ok(LanguageModelCompletionEvent::Thinking {
                                        text: thoughts,
                                        signature: None,
                                    }))
                                }
                                ReasoningContentBlockDelta::Signature(sig) => {
                                    Some(Ok(LanguageModelCompletionEvent::Thinking {
                                        text: "".into(),
                                        signature: Some(sig),
                                    }))
                                }
                                ReasoningContentBlockDelta::RedactedContent(redacted) => {
                                    let content = String::from_utf8(redacted.into_inner())
                                        .unwrap_or("REDACTED".to_string());
                                    Some(Ok(LanguageModelCompletionEvent::Thinking {
                                        text: content,
                                        signature: None,
                                    }))
                                }
                                _ => None,
                            },
                            _ => None,
                        },
                        ConverseStreamOutput::ContentBlockStart(cb_start) => {
                            if let Some(ContentBlockStart::ToolUse(tool_start)) = cb_start.start {
                                state.tool_uses_by_index.insert(
                                    cb_start.content_block_index,
                                    RawToolUse {
                                        id: tool_start.tool_use_id,
                                        name: tool_start.name,
                                        input_json: String::new(),
                                    },
                                );
                            }
                            None
                        }
                        ConverseStreamOutput::MessageStart(_) => None,
                        ConverseStreamOutput::ContentBlockStop(cb_stop) => state
                            .tool_uses_by_index
                            .remove(&cb_stop.content_block_index)
                            .map(|tool_use| {
                                state.emitted_tool_use = true;

                                let input = parse_tool_arguments(&tool_use.input_json)
                                    .unwrap_or_else(|_| Value::Object(Default::default()));

                                Ok(LanguageModelCompletionEvent::ToolUse(
                                    LanguageModelToolUse {
                                        id: tool_use.id.into(),
                                        name: tool_use.name.into(),
                                        is_input_complete: true,
                                        raw_input: tool_use.input_json,
                                        input,
                                        thought_signature: None,
                                    },
                                ))
                            }),
                        ConverseStreamOutput::Metadata(cb_meta) => cb_meta.usage.map(|metadata| {
                            Ok(LanguageModelCompletionEvent::UsageUpdate(TokenUsage {
                                input_tokens: metadata.input_tokens as u64,
                                output_tokens: metadata.output_tokens as u64,
                                cache_creation_input_tokens: metadata
                                    .cache_write_input_tokens
                                    .unwrap_or_default()
                                    as u64,
                                cache_read_input_tokens: metadata
                                    .cache_read_input_tokens
                                    .unwrap_or_default()
                                    as u64,
                            }))
                        }),
                        ConverseStreamOutput::MessageStop(message_stop) => {
                            let stop_reason = if state.emitted_tool_use {
                                // 某些模型（例如 Kimi）即使进行了工具调用也会发送 EndTurn。
                                // 信任内容而不是停止原因。
                                language_model::StopReason::ToolUse
                            } else {
                                match message_stop.stop_reason {
                                    StopReason::ToolUse => language_model::StopReason::ToolUse,
                                    _ => language_model::StopReason::EndTurn,
                                }
                            };
                            Some(Ok(LanguageModelCompletionEvent::Stop(stop_reason)))
                        }
                        _ => None,
                    };

                    Some((result, state))
                }
                Err(err) => Some((
                    Some(Err(LanguageModelCompletionError::Other(anyhow!(err)))),
                    state,
                )),
            },
            None => None,
        }
    })
    .filter_map(|result| async move { result })
}

struct ConfigurationView {
    access_key_id_editor: Entity<InputField>,
    secret_access_key_editor: Entity<InputField>,
    session_token_editor: Entity<InputField>,
    bearer_token_editor: Entity<InputField>,
    state: Entity<State>,
    load_credentials_task: Option<Task<()>>,
    focus_handle: FocusHandle,
}

impl ConfigurationView {
    const PLACEHOLDER_ACCESS_KEY_ID_TEXT: &'static str = "XXXXXXXXXXXXXXXX";
    const PLACEHOLDER_SECRET_ACCESS_KEY_TEXT: &'static str =
        "XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX";
    const PLACEHOLDER_SESSION_TOKEN_TEXT: &'static str = "XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX";
    const PLACEHOLDER_BEARER_TOKEN_TEXT: &'static str = "XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX";

    fn new(state: Entity<State>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();

        cx.observe(&state, |_, _, cx| {
            cx.notify();
        })
        .detach();

        let access_key_id_editor = cx.new(|cx| {
            InputField::new(window, cx, Self::PLACEHOLDER_ACCESS_KEY_ID_TEXT)
                .label("访问密钥 ID")
                .tab_index(0)
                .tab_stop(true)
        });

        let secret_access_key_editor = cx.new(|cx| {
            InputField::new(window, cx, Self::PLACEHOLDER_SECRET_ACCESS_KEY_TEXT)
                .label("秘密访问密钥")
                .tab_index(1)
                .tab_stop(true)
        });

        let session_token_editor = cx.new(|cx| {
            InputField::new(window, cx, Self::PLACEHOLDER_SESSION_TOKEN_TEXT)
                .label("会话令牌（可选）")
                .tab_index(2)
                .tab_stop(true)
        });

        let bearer_token_editor = cx.new(|cx| {
            InputField::new(window, cx, Self::PLACEHOLDER_BEARER_TOKEN_TEXT)
                .label("Bedrock API 密钥")
                .tab_index(3)
                .tab_stop(true)
        });

        let load_credentials_task = Some(cx.spawn({
            let state = state.clone();
            async move |this, cx| {
                if let Some(task) = Some(state.update(cx, |state, cx| state.authenticate(cx))) {
                    // 我们不会记录错误，因为“未登录”也是一种错误。
                    let _ = task.await;
                }
                this.update(cx, |this, cx| {
                    this.load_credentials_task = None;
                    cx.notify();
                })
                .log_err();
            }
        }));

        Self {
            access_key_id_editor,
            secret_access_key_editor,
            session_token_editor,
            bearer_token_editor,
            state,
            load_credentials_task,
            focus_handle,
        }
    }

    fn save_credentials(
        &mut self,
        _: &menu::Confirm,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let access_key_id = self
            .access_key_id_editor
            .read(cx)
            .text(cx)
            .trim()
            .to_string();
        let secret_access_key = self
            .secret_access_key_editor
            .read(cx)
            .text(cx)
            .trim()
            .to_string();
        let session_token = self
            .session_token_editor
            .read(cx)
            .text(cx)
            .trim()
            .to_string();
        let session_token = if session_token.is_empty() {
            None
        } else {
            Some(session_token)
        };
        let bearer_token = self
            .bearer_token_editor
            .read(cx)
            .text(cx)
            .trim()
            .to_string();
        let bearer_token = if bearer_token.is_empty() {
            None
        } else {
            Some(bearer_token)
        };

        let state = self.state.clone();
        cx.spawn(async move |_, cx| {
            state
                .update(cx, |state, cx| {
                    let credentials = BedrockCredentials {
                        access_key_id,
                        secret_access_key,
                        session_token,
                        bearer_token,
                    };

                    state.set_static_credentials(credentials, cx)
                })
                .await
        })
        .detach_and_log_err(cx);
    }

    fn reset_credentials(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.access_key_id_editor
            .update(cx, |editor, cx| editor.set_text("", window, cx));
        self.secret_access_key_editor
            .update(cx, |editor, cx| editor.set_text("", window, cx));
        self.session_token_editor
            .update(cx, |editor, cx| editor.set_text("", window, cx));
        self.bearer_token_editor
            .update(cx, |editor, cx| editor.set_text("", window, cx));

        let state = self.state.clone();
        cx.spawn(async move |_, cx| state.update(cx, |state, cx| state.reset_auth(cx)).await)
            .detach_and_log_err(cx);
    }

    fn should_render_editor(&self, cx: &Context<Self>) -> bool {
        self.state.read(cx).is_authenticated()
    }

    fn on_tab(&mut self, _: &menu::SelectNext, window: &mut Window, cx: &mut Context<Self>) {
        window.focus_next(cx);
    }

    fn on_tab_prev(
        &mut self,
        _: &menu::SelectPrevious,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus_prev(cx);
    }
}

impl Render for ConfigurationView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let env_var_set = state.credentials_from_env;
        let auth = state.auth.clone();
        let settings_auth_method = state
            .settings
            .as_ref()
            .and_then(|s| s.authentication_method.clone());

        if self.load_credentials_task.is_some() {
            return div().child(Label::new("正在加载凭据...")).into_any();
        }

        let configured_label = match &auth {
            Some(BedrockAuth::Automatic) => {
                "正在使用自动凭据（AWS 默认链）".into()
            }
            Some(BedrockAuth::NamedProfile { profile_name }) => {
                format!("正在使用 AWS 配置文件：{profile_name}")
            }
            Some(BedrockAuth::SingleSignOn { profile_name }) => {
                format!("正在使用 AWS SSO 配置文件：{profile_name}")
            }
            Some(BedrockAuth::IamCredentials { .. }) if env_var_set => {
                format!(
                    "正在使用来自 {} 和 {} 环境变量的 IAM 凭据",
                    ZED_BEDROCK_ACCESS_KEY_ID_VAR.name, ZED_BEDROCK_SECRET_ACCESS_KEY_VAR.name
                )
            }
            Some(BedrockAuth::IamCredentials { .. }) => "正在使用 IAM 凭据".into(),
            Some(BedrockAuth::ApiKey { .. }) if env_var_set => {
                format!(
                    "正在使用来自 {} 环境变量的 Bedrock API 密钥",
                    ZED_BEDROCK_BEARER_TOKEN_VAR.name
                )
            }
            Some(BedrockAuth::ApiKey { .. }) => "正在使用 Bedrock API 密钥".into(),
            None => "未认证".into(),
        };

        // 确定凭据是否可以重置
        // 来自设置的身份验证（非 ApiKey）无法从界面重置
        let is_settings_derived = matches!(
            settings_auth_method,
            Some(BedrockAuthMethod::Automatic)
                | Some(BedrockAuthMethod::NamedProfile)
                | Some(BedrockAuthMethod::SingleSignOn)
        );

        let tooltip_label = if env_var_set {
            Some(format!(
                "要重置凭据，请取消设置 {}、{}、{} 或 {} 环境变量。",
                ZED_BEDROCK_ACCESS_KEY_ID_VAR.name,
                ZED_BEDROCK_SECRET_ACCESS_KEY_VAR.name,
                ZED_BEDROCK_SESSION_TOKEN_VAR.name,
                ZED_BEDROCK_BEARER_TOKEN_VAR.name
            ))
        } else if is_settings_derived {
            Some(
                "身份验证方法已在设置中配置。编辑 settings.json 以更改。"
                    .to_string(),
            )
        } else {
            None
        };

        if self.should_render_editor(cx) {
            return ConfiguredApiCard::new(configured_label)
                .disabled(env_var_set || is_settings_derived)
                .on_click(cx.listener(|this, _, window, cx| this.reset_credentials(window, cx)))
                .when_some(tooltip_label, |this, label| this.tooltip_label(label))
                .into_any_element();
        }

        v_flex()
            .min_w_0()
            .w_full()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_tab))
            .on_action(cx.listener(Self::on_tab_prev))
            .on_action(cx.listener(ConfigurationView::save_credentials))
            .child(Label::new("要将 Zed 的代理与 Bedrock 一起使用，你可以通过设置文件设置自定义身份验证策略，或使用静态凭据。"))
            .child(Label::new("但首先，要在 AWS 上访问模型，你需要：").mt_1())
            .child(
                List::new()
                    .child(
                        ListBulletItem::new("")
                            .child(Label::new(
                                "根据以下内容向你将使用的策略授予权限：",
                            ))
                            .child(ButtonLink::new(
                                "先决条件",
                                "https://docs.aws.amazon.com/bedrock/latest/userguide/inference-prereq.html",
                            )),
                    )
                    .child(
                        ListBulletItem::new("")
                            .child(Label::new("选择你要访问的模型："))
                            .child(ButtonLink::new(
                                "Bedrock 模型目录",
                                "https://us-east-1.console.aws.amazon.com/bedrock/home?region=us-east-1#/model-catalog",
                            )),
                    ),
            )
            .child(self.render_static_credentials_ui())
            .into_any()
    }
}

impl ConfigurationView {
    fn render_static_credentials_ui(&self) -> impl IntoElement {
        let section_header = |title: SharedString| {
            h_flex()
                .gap_2()
                .child(Label::new(title).size(LabelSize::Default))
                .child(Divider::horizontal())
        };

        let list_item = List::new()
            .child(
                ListBulletItem::new("")
                    .child(Label::new(
                        "对于访问密钥：在 AWS 控制台中创建一个具有编程访问权限的 IAM 用户",
                    ))
                    .child(ButtonLink::new(
                        "IAM 控制台",
                        "https://us-east-1.console.aws.amazon.com/iam/home?region=us-east-1#/users",
                    )),
            )
            .child(
                ListBulletItem::new("")
                    .child(Label::new("对于 Bedrock API 密钥：从以下位置生成 API 密钥"))
                    .child(ButtonLink::new(
                        "Bedrock 控制台",
                        "https://docs.aws.amazon.com/bedrock/latest/userguide/api-keys-use.html",
                    )),
            )
            .child(
                ListBulletItem::new("")
                    .child(Label::new("将必要的 Bedrock 权限附加到"))
                    .child(ButtonLink::new(
                        "此用户",
                        "https://docs.aws.amazon.com/bedrock/latest/userguide/inference-prereq.html",
                    )),
            )
            .child(ListBulletItem::new(
                "在下面输入访问密钥或 Bedrock API 密钥（不要同时输入两者）",
            ));

        v_flex()
            .my_2()
            .tab_group()
            .gap_1p5()
            .child(section_header("静态凭据".into()))
            .child(Label::new(
                "此方法使用你的 AWS 访问密钥 ID 和秘密访问密钥，或 Bedrock API 密钥。",
            ))
            .child(list_item)
            .child(self.access_key_id_editor.clone())
            .child(self.secret_access_key_editor.clone())
            .child(self.session_token_editor.clone())
            .child(
                Label::new(format!(
                    "你还可以设置 {}、{} 和 {} 环境变量（或 {} 用于 Bedrock API 密钥身份验证）并重新启动 Zed。",
                    ZED_BEDROCK_ACCESS_KEY_ID_VAR.name,
                    ZED_BEDROCK_SECRET_ACCESS_KEY_VAR.name,
                    ZED_BEDROCK_REGION_VAR.name,
                    ZED_BEDROCK_BEARER_TOKEN_VAR.name
                ))
                .size(LabelSize::Small)
                .color(Color::Muted),
            )
            .child(
                Label::new(format!(
                    "可选地，如果你的环境使用 AWS CLI 配置文件，你可以设置 {}；如果需要自定义端点，你可以设置 {}；如果需要会话令牌，你可以设置 {}。",
                    ZED_AWS_PROFILE_VAR.name,
                    ZED_AWS_ENDPOINT_VAR.name,
                    ZED_BEDROCK_SESSION_TOKEN_VAR.name
                ))
                .size(LabelSize::Small)
                .color(Color::Muted)
                .mt_1()
                .mb_2p5(),
            )
            .child(section_header("使用 API 密钥".into()))
            .child(self.bearer_token_editor.clone())
            .child(
                Label::new(format!(
                    "区域通过 {} 环境变量或 settings.json 配置（默认为 us-east-1）。",
                    ZED_BEDROCK_REGION_VAR.name
                ))
                .size(LabelSize::Small)
                .color(Color::Muted)
            )
    }
}