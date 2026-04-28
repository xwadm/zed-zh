use anyhow::{Result, anyhow};
use credentials_provider::CredentialsProvider;
use fs::Fs;
use futures::{FutureExt, StreamExt, future::BoxFuture, stream::BoxStream};
use futures::{Stream, TryFutureExt, stream};
use gpui::{AnyView, App, AsyncApp, Context, CursorStyle, Entity, Task};
use http_client::HttpClient;
use language_model::{
    ApiKeyState, AuthenticateError, EnvVar, IconOrSvg, LanguageModel, LanguageModelCompletionError,
    LanguageModelCompletionEvent, LanguageModelId, LanguageModelName, LanguageModelProvider,
    LanguageModelProviderId, LanguageModelProviderName, LanguageModelProviderState,
    LanguageModelRequest, LanguageModelRequestTool, LanguageModelToolChoice, LanguageModelToolUse,
    LanguageModelToolUseId, MessageContent, RateLimiter, Role, StopReason, TokenUsage, env_var,
};
use menu;
use ollama::{
    ChatMessage, ChatOptions, ChatRequest, ChatResponseDelta, OLLAMA_API_URL, OllamaFunctionCall,
    OllamaFunctionTool, OllamaToolCall, get_models, show_model, stream_chat_completion,
};
pub use settings::OllamaAvailableModel as AvailableModel;
use settings::{Settings, SettingsStore, update_settings_file};
use std::pin::Pin;
use std::sync::LazyLock;
use std::{collections::HashMap, sync::Arc};
use ui::{
    ButtonLike, ButtonLink, ConfiguredApiCard, ElevationIndex, List, ListBulletItem, Tooltip,
    prelude::*,
};
use ui_input::InputField;

use crate::AllLanguageModelSettings;

const OLLAMA_DOWNLOAD_URL: &str = "https://ollama.com/download";
const OLLAMA_LIBRARY_URL: &str = "https://ollama.com/library";
const OLLAMA_SITE: &str = "https://ollama.com/";

const PROVIDER_ID: LanguageModelProviderId = LanguageModelProviderId::new("ollama");
const PROVIDER_NAME: LanguageModelProviderName = LanguageModelProviderName::new("Ollama");

const API_KEY_ENV_VAR_NAME: &str = "OLLAMA_API_KEY";
static API_KEY_ENV_VAR: LazyLock<EnvVar> = env_var!(API_KEY_ENV_VAR_NAME);

#[derive(Default, Debug, Clone, PartialEq)]
pub struct OllamaSettings {
    pub api_url: String,
    pub auto_discover: bool,
    pub available_models: Vec<AvailableModel>,
    pub context_window: Option<u64>,
}

pub struct OllamaLanguageModelProvider {
    http_client: Arc<dyn HttpClient>,
    state: Entity<State>,
}

pub struct State {
    api_key_state: ApiKeyState,
    credentials_provider: Arc<dyn CredentialsProvider>,
    http_client: Arc<dyn HttpClient>,
    fetched_models: Vec<ollama::Model>,
    fetch_model_task: Option<Task<Result<()>>>,
}

impl State {
    fn is_authenticated(&self) -> bool {
        !self.fetched_models.is_empty()
    }

