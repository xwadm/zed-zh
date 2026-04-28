use anyhow::{Context as _, Result};
use collections::HashMap;
use context_server::{ContextServerCommand, ContextServerId};
use editor::{Editor, EditorElement, EditorStyle};

use gpui::{
    AsyncWindowContext, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, ScrollHandle,
    Subscription, Task, TextStyle, TextStyleRefinement, UnderlineStyle, WeakEntity, prelude::*,
};
use language::{Language, LanguageRegistry};
use markdown::{Markdown, MarkdownElement, MarkdownStyle};
use notifications::status_toast::StatusToast;
use parking_lot::Mutex;
use project::{
    context_server_store::{
        ContextServerStatus, ContextServerStore, ServerStatusChangedEvent,
        registry::ContextServerDescriptorRegistry,
    },
    project_settings::{ContextServerSettings, ProjectSettings},
    worktree_store::WorktreeStore,
};
use serde::Deserialize;
use settings::{Settings as _, update_settings_file};
use std::sync::Arc;
use theme_settings::ThemeSettings;
use ui::{
    CommonAnimationExt, KeyBinding, Modal, ModalFooter, ModalHeader, Section, Tooltip,
    WithScrollbar, prelude::*,
};
use util::ResultExt as _;
use workspace::{ModalView, Workspace};

use crate::AddContextServer;

/// 配置目标类型
enum ConfigurationTarget {
    /// 新建服务器
    New,
    /// 现有标准服务器
    Existing {
        id: ContextServerId,
        command: ContextServerCommand,
    },
    /// 现有HTTP服务器
    ExistingHttp {
        id: ContextServerId,
        url: String,
        headers: HashMap<String, String>,
    },
    /// 扩展服务器
    Extension {
        id: ContextServerId,
        repository_url: Option<SharedString>,
        installation: Option<extension::ContextServerConfiguration>,
    },
}

/// 配置来源
enum ConfigurationSource {
    New {
        editor: Entity<Editor>,
        is_http: bool,
    },
    Existing {
        editor: Entity<Editor>,
        is_http: bool,
    },
    Extension {
        id: ContextServerId,
        editor: Option<Entity<Editor>>,
        repository_url: Option<SharedString>,
        installation_instructions: Option<Entity<markdown::Markdown>>,
        settings_validator: Option<jsonschema::Validator>,
    },
}

impl ConfigurationSource {
    /// 是否存在配置选项
    fn has_configuration_options(&self) -> bool {
        !matches!(self, ConfigurationSource::Extension { editor: None, .. })
    }

    /// 是否为新建配置
    fn is_new(&self) -> bool {
        matches!(self, ConfigurationSource::New { .. })
    }

    /// 从配置目标创建配置来源
    fn from_target(
        target: ConfigurationTarget,
        language_registry: Arc<LanguageRegistry>,
        jsonc_language: Option<Arc<Language>>,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        fn create_editor(
            json: String,
            jsonc_language: Option<Arc<Language>>,
            window: &mut Window,
            cx: &mut App,
        ) -> Entity<Editor> {
            cx.new(|cx| {
                let mut editor = Editor::auto_height(4, 16, window, cx);
                editor.set_text(json, window, cx);
                editor.set_show_gutter(false, cx);
                editor.set_soft_wrap_mode(language::language_settings::SoftWrap::None, cx);
                if let Some(buffer) = editor.buffer().read(cx).as_singleton() {
                    buffer.update(cx ,|buffer, cx| buffer.set_language(jsonc_language, cx))
                }
                editor
            })
        }

        match target {
            ConfigurationTarget::New => ConfigurationSource::New {
                editor: create_editor(context_server_input(None), jsonc_language, window, cx),
                is_http: false,
            },
            ConfigurationTarget::Existing { id, command } => ConfigurationSource::Existing {
                editor: create_editor(
                    context_server_input(Some((id, command))),
                    jsonc_language,
                    window,
                    cx,
                ),
                is_http: false,
            },
            ConfigurationTarget::ExistingHttp {
                id,
                url,
                headers: auth,
            } => ConfigurationSource::Existing {
                editor: create_editor(
                    context_server_http_input(Some((id, url, auth))),
                    jsonc_language,
                    window,
                    cx,
                ),
                is_http: true,
            },
            ConfigurationTarget::Extension {
                id,
                repository_url,
                installation,
            } => {
                let settings_validator = installation.as_ref().and_then(|installation| {
                    jsonschema::validator_for(&installation.settings_schema)
                        .context("Failed to load JSON schema for context server settings")
                        .log_err()
                });
                let installation_instructions = installation.as_ref().map(|installation| {
                    cx.new(|cx| {
                        Markdown::new(
                            installation.installation_instructions.clone().into(),
                            Some(language_registry.clone()),
                            None,
                            cx,
                        )
                    })
                });
                ConfigurationSource::Extension {
                    id,
                    repository_url,
                    installation_instructions,
                    settings_validator,
                    editor: installation.map(|installation| {
                        create_editor(installation.default_settings, jsonc_language, window, cx)
                    }),
                }
            }
        }
    }

    /// 获取配置输出结果
    fn output(&self, cx: &mut App) -> Result<(ContextServerId, ContextServerSettings)> {
        match self {
            ConfigurationSource::New { editor, is_http }
            | ConfigurationSource::Existing { editor, is_http } => {
                if *is_http {
                    parse_http_input(&editor.read(cx).text(cx)).map(|(id, url, auth)| {
                        (
                            id,
                            ContextServerSettings::Http {
                                enabled: true,
                                url,
                                headers: auth,
                                timeout: None,
                            },
                        )
                    })
                } else {
                    parse_input(&editor.read(cx).text(cx)).map(|(id, command)| {
                        (
                            id,
                            ContextServerSettings::Stdio {
                                enabled: true,
                                remote: false,
                                command,
                            },
                        )
                    })
                }
            }
            ConfigurationSource::Extension {
                id,
                editor,
                settings_validator,
                ..
            } => {
                let text = editor
                    .as_ref()
                    .context("No output available")?
                    .read(cx)
                    .text(cx);
                let settings = serde_json_lenient::from_str::<serde_json::Value>(&text)?;
                if let Some(settings_validator) = settings_validator
                    && let Err(error) = settings_validator.validate(&settings)
                {
                    return Err(anyhow::anyhow!(error.to_string()));
                }
                Ok((
                    id.clone(),
                    ContextServerSettings::Extension {
                        enabled: true,
                        remote: false,
                        settings,
                    },
                ))
            }
        }
    }
}

/// 生成标准服务器配置输入模板
fn context_server_input(existing: Option<(ContextServerId, ContextServerCommand)>) -> String {
    let (name, command, args, env) = match existing {
        Some((id, cmd)) => {
            let args = serde_json::to_string(&cmd.args).unwrap();
            let env = serde_json::to_string(&cmd.env.unwrap_or_default()).unwrap();
            let cmd_path = serde_json::to_string(&cmd.path).unwrap();
            (id.0.to_string(), cmd_path, args, env)
        }
        None => (
            "some-mcp-server".to_string(),
            "".to_string(),
            "[]".to_string(),
            "{}".to_string(),
        ),
    };

    format!(
        r#"{{
  /// 配置通过标准输入输出本地运行的MCP服务器
  ///
  /// MCP服务器名称
  "{name}": {{
    /// 运行MCP服务器的命令
    "command": {command},
    /// 传递给MCP服务器的参数
    "args": {args},
    /// 要设置的环境变量
    "env": {env}
  }}
}}"#
    )
}

/// 生成HTTP服务器配置输入模板
fn context_server_http_input(
    existing: Option<(ContextServerId, String, HashMap<String, String>)>,
) -> String {
    let (name, url, headers) = match existing {
        Some((id, url, headers)) => {
            let header = if headers.is_empty() {
                r#"// "Authorization": "Bearer <token>"#.to_string()
            } else {
                let json = serde_json::to_string_pretty(&headers).unwrap();
                let mut lines = json.split("\n").collect::<Vec<_>>();
                if lines.len() > 1 {
                    lines.remove(0);
                    lines.pop();
                }
                lines
                    .into_iter()
                    .map(|line| format!("  {}", line))
                    .collect::<String>()
            };
            (id.0.to_string(), url, header)
        }
        None => (
            "some-remote-server".to_string(),
            "https://example.com/mcp".to_string(),
            r#"// "Authorization": "Bearer <token>"#.to_string(),
        ),
    };

    format!(
        r#"{{
  /// 配置通过HTTP连接的MCP服务器
  ///
  /// 远程MCP服务器名称
  "{name}": {{
    /// 远程MCP服务器的URL
    "url": "{url}",
    "headers": {{
     /// 要发送的请求头
     {headers}
    }}
  }}
}}"#
    )
}