    fn set_api_key(&mut self, api_key: Option<String>, cx: &mut Context<Self>) -> Task<Result<()>> {
        let credentials_provider = self.credentials_provider.clone();
        let api_url = OllamaLanguageModelProvider::api_url(cx);
        let task = self.api_key_state.store(
            api_url,
            api_key,
            |this| &mut this.api_key_state,
            credentials_provider,
            cx,
        );

        self.fetched_models.clear();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| this.restart_fetch_models_task(cx))
                .ok();
            result
        })
    }

    fn authenticate(&mut self, cx: &mut Context<Self>) -> Task<Result<(), AuthenticateError>> {
        let credentials_provider = self.credentials_provider.clone();
        let api_url = OllamaLanguageModelProvider::api_url(cx);
        let task = self.api_key_state.load_if_needed(
            api_url,
            |this| &mut this.api_key_state,
            credentials_provider,
            cx,
        );

        // 始终尝试获取模型 - 如果不需要 API 密钥（本地 Ollama），可以直接工作；
        // 如果需要且已提供 API 密钥，可以工作；
        // 如果需要但未提供 API 密钥，则会优雅地失败。
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| this.restart_fetch_models_task(cx))
                .ok();
            result
        })
    }

    fn fetch_models(&mut self, cx: &mut Context<Self>) -> Task<Result<()>> {
        let http_client = Arc::clone(&self.http_client);
        let api_url = OllamaLanguageModelProvider::api_url(cx);
        let api_key = self.api_key_state.key(&api_url);

        // 通过检查服务器是否可用来代理“已认证”状态，即通过获取模型列表来判断
        cx.spawn(async move |this, cx| {
            let models = get_models(http_client.as_ref(), &api_url, api_key.as_deref()).await?;

            let tasks = models
                .into_iter()
                // Ollama API 没有元数据标识哪些是嵌入模型，
                // 所以直接过滤掉名称中包含 “-embed” 的模型
                .filter(|model| !model.name.contains("-embed"))
                .map(|model| {
                    let http_client = Arc::clone(&http_client);
                    let api_url = api_url.clone();
                    let api_key = api_key.clone();
                    async move {
                        let name = model.name.as_str();
                        let model =
                            show_model(http_client.as_ref(), &api_url, api_key.as_deref(), name)
                                .await?;
                        let ollama_model = ollama::Model::new(
                            name,
                            None,
                            model.context_length,
                            Some(model.supports_tools()),
                            Some(model.supports_vision()),
                            Some(model.supports_thinking()),
                        );
                        Ok(ollama_model)
                    }
                });

            // 因为可用模型数量可能很多，对能力获取进行速率限制
            let mut ollama_models: Vec<_> = futures::stream::iter(tasks)
                .buffer_unordered(5)
                .collect::<Vec<Result<_>>>()
                .await
                .into_iter()
                .collect::<Result<Vec<_>>>()?;

            ollama_models.sort_by(|a, b| a.name.cmp(&b.name));

            this.update(cx, |this, cx| {
                this.fetched_models = ollama_models;
                cx.notify();
            })
        })
    }

    fn restart_fetch_models_task(&mut self, cx: &mut Context<Self>) {
        let task = self.fetch_models(cx);
        self.fetch_model_task.replace(task);
    }
}

impl OllamaLanguageModelProvider {
    pub fn new(
        http_client: Arc<dyn HttpClient>,
        credentials_provider: Arc<dyn CredentialsProvider>,
        cx: &mut App,
    ) -> Self {
        let this = Self {
            http_client: http_client.clone(),
            state: cx.new(|cx| {
                cx.observe_global::<SettingsStore>({
                    let mut last_settings = OllamaLanguageModelProvider::settings(cx).clone();
                    move |this: &mut State, cx| {
                        let current_settings = OllamaLanguageModelProvider::settings(cx);
                        let settings_changed = current_settings != &last_settings;
                        if settings_changed {
                            let url_changed = last_settings.api_url != current_settings.api_url;
                            last_settings = current_settings.clone();
                            if url_changed {
                                let credentials_provider = this.credentials_provider.clone();
                                let api_url = Self::api_url(cx);
                                this.api_key_state.handle_url_change(
                                    api_url,
                                    |this| &mut this.api_key_state,
                                    credentials_provider,
                                    cx,
                                );
                                this.fetched_models.clear();
                                this.authenticate(cx).detach();
                            }
                            cx.notify();
                        }
                    }
                })
                .detach();

                State {
                    http_client,
                    fetched_models: Default::default(),
                    fetch_model_task: None,
                    api_key_state: ApiKeyState::new(Self::api_url(cx), (*API_KEY_ENV_VAR).clone()),
                    credentials_provider,
                }
            }),
        };
        this
    }

    fn settings(cx: &App) -> &OllamaSettings {
        &AllLanguageModelSettings::get_global(cx).ollama
    }

    fn api_url(cx: &App) -> SharedString {
        let api_url = &Self::settings(cx).api_url;
        if api_url.is_empty() {
            OLLAMA_API_URL.into()
        } else {
            SharedString::new(api_url.as_str())
        }
    }

    fn has_custom_url(cx: &App) -> bool {
        Self::settings(cx).api_url != OLLAMA_API_URL
    }
}

impl LanguageModelProviderState for OllamaLanguageModelProvider {
    type ObservableEntity = State;

    fn observable_entity(&self) -> Option<Entity<Self::ObservableEntity>> {
        Some(self.state.clone())
    }
}

impl LanguageModelProvider for OllamaLanguageModelProvider {
    fn id(&self) -> LanguageModelProviderId {
        PROVIDER_ID
    }