/// 解析HTTP服务器配置
fn parse_http_input(text: &str) -> Result<(ContextServerId, String, HashMap<String, String>)> {
    #[derive(Deserialize)]
    struct Temp {
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
    }
    let value: HashMap<String, Temp> = serde_json_lenient::from_str(text)?;
    if value.len() != 1 {
        anyhow::bail!("Expected exactly one context server configuration");
    }

    let (key, value) = value.into_iter().next().unwrap();

    Ok((ContextServerId(key.into()), value.url, value.headers))
}

/// 解析扩展服务器配置
fn resolve_context_server_extension(
    id: ContextServerId,
    worktree_store: Entity<WorktreeStore>,
    cx: &mut App,
) -> Task<Option<ConfigurationTarget>> {
    let registry = ContextServerDescriptorRegistry::default_global(cx).read(cx);

    let Some(descriptor) = registry.context_server_descriptor(&id.0) else {
        return Task::ready(None);
    };

    let extension = crate::agent_configuration::resolve_extension_for_context_server(&id, cx);
    cx.spawn(async move |cx| {
        let installation = descriptor
            .configuration(worktree_store, cx)
            .await
            .context("Failed to resolve context server configuration")
            .log_err()
            .flatten();

        Some(ConfigurationTarget::Extension {
            id,
            repository_url: extension
                .and_then(|(_, manifest)| manifest.repository.clone().map(SharedString::from)),
            installation,
        })
    })
}

/// 弹窗状态
enum State {
    Idle,
    Waiting,
    AuthRequired { server_id: ContextServerId },
    Authenticating { _server_id: ContextServerId },
    Error(SharedString),
}

/// MCP服务器配置弹窗
pub struct ConfigureContextServerModal {
    context_server_store: Entity<ContextServerStore>,
    workspace: WeakEntity<Workspace>,
    source: ConfigurationSource,
    state: State,
    original_server_id: Option<ContextServerId>,
    scroll_handle: ScrollHandle,
    _auth_subscription: Option<Subscription>,
}

impl ConfigureContextServerModal {
    /// 注册弹窗
    pub fn register(
        workspace: &mut Workspace,
        language_registry: Arc<LanguageRegistry>,
        _window: Option<&mut Window>,
        _cx: &mut Context<Workspace>,
    ) {
        workspace.register_action({
            move |_workspace, _: &AddContextServer, window, cx| {
                let workspace_handle = cx.weak_entity();
                let language_registry = language_registry.clone();
                window
                    .spawn(cx, async move |cx| {
                        Self::show_modal(
                            ConfigurationTarget::New,
                            language_registry,
                            workspace_handle,
                            cx,
                        )
                        .await
                    })
                    .detach_and_log_err(cx);
            }
        });
    }

    /// 为现有服务器显示配置弹窗
    pub fn show_modal_for_existing_server(
        server_id: ContextServerId,
        language_registry: Arc<LanguageRegistry>,
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<()>> {
        let Some(settings) = ProjectSettings::get_global(cx)
            .context_servers
            .get(&server_id.0)
            .cloned()
            .or_else(|| {
                ContextServerDescriptorRegistry::default_global(cx)
                    .read(cx)
                    .context_server_descriptor(&server_id.0)
                    .map(|_| ContextServerSettings::default_extension())
            })
        else {
            return Task::ready(Err(anyhow::anyhow!("Context server not found")));
        };

        window.spawn(cx, async move |cx| {
            let target = match settings {
                ContextServerSettings::Stdio {
                    enabled: _,
                    command,
                    ..
                } => Some(ConfigurationTarget::Existing {
                    id: server_id,
                    command,
                }),
                ContextServerSettings::Http {
                    enabled: _,
                    url,
                    headers,
                    timeout: _,
                    ..
                } => Some(ConfigurationTarget::ExistingHttp {
                    id: server_id,
                    url,
                    headers,
                }),
                ContextServerSettings::Extension { .. } => {
                    match workspace
                        .update(cx, |workspace, cx| {
                            resolve_context_server_extension(
                                server_id,
                                workspace.project().read(cx).worktree_store(),
                                cx,
                            )
                        })
                        .ok()
                    {
                        Some(task) => task.await,
                        None => None,
                    }
                }
            };

            match target {
                Some(target) => Self::show_modal(target, language_registry, workspace, cx).await,
                None => Err(anyhow::anyhow!("Failed to resolve context server")),
            }
        })
    }

    /// 显示配置弹窗
    fn show_modal(
        target: ConfigurationTarget,
        language_registry: Arc<LanguageRegistry>,
        workspace: WeakEntity<Workspace>,
        cx: &mut AsyncWindowContext,
    ) -> Task<Result<()>> {
        cx.spawn(async move |cx| {
            let jsonc_language = language_registry.language_for_name("jsonc").await.ok();
            workspace.update_in(cx, |workspace, window, cx| {
                let workspace_handle = cx.weak_entity();
                let context_server_store = workspace.project().read(cx).context_server_store();
                workspace.toggle_modal(window, cx, |window, cx| Self {
                    context_server_store,
                    workspace: workspace_handle,
                    state: State::Idle,
                    original_server_id: match &target {
                        ConfigurationTarget::Existing { id, .. } => Some(id.clone()),
                        ConfigurationTarget::ExistingHttp { id, .. } => Some(id.clone()),
                        ConfigurationTarget::Extension { id, .. } => Some(id.clone()),
                        ConfigurationTarget::New => None,
                    },
                    source: ConfigurationSource::from_target(
                        target,
                        language_registry,
                        jsonc_language,
                        window,
                        cx,
                    ),
                    scroll_handle: ScrollHandle::new(),
                    _auth_subscription: None,
                })
            })
        })
    }

    /// 设置错误状态
    fn set_error(&mut self, err: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.state = State::Error(err.into());
        cx.notify();
    }

    /// 确认配置
    fn confirm(&mut self, _: &menu::Confirm, cx: &mut Context<Self>) {
        if matches!(
            self.state,
            State::Waiting | State::AuthRequired { .. } | State::Authenticating { .. }
        ) {
            return;
        }

        self.state = State::Idle;
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };

        let (id, settings) = match self.source.output(cx) {
            Ok(val) => val,
            Err(error) => {
                self.set_error(error.to_string(), cx);
                return;
            }
        };

        self.state = State::Waiting;

        let existing_server = self.context_server_store.read(cx).get_running_server(&id);
        if existing_server.is_some() {
            self.context_server_store.update(cx, |store, cx| {
                store.stop_server(&id, cx).log_err();
            });
        }

        let wait_for_context_server_task =
            wait_for_context_server(&self.context_server_store, id.clone(), cx);
        cx.spawn({
            let id = id.clone();
            async move |this, cx| {
                let result = wait_for_context_server_task.await;
                this.update(cx, |this, cx| match result {
                    Ok(ContextServerStatus::Running) => {
                        this.state = State::Idle;
                        this.show_configured_context_server_toast(id, cx);
                        cx.emit(DismissEvent);
                    }
                    Ok(ContextServerStatus::AuthRequired) => {
                        this.state = State::AuthRequired { server_id: id };
                        cx.notify();
                    }
                    Err(err) => {
                        this.set_error(err, cx);
                    }
                    Ok(_) => {}
                })
            }
        })
        .detach();

        let settings_changed =
            ProjectSettings::get_global(cx).context_servers.get(&id.0) != Some(&settings);

        if settings_changed {
            // 写入配置文件时会自动重启服务器
            workspace.update(cx, |workspace, cx| {
                let fs = workspace.app_state().fs.clone();
                let original_server_id = self.original_server_id.clone();
                update_settings_file(fs.clone(), cx, move |current, _| {
                    if let Some(original_id) = original_server_id {
                        if original_id != id {
                            current.project.context_servers.remove(&original_id.0);
                        }
                    }
                    current
                        .project
                        .context_servers
                        .insert(id.0, settings.into());
                });
            });
        } else if let Some(existing_server) = existing_server {
            self.context_server_store
                .update(cx, |store, cx| store.start_server(existing_server, cx));
        }
    }