    fn name(&self) -> LanguageModelProviderName {
        PROVIDER_NAME
    }

    fn icon(&self) -> IconOrSvg {
        IconOrSvg::Icon(IconName::AiOllama)
    }

    fn default_model(&self, _: &App) -> Option<Arc<dyn LanguageModel>> {
        // 我们不应尝试选择默认模型，因为这可能会导致加载一个尚未加载的模型。
        // 在资源受限的环境中，默认加载某个模型会导致糟糕的用户体验。
        None
    }

    fn default_fast_model(&self, _: &App) -> Option<Arc<dyn LanguageModel>> {
        // 参见 default_model 的说明。
        None
    }

    fn provided_models(&self, cx: &App) -> Vec<Arc<dyn LanguageModel>> {
        let mut models: HashMap<String, ollama::Model> = HashMap::new();
        let settings = OllamaLanguageModelProvider::settings(cx);

        // 从 Ollama API 返回的模型中添加
        for model in self.state.read(cx).fetched_models.iter() {
            let mut model = model.clone();
            if let Some(context_window) = settings.context_window {
                model.max_tokens = context_window;
            }
            models.insert(model.name.clone(), model);
        }

        // 用设置中的可用模型覆盖
        merge_settings_into_models(
            &mut models,
            &settings.available_models,
            settings.context_window,
        );

        let mut models = models
            .into_values()
            .map(|model| {
                Arc::new(OllamaLanguageModel {
                    id: LanguageModelId::from(model.name.clone()),
                    model,
                    http_client: self.http_client.clone(),
                    request_limiter: RateLimiter::new(4),
                    state: self.state.clone(),
                }) as Arc<dyn LanguageModel>
            })
            .collect::<Vec<_>>();
        models.sort_by_key(|model| model.name());
        models
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
        let state = self.state.clone();
        cx.new(|cx| ConfigurationView::new(state, window, cx))
            .into()
    }

    fn reset_credentials(&self, cx: &mut App) -> Task<Result<()>> {
        self.state
            .update(cx, |state, cx| state.set_api_key(None, cx))
    }
}

pub struct OllamaLanguageModel {
    id: LanguageModelId,
    model: ollama::Model,
    http_client: Arc<dyn HttpClient>,
    request_limiter: RateLimiter,
    state: Entity<State>,
}

impl OllamaLanguageModel {
    fn to_ollama_request(&self, request: LanguageModelRequest) -> ChatRequest {
        let supports_vision = self.model.supports_vision.unwrap_or(false);

        let mut messages = Vec::with_capacity(request.messages.len());

        for mut msg in request.messages.into_iter() {
            let images = if supports_vision {
                msg.content
                    .iter()
                    .filter_map(|content| match content {
                        MessageContent::Image(image) => Some(image.source.to_string()),
                        _ => None,
                    })
                    .collect::<Vec<String>>()
            } else {
                vec![]
            };

            match msg.role {
                Role::User => {
                    for tool_result in msg
                        .content
                        .extract_if(.., |x| matches!(x, MessageContent::ToolResult(..)))
                    {
                        match tool_result {
                            MessageContent::ToolResult(tool_result) => {
                                messages.push(ChatMessage::Tool {
                                    tool_name: tool_result.tool_name.to_string(),
                                    content: tool_result.content.to_str().unwrap_or("").to_string(),
                                })
                            }
                            _ => unreachable!("只应提取工具结果"),
                        }
                    }
                    if !msg.content.is_empty() {
                        messages.push(ChatMessage::User {
                            content: msg.string_contents(),
                            images: if images.is_empty() {
                                None
                            } else {
                                Some(images)
                            },
                        })
                    }
                }
                Role::Assistant => {
                    let content = msg.string_contents();
                    let mut thinking = None;
                    let mut tool_calls = Vec::new();
                    for content in msg.content.into_iter() {
                        match content {
                            MessageContent::Thinking { text, .. } if !text.is_empty() => {
                                thinking = Some(text)
                            }
                            MessageContent::ToolUse(tool_use) => {
                                tool_calls.push(OllamaToolCall {
                                    id: tool_use.id.to_string(),
                                    function: OllamaFunctionCall {
                                        name: tool_use.name.to_string(),
                                        arguments: tool_use.input,
                                    },
                                });
                            }
                            _ => (),
                        }
                    }
                    messages.push(ChatMessage::Assistant {
                        content,
                        tool_calls: Some(tool_calls),
                        images: if images.is_empty() {
                            None
                        } else {
                            Some(images)
                        },
                        thinking,
                    })
                }
                Role::System => messages.push(ChatMessage::System {
                    content: msg.string_contents(),
                }),
            }
        }
        ChatRequest {
            model: self.model.name.clone(),
            messages,
            keep_alive: self.model.keep_alive.clone().unwrap_or_default(),
            stream: true,
            options: Some(ChatOptions {
                num_ctx: Some(self.model.max_tokens),
                // 仅在显式提供停止词时发送。当为空/None 时，
                // Ollama 将使用模型 Modelfile 中定义的默认停止词。
                // 发送空数组会覆盖并禁用默认值。
                stop: if request.stop.is_empty() {
                    None
                } else {
                    Some(request.stop)
                },
                temperature: request.temperature.or(Some(1.0)),
                ..Default::default()
            }),
            think: self
                .model
                .supports_thinking
                .map(|supports_thinking| supports_thinking && request.thinking_allowed),
            tools: if self.model.supports_tools.unwrap_or(false) {
                request.tools.into_iter().map(tool_into_ollama).collect()
            } else {
                vec![]
            },
        }
    }
}