    /// 取消配置
    fn cancel(&mut self, _: &menu::Cancel, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }

    /// 服务器认证
    fn authenticate(&mut self, server_id: ContextServerId, cx: &mut Context<Self>) {
        self.context_server_store.update(cx, |store, cx| {
            store.authenticate_server(&server_id, cx).log_err();
        });

        self.state = State::Authenticating {
            _server_id: server_id.clone(),
        };

        self._auth_subscription = Some(cx.subscribe(
            &self.context_server_store,
            move |this, _, event: &ServerStatusChangedEvent, cx| {
                if event.server_id != server_id {
                    return;
                }
                match &event.status {
                    ContextServerStatus::Running => {
                        this._auth_subscription = None;
                        this.state = State::Idle;
                        this.show_configured_context_server_toast(event.server_id.clone(), cx);
                        cx.emit(DismissEvent);
                    }
                    ContextServerStatus::AuthRequired => {
                        this._auth_subscription = None;
                        this.state = State::AuthRequired {
                            server_id: event.server_id.clone(),
                        };
                        cx.notify();
                    }
                    ContextServerStatus::Error(error) => {
                        this._auth_subscription = None;
                        this.set_error(error.clone(), cx);
                    }
                    ContextServerStatus::Authenticating
                    | ContextServerStatus::Starting
                    | ContextServerStatus::Stopped => {}
                }
            },
        ));

        cx.notify();
    }

    /// 显示配置成功提示
    fn show_configured_context_server_toast(&self, id: ContextServerId, cx: &mut App) {
        self.workspace
            .update(cx, {
                |workspace, cx| {
                    let status_toast = StatusToast::new(
                        format!("{} 配置成功。", id.0),
                        cx,
                        |this, _cx| {
                            this.icon(
                                Icon::new(IconName::ToolHammer)
                                    .size(IconSize::Small)
                                    .color(Color::Muted),
                            )
                            .action("关闭", |_, _| {})
                        },
                    );

                    workspace.toggle_status_toast(status_toast, cx);
                }
            })
            .log_err();
    }
}

/// 解析标准服务器配置
fn parse_input(text: &str) -> Result<(ContextServerId, ContextServerCommand)> {
    let value: serde_json::Value = serde_json_lenient::from_str(text)?;
    let object = value.as_object().context("Expected object")?;
    anyhow::ensure!(object.len() == 1, "Expected exactly one key-value pair");
    let (context_server_name, value) = object.into_iter().next().unwrap();
    let command: ContextServerCommand = serde_json::from_value(value.clone())?;
    Ok((ContextServerId(context_server_name.clone().into()), command))
}

impl ModalView for ConfigureContextServerModal {}

impl Focusable for ConfigureContextServerModal {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.source {
            ConfigurationSource::New { editor, .. } => editor.focus_handle(cx),
            ConfigurationSource::Existing { editor, .. } => editor.focus_handle(cx),
            ConfigurationSource::Extension { editor, .. } => editor
                .as_ref()
                .map(|editor| editor.focus_handle(cx))
                .unwrap_or_else(|| cx.focus_handle()),
        }
    }
}

impl EventEmitter<DismissEvent> for ConfigureContextServerModal {}

impl ConfigureContextServerModal {
    /// 渲染弹窗头部
    fn render_modal_header(&self) -> ModalHeader {
        let text: SharedString = match &self.source {
            ConfigurationSource::New { .. } => "添加MCP服务器".into(),
            ConfigurationSource::Existing { .. } => "配置MCP服务器".into(),
            ConfigurationSource::Extension { id, .. } => format!("配置 {}", id.0).into(),
        };
        ModalHeader::new().headline(text)
    }