impl LanguageModel for OllamaLanguageModel {
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
        self.model.supports_tools.unwrap_or(false)
    }

    fn supports_images(&self) -> bool {
        self.model.supports_vision.unwrap_or(false)
    }

    fn supports_thinking(&self) -> bool {
        self.model.supports_thinking.unwrap_or(false)
    }

    fn supports_tool_choice(&self, choice: LanguageModelToolChoice) -> bool {
        match choice {
            LanguageModelToolChoice::Auto => false,
            LanguageModelToolChoice::Any => false,
            LanguageModelToolChoice::None => false,
        }
    }

    fn telemetry_id(&self) -> String {
        format!("ollama/{}", self.model.id())
    }

    fn max_token_count(&self) -> u64 {
        self.model.max_token_count()
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
        let request = self.to_ollama_request(request);

        let http_client = self.http_client.clone();
        let (api_key, api_url) = self.state.read_with(cx, |state, cx| {
            let api_url = OllamaLanguageModelProvider::api_url(cx);
            (state.api_key_state.key(&api_url), api_url)
        });

        let future = self.request_limiter.stream(async move {
            let stream =
                stream_chat_completion(http_client.as_ref(), &api_url, api_key.as_deref(), request)
                    .await?;
            let stream = map_to_language_model_completion_events(stream);
            Ok(stream)
        });

        future.map_ok(|f| f.boxed()).boxed()
    }
}

fn map_to_language_model_completion_events(
    stream: Pin<Box<dyn Stream<Item = anyhow::Result<ChatResponseDelta>> + Send>>,
) -> impl Stream<Item = Result<LanguageModelCompletionEvent, LanguageModelCompletionError>> {
    struct State {
        stream: Pin<Box<dyn Stream<Item = anyhow::Result<ChatResponseDelta>> + Send>>,
        used_tools: bool,
    }

    // 我们需要从原始流的单个响应中创建一个 ToolUse 和一个 Stop 事件
    let stream = stream::unfold(
        State {
            stream,
            used_tools: false,
        },
        async move |mut state| {
            let response = state.stream.next().await?;

            let delta = match response {
                Ok(delta) => delta,
                Err(e) => {
                    let event = Err(LanguageModelCompletionError::from(anyhow!(e)));
                    return Some((vec![event], state));
                }
            };

            let mut events = Vec::new();

            match delta.message {
                ChatMessage::User { content, images: _ } => {
                    events.push(Ok(LanguageModelCompletionEvent::Text(content)));
                }
                ChatMessage::System { content } => {
                    events.push(Ok(LanguageModelCompletionEvent::Text(content)));
                }
                ChatMessage::Tool { content, .. } => {
                    events.push(Ok(LanguageModelCompletionEvent::Text(content)));
                }
                ChatMessage::Assistant {
                    content,
                    tool_calls,
                    images: _,
                    thinking,
                } => {
                    if let Some(text) = thinking {
                        events.push(Ok(LanguageModelCompletionEvent::Thinking {
                            text,
                            signature: None,
                        }));
                    }

                    if let Some(tool_call) = tool_calls.and_then(|v| v.into_iter().next()) {
                        let OllamaToolCall { id, function } = tool_call;
                        let event = LanguageModelCompletionEvent::ToolUse(LanguageModelToolUse {
                            id: LanguageModelToolUseId::from(id),
                            name: Arc::from(function.name),
                            raw_input: function.arguments.to_string(),
                            input: function.arguments,
                            is_input_complete: true,
                            thought_signature: None,
                        });
                        events.push(Ok(event));
                        state.used_tools = true;
                    } else if !content.is_empty() {
                        events.push(Ok(LanguageModelCompletionEvent::Text(content)));
                    }
                }
            };

            if delta.done {
                events.push(Ok(LanguageModelCompletionEvent::UsageUpdate(TokenUsage {
                    input_tokens: delta.prompt_eval_count.unwrap_or(0),
                    output_tokens: delta.eval_count.unwrap_or(0),
                    cache_creation_input_tokens: 0,
                    cache_read_input_tokens: 0,
                })));
                if state.used_tools {
                    state.used_tools = false;
                    events.push(Ok(LanguageModelCompletionEvent::Stop(StopReason::ToolUse)));
                } else {
                    events.push(Ok(LanguageModelCompletionEvent::Stop(StopReason::EndTurn)));
                }
            }

            Some((events, state))
        },
    );

    stream.flat_map(futures::stream::iter)
}