    /// 渲染弹窗描述
    fn render_modal_description(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        const MODAL_DESCRIPTION: &str =
            "查看服务器文档了解所需的参数和环境变量。";

        if let ConfigurationSource::Extension {
            installation_instructions: Some(installation_instructions),
            ..
        } = &self.source
        {
            div()
                .pb_2()
                .text_sm()
                .child(MarkdownElement::new(
                    installation_instructions.clone(),
                    default_markdown_style(window, cx),
                ))
                .into_any_element()
        } else {
            Label::new(MODAL_DESCRIPTION)
                .color(Color::Muted)
                .into_any_element()
        }
    }

    /// 渲染标签栏
    fn render_tab_bar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let is_http = match &self.source {
            ConfigurationSource::New { is_http, .. } => *is_http,
            _ => return None,
        };

        let tab = |label: &'static str, active: bool| {
            div()
                .id(label)
                .cursor_pointer()
                .p_1()
                .text_sm()
                .border_b_1()
                .when(active, |this| {
                    this.border_color(cx.theme().colors().border_focused)
                })
                .when(!active, |this| {
                    this.border_color(gpui::transparent_black())
                        .text_color(cx.theme().colors().text_muted)
                        .hover(|s| s.text_color(cx.theme().colors().text))
                })
                .child(label)
        };

        Some(
            h_flex()
                .pt_1()
                .mb_2p5()
                .gap_1()
                .border_b_1()
                .border_color(cx.theme().colors().border.opacity(0.5))
                .child(
                    tab("本地", !is_http).on_click(cx.listener(|this, _, window, cx| {
                        if let ConfigurationSource::New { editor, is_http } = &mut this.source {
                            if *is_http {
                                *is_http = false;
                                let new_text = context_server_input(None);
                                editor.update(cx, |editor, cx| {
                                    editor.set_text(new_text, window, cx);
                                });
                            }
                        }
                    })),
                )
                .child(
                    tab("远程", is_http).on_click(cx.listener(|this, _, window, cx| {
                        if let ConfigurationSource::New { editor, is_http } = &mut this.source {
                            if !*is_http {
                                *is_http = true;
                                let new_text = context_server_http_input(None);
                                editor.update(cx, |editor, cx| {
                                    editor.set_text(new_text, window, cx);
                                });
                            }
                        }
                    })),
                )
                .into_any_element(),
        )
    }

    /// 渲染弹窗内容
    fn render_modal_content(&self, cx: &App) -> AnyElement {
        let editor = match &self.source {
            ConfigurationSource::New { editor, .. } => editor,
            ConfigurationSource::Existing { editor, .. } => editor,
            ConfigurationSource::Extension { editor, .. } => {
                let Some(editor) = editor else {
                    return div().into_any_element();
                };
                editor
            }
        };

        div()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(cx.theme().colors().border_variant)
            .bg(cx.theme().colors().editor_background)
            .child({
                let settings = ThemeSettings::get_global(cx);
                let text_style = TextStyle {
                    color: cx.theme().colors().text,
                    font_family: settings.buffer_font.family.clone(),
                    font_fallbacks: settings.buffer_font.fallbacks.clone(),
                    font_size: settings.buffer_font_size(cx).into(),
                    font_weight: settings.buffer_font.weight,
                    line_height: relative(settings.buffer_line_height.value()),
                    ..Default::default()
                };
                EditorElement::new(
                    editor,
                    EditorStyle {
                        background: cx.theme().colors().editor_background,
                        local_player: cx.theme().players().local(),
                        text: text_style,
                        syntax: cx.theme().syntax().clone(),
                        ..Default::default()
                    },
                )
            })
            .into_any_element()
    }

    /// 渲染弹窗底部
    fn render_modal_footer(&self, cx: &mut Context<Self>) -> ModalFooter {
        let focus_handle = self.focus_handle(cx);
        let is_busy = matches!(
            self.state,
            State::Waiting | State::AuthRequired { .. } | State::Authenticating { .. }
        );

        ModalFooter::new()
            .start_slot::<Button>(
                if let ConfigurationSource::Extension {
                    repository_url: Some(repository_url),
                    ..
                } = &self.source
                {
                    Some(
                        Button::new("open-repository", "打开仓库")
                            .end_icon(
                                Icon::new(IconName::ArrowUpRight)
                                    .size(IconSize::Small)
                                    .color(Color::Muted),
                            )
                            .tooltip({
                                let repository_url = repository_url.clone();
                                move |_window, cx| {
                                    Tooltip::with_meta(
                                        "打开仓库",
                                        None,
                                        repository_url.clone(),
                                        cx,
                                    )
                                }
                            })
                            .on_click({
                                let repository_url = repository_url.clone();
                                move |_, _, cx| cx.open_url(&repository_url)
                            }),
                    )
                } else {
                    None
                },
            )
            .end_slot(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new(
                            "cancel",
                            if self.source.has_configuration_options() {
                                "取消"
                            } else {
                                "关闭"
                            },
                        )
                        .key_binding(
                            KeyBinding::for_action_in(&menu::Cancel, &focus_handle, cx)
                                .map(|kb| kb.size(rems_from_px(12.))),
                        )
                        .on_click(
                            cx.listener(|this, _event, _window, cx| this.cancel(&menu::Cancel, cx)),
                        ),
                    )
                    .children(self.source.has_configuration_options().then(|| {
                        Button::new(
                            "add-server",
                            if self.source.is_new() {
                                    "添加服务器"
                                } else {
                                    "配置服务器"
                                },
                        )
                        .disabled(is_busy)
                        .key_binding(
                            KeyBinding::for_action_in(&menu::Confirm, &focus_handle, cx)
                                .map(|kb| kb.size(rems_from_px(12.))),
                        )
                        .on_click(
                            cx.listener(|this, _event, _window, cx| {
                                this.confirm(&menu::Confirm, cx)
                            }),
                        )
                    })),
            )
    }

    /// 渲染加载状态
    fn render_loading(&self, label: impl Into<SharedString>) -> Div {
        h_flex()
            .h_8()
            .gap_1p5()
            .justify_center()
            .child(
                Icon::new(IconName::LoadCircle)
                    .size(IconSize::XSmall)
                    .color(Color::Muted)
                    .with_rotate_animation(3),
            )
            .child(Label::new(label).size(LabelSize::Small).color(Color::Muted))
    }

    /// 渲染认证状态
    fn render_auth_required(&self, server_id: &ContextServerId, cx: &mut Context<Self>) -> Div {
        h_flex()
            .h_8()
            .min_w_0()
            .w_full()
            .gap_2()
            .justify_center()
            .child(
                h_flex()
                    .gap_1p5()
                    .child(
                        Icon::new(IconName::Info)
                            .size(IconSize::Small)
                            .color(Color::Muted),
                    )
                    .child(
                        Label::new("需要认证才能连接此服务器")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
            )
            .child(
                Button::new("authenticate-server", "认证")
                    .style(ButtonStyle::Outlined)
                    .label_size(LabelSize::Small)
                    .on_click({
                        let server_id = server_id.clone();
                        cx.listener(move |this, _event, _window, cx| {
                            this.authenticate(server_id.clone(), cx);
                        })
                    }),
            )
    }

    /// 渲染错误状态
    fn render_modal_error(error: SharedString) -> Div {
        h_flex()
            .h_8()
            .gap_1p5()
            .justify_center()
            .child(
                Icon::new(IconName::Warning)
                    .size(IconSize::Small)
                    .color(Color::Warning),
            )
            .child(
                div()
                    .w_full()
                    .child(Label::new(error).size(LabelSize::Small).color(Color::Muted)),
            )
    }
}