struct ConfigurationView {
    api_key_editor: Entity<InputField>,
    api_url_editor: Entity<InputField>,
    context_window_editor: Entity<InputField>,
    state: Entity<State>,
}

impl ConfigurationView {
    pub fn new(state: Entity<State>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let api_key_editor = cx.new(|cx| InputField::new(window, cx, "63e02e...").label("API 密钥"));

        let api_url_editor = cx.new(|cx| {
            let input = InputField::new(window, cx, OLLAMA_API_URL).label("API URL");
            input.set_text(&OllamaLanguageModelProvider::api_url(cx), window, cx);
            input
        });

        let context_window_editor = cx.new(|cx| {
            let input = InputField::new(window, cx, "8192").label("上下文窗口");
            if let Some(context_window) = OllamaLanguageModelProvider::settings(cx).context_window {
                input.set_text(&context_window.to_string(), window, cx);
            }
            input
        });

        cx.observe(&state, |_, _, cx| {
            cx.notify();
        })
        .detach();

        Self {
            api_key_editor,
            api_url_editor,
            context_window_editor,
            state,
        }
    }

    fn retry_connection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let has_api_url = OllamaLanguageModelProvider::has_custom_url(cx);
        let has_api_key = self
            .state
            .read_with(cx, |state, _| state.api_key_state.has_key());
        if !has_api_url {
            self.save_api_url(cx);
        }
        if !has_api_key {
            self.save_api_key(&Default::default(), window, cx);
        }

        self.state.update(cx, |state, cx| {
            state.restart_fetch_models_task(cx);
        });
    }

    fn save_api_key(&mut self, _: &menu::Confirm, window: &mut Window, cx: &mut Context<Self>) {
        let api_key = self.api_key_editor.read(cx).text(cx).trim().to_string();
        if api_key.is_empty() {
            return;
        }

        // URL 更改可能导致编辑器再次显示
        self.api_key_editor
            .update(cx, |input, cx| input.set_text("", window, cx));

        let state = self.state.clone();
        cx.spawn_in(window, async move |_, cx| {
            state
                .update(cx, |state, cx| state.set_api_key(Some(api_key), cx))
                .await
        })
        .detach_and_log_err(cx);
    }

    fn reset_api_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.api_key_editor
            .update(cx, |input, cx| input.set_text("", window, cx));

        let state = self.state.clone();
        cx.spawn_in(window, async move |_, cx| {
            state
                .update(cx, |state, cx| state.set_api_key(None, cx))
                .await
        })
        .detach_and_log_err(cx);

        cx.notify();
    }

    fn save_api_url(&self, cx: &mut Context<Self>) {
        let api_url = self.api_url_editor.read(cx).text(cx).trim().to_string();
        let current_url = OllamaLanguageModelProvider::api_url(cx);
        if !api_url.is_empty() && &api_url != &current_url {
            let fs = <dyn Fs>::global(cx);
            update_settings_file(fs, cx, move |settings, _| {
                settings
                    .language_models
                    .get_or_insert_default()
                    .ollama
                    .get_or_insert_default()
                    .api_url = Some(api_url);
            });
        }
    }

    fn reset_api_url(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.api_url_editor
            .update(cx, |input, cx| input.set_text("", window, cx));
        let fs = <dyn Fs>::global(cx);
        update_settings_file(fs, cx, |settings, _cx| {
            if let Some(settings) = settings
                .language_models
                .as_mut()
                .and_then(|models| models.ollama.as_mut())
            {
                settings.api_url = Some(OLLAMA_API_URL.into());
            }
        });
        cx.notify();
    }

    fn save_context_window(&mut self, cx: &mut Context<Self>) {
        let context_window_str = self
            .context_window_editor
            .read(cx)
            .text(cx)
            .trim()
            .to_string();
        let current_context_window = OllamaLanguageModelProvider::settings(cx).context_window;

        if let Ok(context_window) = context_window_str.parse::<u64>() {
            if Some(context_window) != current_context_window {
                let fs = <dyn Fs>::global(cx);
                update_settings_file(fs, cx, move |settings, _| {
                    settings
                        .language_models
                        .get_or_insert_default()
                        .ollama
                        .get_or_insert_default()
                        .context_window = Some(context_window);
                });
            }
        } else if context_window_str.is_empty() && current_context_window.is_some() {
            let fs = <dyn Fs>::global(cx);
            update_settings_file(fs, cx, move |settings, _| {
                settings
                    .language_models
                    .get_or_insert_default()
                    .ollama
                    .get_or_insert_default()
                    .context_window = None;
            });
        }
    }

    fn reset_context_window(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.context_window_editor
            .update(cx, |input, cx| input.set_text("", window, cx));
        let fs = <dyn Fs>::global(cx);
        update_settings_file(fs, cx, |settings, _cx| {
            if let Some(settings) = settings
                .language_models
                .as_mut()
                .and_then(|models| models.ollama.as_mut())
            {
                settings.context_window = None;
            }
        });
        cx.notify();
    }

    fn render_instructions(cx: &App) -> Div {
        v_flex()
            .gap_2()
            .child(Label::new(
                "使用 Ollama 在本地机器上运行 LLM，或连接到 Ollama 服务器。\
                可以访问 Llama、Mistral、Gemma 以及数百种其他模型。",
            ))
            .child(Label::new("使用本地 Ollama："))
            .child(
                List::new()
                    .child(
                        ListBulletItem::new("")
                            .child(Label::new("从以下网址下载并安装 Ollama："))
                            .child(ButtonLink::new("ollama.com", "https://ollama.com/download")),
                    )
                    .child(
                        ListBulletItem::new("")
                            .child(Label::new("启动 Ollama 并下载模型："))
                            .child(Label::new("ollama run gpt-oss:20b").inline_code(cx)),
                    )
                    .child(ListBulletItem::new(
                        "点击下方的“连接”按钮开始在 Zed 中使用 Ollama",
                    )),
            )
            .child(Label::new(
                "或者，您可以通过指定 URL 和 API 密钥（可能不需要）连接到 Ollama 服务器：",
            ))
    }

    fn render_api_key_editor(&self, cx: &Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let env_var_set = state.api_key_state.is_from_env_var();
        let configured_card_label = if env_var_set {
            format!("API 密钥已通过 {API_KEY_ENV_VAR_NAME} 环境变量设置。")
        } else {
            "API 密钥已配置".to_string()
        };

        if !state.api_key_state.has_key() {
            v_flex()
              .on_action(cx.listener(Self::save_api_key))
              .child(self.api_key_editor.clone())
              .child(
                  Label::new(
                      format!("您也可以设置 {API_KEY_ENV_VAR_NAME} 环境变量并重启 Zed。")
                  )
                  .size(LabelSize::Small)
                  .color(Color::Muted),
              )
              .into_any_element()
        } else {
            ConfiguredApiCard::new(configured_card_label)
                .disabled(env_var_set)
                .on_click(cx.listener(|this, _, window, cx| this.reset_api_key(window, cx)))
                .when(env_var_set, |this| {
                    this.tooltip_label(format!("要重置 API 密钥，请取消设置 {API_KEY_ENV_VAR_NAME} 环境变量。"))
                })
                .into_any_element()
        }
    }

    fn render_context_window_editor(&self, cx: &Context<Self>) -> Div {
        let settings = OllamaLanguageModelProvider::settings(cx);
        let custom_context_window_set = settings.context_window.is_some();

        if custom_context_window_set {
            h_flex()
                .p_3()
                .justify_between()
                .rounded_md()
                .border_1()
                .border_color(cx.theme().colors().border)
                .bg(cx.theme().colors().elevated_surface_background)
                .child(
                    h_flex()
                        .gap_2()
                        .child(Icon::new(IconName::Check).color(Color::Success))
                        .child(v_flex().gap_1().child(Label::new(format!(
                            "上下文窗口: {}",
                            settings.context_window.unwrap()
                        )))),
                )
                .child(
                    Button::new("reset-context-window", "重置")
                        .label_size(LabelSize::Small)
                        .start_icon(Icon::new(IconName::Undo).size(IconSize::Small))
                        .layer(ElevationIndex::ModalSurface)
                        .on_click(
                            cx.listener(|this, _, window, cx| {
                                this.reset_context_window(window, cx)
                            }),
                        ),
                )
        } else {
            v_flex()
                .on_action(
                    cx.listener(|this, _: &menu::Confirm, _window, cx| {
                        this.save_context_window(cx)
                    }),
                )
                .child(self.context_window_editor.clone())
                .child(
                    Label::new("默认：由模型决定")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
        }
    }

    fn render_api_url_editor(&self, cx: &Context<Self>) -> Div {
        let api_url = OllamaLanguageModelProvider::api_url(cx);
        let custom_api_url_set = api_url != OLLAMA_API_URL;

        if custom_api_url_set {
            h_flex()
                .p_3()
                .justify_between()
                .rounded_md()
                .border_1()
                .border_color(cx.theme().colors().border)
                .bg(cx.theme().colors().elevated_surface_background)
                .child(
                    h_flex()
                        .gap_2()
                        .child(Icon::new(IconName::Check).color(Color::Success))
                        .child(v_flex().gap_1().child(Label::new(api_url))),
                )
                .child(
                    Button::new("reset-api-url", "重置 API URL")
                        .label_size(LabelSize::Small)
                        .start_icon(Icon::new(IconName::Undo).size(IconSize::Small))
                        .layer(ElevationIndex::ModalSurface)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.reset_api_url(window, cx)),
                        ),
                )
        } else {
            v_flex()
                .on_action(cx.listener(|this, _: &menu::Confirm, _window, cx| {
                    this.save_api_url(cx);
                    cx.notify();
                }))
                .gap_2()
                .child(self.api_url_editor.clone())
        }
    }
}