impl Render for ConfigureContextServerModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .elevation_3(cx)
            .w(rems(40.))
            .key_context("ConfigureContextServerModal")
            .on_action(
                cx.listener(|this, _: &menu::Cancel, _window, cx| this.cancel(&menu::Cancel, cx)),
            )
            .on_action(
                cx.listener(|this, _: &menu::Confirm, _window, cx| {
                    this.confirm(&menu::Confirm, cx)
                }),
            )
            .capture_any_mouse_down(cx.listener(|this, _, window, cx| {
                this.focus_handle(cx).focus(window, cx);
            }))
            .child(
                Modal::new("configure-context-server", None)
                    .header(self.render_modal_header())
                    .section(
                        Section::new().child(
                            div()
                                .size_full()
                                .child(
                                    div()
                                        .id("modal-content")
                                        .max_h(vh(0.7, window))
                                        .overflow_y_scroll()
                                        .track_scroll(&self.scroll_handle)
                                        .child(self.render_modal_description(window, cx))
                                        .children(self.render_tab_bar(cx))
                                        .child(self.render_modal_content(cx))
                                        .child(match &self.state {
                                            State::Idle => div(),
                                            State::Waiting => {
                                                self.render_loading("正在连接服务器…")
                                            }
                                            State::AuthRequired { server_id } => {
                                                self.render_auth_required(&server_id.clone(), cx)
                                            }
                                            State::Authenticating { .. } => {
                                                self.render_loading("正在认证…")
                                            }
                                            State::Error(error) => {
                                                Self::render_modal_error(error.clone())
                                            }
                                        }),
                                )
                                .vertical_scrollbar_for(&self.scroll_handle, window, cx),
                        ),
                    )
                    .footer(self.render_modal_footer(cx)),
            )
    }
}

/// 等待服务器启动
fn wait_for_context_server(
    context_server_store: &Entity<ContextServerStore>,
    context_server_id: ContextServerId,
    cx: &mut App,
) -> Task<Result<ContextServerStatus, Arc<str>>> {
    use std::time::Duration;

    const WAIT_TIMEOUT: Duration = Duration::from_secs(120);

    let (tx, rx) = futures::channel::oneshot::channel();
    let tx = Arc::new(Mutex::new(Some(tx)));

    let context_server_id_for_timeout = context_server_id.clone();
    let subscription = cx.subscribe(context_server_store, move |_, event, _cx| {
        let ServerStatusChangedEvent { server_id, status } = event;

        if server_id != &context_server_id {
            return;
        }

        match status {
            ContextServerStatus::Running | ContextServerStatus::AuthRequired => {
                if let Some(tx) = tx.lock().take() {
                    let _ = tx.send(Ok(status.clone()));
                }
            }
            ContextServerStatus::Stopped => {
                if let Some(tx) = tx.lock().take() {
                    let _ = tx.send(Err("Context server stopped running".into()));
                }
            }
            ContextServerStatus::Error(error) => {
                if let Some(tx) = tx.lock().take() {
                    let _ = tx.send(Err(error.clone()));
                }
            }
            ContextServerStatus::Starting | ContextServerStatus::Authenticating => {}
        }
    });

    cx.spawn(async move |cx| {
        let timeout = cx.background_executor().timer(WAIT_TIMEOUT);
        let result = futures::future::select(rx, timeout).await;
        drop(subscription);
        match result {
            futures::future::Either::Left((Ok(inner), _)) => inner,
            futures::future::Either::Left((Err(_), _)) => {
                Err(Arc::from("Context server store was dropped"))
            }
            futures::future::Either::Right(_) => Err(Arc::from(format!(
                "等待上下文服务器 `{}` 启动超时。查看Zed日志获取详细信息。",
                context_server_id_for_timeout
            ))),
        }
    })
}

/// 默认Markdown样式
pub(crate) fn default_markdown_style(window: &Window, cx: &App) -> MarkdownStyle {
    let theme_settings = ThemeSettings::get_global(cx);
    let colors = cx.theme().colors();
    let mut text_style = window.text_style();
    text_style.refine(&TextStyleRefinement {
        font_family: Some(theme_settings.ui_font.family.clone()),
        font_fallbacks: theme_settings.ui_font.fallbacks.clone(),
        font_features: Some(theme_settings.ui_font.features.clone()),
        font_size: Some(TextSize::XSmall.rems(cx).into()),
        color: Some(colors.text_muted),
        ..Default::default()
    });

    MarkdownStyle {
        base_text_style: text_style.clone(),
        selection_background_color: colors.element_selection_background,
        link: TextStyleRefinement {
            background_color: Some(colors.editor_foreground.opacity(0.025)),
            underline: Some(UnderlineStyle {
                color: Some(colors.text_accent.opacity(0.5)),
                thickness: px(1.),
                ..Default::default()
            }),
            ..Default::default()
        },
        ..Default::default()
    }
}