impl Render for ConfigurationView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_authenticated = self.state.read(cx).is_authenticated();

        v_flex()
            .gap_2()
            .child(Self::render_instructions(cx))
            .child(self.render_api_url_editor(cx))
            .child(self.render_context_window_editor(cx))
            .child(self.render_api_key_editor(cx))
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .gap_2()
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .map(|this| {
                                if is_authenticated {
                                    this.child(
                                        Button::new("ollama-site", "Ollama")
                                            .style(ButtonStyle::Subtle)
                                            .end_icon(
                                                Icon::new(IconName::ArrowUpRight)
                                                    .size(IconSize::XSmall)
                                                    .color(Color::Muted),
                                            )
                                            .on_click(move |_, _, cx| cx.open_url(OLLAMA_SITE))
                                            .into_any_element(),
                                    )
                                } else {
                                    this.child(
                                        Button::new("download_ollama_button", "下载 Ollama")
                                            .style(ButtonStyle::Subtle)
                                            .end_icon(
                                                Icon::new(IconName::ArrowUpRight)
                                                    .size(IconSize::XSmall)
                                                    .color(Color::Muted),
                                            )
                                            .on_click(move |_, _, cx| {
                                                cx.open_url(OLLAMA_DOWNLOAD_URL)
                                            })
                                            .into_any_element(),
                                    )
                                }
                            })
                            .child(
                                Button::new("view-models", "查看所有模型")
                                    .style(ButtonStyle::Subtle)
                                    .end_icon(
                                        Icon::new(IconName::ArrowUpRight)
                                            .size(IconSize::XSmall)
                                            .color(Color::Muted),
                                    )
                                    .on_click(move |_, _, cx| cx.open_url(OLLAMA_LIBRARY_URL)),
                            ),
                    )
                    .map(|this| {
                        if is_authenticated {
                            this.child(
                                ButtonLike::new("connected")
                                    .disabled(true)
                                    .cursor_style(CursorStyle::Arrow)
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .child(Icon::new(IconName::Check).color(Color::Success))
                                            .child(Label::new("已连接"))
                                            .into_any_element(),
                                    )
                                    .child(
                                        IconButton::new("refresh-models", IconName::RotateCcw)
                                            .tooltip(Tooltip::text("刷新模型"))
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.state.update(cx, |state, _| {
                                                    state.fetched_models.clear();
                                                });
                                                this.retry_connection(window, cx);
                                            })),
                                    ),
                            )
                        } else {
                            this.child(
                                Button::new("retry_ollama_models", "连接")
                                    .start_icon(
                                        Icon::new(IconName::PlayOutlined).size(IconSize::XSmall),
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.retry_connection(window, cx)
                                    })),
                            )
                        }
                    }),
            )
    }
}

fn merge_settings_into_models(
    models: &mut HashMap<String, ollama::Model>,
    available_models: &[AvailableModel],
    context_window: Option<u64>,
) {
    for setting_model in available_models {
        if let Some(model) = models.get_mut(&setting_model.name) {
            if context_window.is_none() {
                model.max_tokens = setting_model.max_tokens;
            }
            model.display_name = setting_model.display_name.clone();
            model.keep_alive = setting_model.keep_alive.clone();
            model.supports_tools = setting_model.supports_tools;
            model.supports_vision = setting_model.supports_images;
            model.supports_thinking = setting_model.supports_thinking;
        } else {
            models.insert(
                setting_model.name.clone(),
                ollama::Model {
                    name: setting_model.name.clone(),
                    display_name: setting_model.display_name.clone(),
                    max_tokens: context_window.unwrap_or(setting_model.max_tokens),
                    keep_alive: setting_model.keep_alive.clone(),
                    supports_tools: setting_model.supports_tools,
                    supports_vision: setting_model.supports_images,
                    supports_thinking: setting_model.supports_thinking,
                },
            );
        }
    }
}

fn tool_into_ollama(tool: LanguageModelRequestTool) -> ollama::OllamaTool {
    ollama::OllamaTool::Function {
        function: OllamaFunctionTool {
            name: tool.name,
            description: Some(tool.description),
            parameters: Some(tool.input_schema),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_settings_preserves_display_names_for_similar_models() {
        // 回归测试：https://github.com/zed-industries/zed/issues/43646
        // 当多个模型共享相同的基本名称（例如 qwen2.5-coder:1.5b 和 qwen2.5-coder:3b）时，
        // 每个模型都应从设置中获取各自的 display_name，而不是随机获取一个。

        let mut models: HashMap<String, ollama::Model> = HashMap::new();
        models.insert(
            "qwen2.5-coder:1.5b".to_string(),
            ollama::Model {
                name: "qwen2.5-coder:1.5b".to_string(),
                display_name: None,
                max_tokens: 4096,
                keep_alive: None,
                supports_tools: None,
                supports_vision: None,
                supports_thinking: None,
            },
        );
        models.insert(
            "qwen2.5-coder:3b".to_string(),
            ollama::Model {
                name: "qwen2.5-coder:3b".to_string(),
                display_name: None,
                max_tokens: 4096,
                keep_alive: None,
                supports_tools: None,
                supports_vision: None,
                supports_thinking: None,
            },
        );

        let available_models = vec![
            AvailableModel {
                name: "qwen2.5-coder:1.5b".to_string(),
                display_name: Some("QWEN2.5 Coder 1.5B".to_string()),
                max_tokens: 5000,
                keep_alive: None,
                supports_tools: Some(true),
                supports_images: None,
                supports_thinking: None,
            },
            AvailableModel {
                name: "qwen2.5-coder:3b".to_string(),
                display_name: Some("QWEN2.5 Coder 3B".to_string()),
                max_tokens: 6000,
                keep_alive: None,
                supports_tools: Some(true),
                supports_images: None,
                supports_thinking: None,
            },
        ];

        merge_settings_into_models(&mut models, &available_models, None);

        let model_1_5b = models
            .get("qwen2.5-coder:1.5b")
            .expect("1.5b 模型缺失");
        let model_3b = models.get("qwen2.5-coder:3b").expect("3b 模型缺失");

        assert_eq!(
            model_1_5b.display_name,
            Some("QWEN2.5 Coder 1.5B".to_string()),
            "1.5b 模型应具有自己的 display_name"
        );
        assert_eq!(model_1_5b.max_tokens, 5000);

        assert_eq!(
            model_3b.display_name,
            Some("QWEN2.5 Coder 3B".to_string()),
            "3b 模型应具有自己的 display_name"
        );
        assert_eq!(model_3b.max_tokens, 6000);
    }
}