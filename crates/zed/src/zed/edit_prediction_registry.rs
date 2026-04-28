use client::{Client, UserStore};
use codestral::{CodestralEditPredictionDelegate, load_codestral_api_key};
use collections::HashMap;
use copilot::CopilotEditPredictionDelegate;
use edit_prediction::{EditPredictionModel, ZedEditPredictionDelegate};
use editor::Editor;
use gpui::{AnyWindowHandle, App, AppContext as _, Context, Entity, WeakEntity};
use language::language_settings::{EditPredictionProvider, all_language_settings};

use settings::{EditPredictionPromptFormat, SettingsStore};
use std::{cell::RefCell, rc::Rc, sync::Arc};
use ui::Window;

/// 初始化代码补全预测模块
pub fn init(client: Arc<Client>, user_store: Entity<UserStore>, cx: &mut App) {
    edit_prediction::EditPredictionStore::global(&client, &user_store, cx);

    let editors: Rc<RefCell<HashMap<WeakEntity<Editor>, AnyWindowHandle>>> = Rc::default();
    cx.observe_new({
        let editors = editors.clone();
        let client = client.clone();
        let user_store = user_store.clone();
        move |editor: &mut Editor, window, cx: &mut Context<Editor>| {
            if !editor.mode().is_full() {
                return;
            }

            register_backward_compatible_actions(editor, cx);

            let Some(window) = window else {
                return;
            };

            let editor_handle = cx.entity().downgrade();
            cx.on_release({
                let editor_handle = editor_handle.clone();
                let editors = editors.clone();
                move |_, _| {
                    editors.borrow_mut().remove(&editor_handle);
                }
            })
            .detach();

            editors
                .borrow_mut()
                .insert(editor_handle, window.window_handle());
            let provider_config = edit_prediction_provider_config_for_settings(cx);
            assign_edit_prediction_provider(
                editor,
                provider_config,
                &client,
                user_store.clone(),
                window,
                cx,
            );
        }
    })
    .detach();

    cx.on_action(clear_edit_prediction_store_edit_history);

    cx.subscribe(&user_store, {
        let editors = editors.clone();
        let client = client.clone();

        move |user_store, event, cx| match event {
            client::user::Event::PrivateUserInfoUpdated
            | client::user::Event::OrganizationChanged => {
                let provider_config = edit_prediction_provider_config_for_settings(cx);
                assign_edit_prediction_providers(
                    &editors,
                    provider_config,
                    &client,
                    user_store,
                    cx,
                );
            }
            _ => {}
        }
    })
    .detach();

    cx.observe_global::<SettingsStore>({
        let mut previous_config = edit_prediction_provider_config_for_settings(cx);
        move |cx| {
            let new_provider_config = edit_prediction_provider_config_for_settings(cx);

            if new_provider_config != previous_config {
                telemetry::event!(
                    "Edit Prediction Provider Changed",
                    from = previous_config.map(|config| config.name()),
                    to = new_provider_config.map(|config| config.name())
                );

                previous_config = new_provider_config;
                assign_edit_prediction_providers(
                    &editors,
                    new_provider_config,
                    &client,
                    user_store.clone(),
                    cx,
                );
            }
        }
    })
    .detach();
}

/// 根据设置获取代码补全预测服务配置
fn edit_prediction_provider_config_for_settings(cx: &App) -> Option<EditPredictionProviderConfig> {
    let settings = &all_language_settings(None, cx).edit_predictions;
    let provider = settings.provider;
    match provider {
        EditPredictionProvider::None => None,
        EditPredictionProvider::Copilot => Some(EditPredictionProviderConfig::Copilot),
        EditPredictionProvider::Zed => {
            Some(EditPredictionProviderConfig::Zed(EditPredictionModel::Zeta))
        }
        EditPredictionProvider::Codestral => Some(EditPredictionProviderConfig::Codestral),
        EditPredictionProvider::Ollama | EditPredictionProvider::OpenAiCompatibleApi => {
            let custom_settings = if provider == EditPredictionProvider::Ollama {
                settings.ollama.as_ref()?
            } else {
                settings.open_ai_compatible_api.as_ref()?
            };

            let mut format = custom_settings.prompt_format;
            if format == EditPredictionPromptFormat::Infer {
                if let Some(inferred_format) = infer_prompt_format(&custom_settings.model) {
                    format = inferred_format;
                } else {
                    // 待办：通知用户提示格式自动推断失败
                    return None;
                }
            }

            if matches!(
                format,
                EditPredictionPromptFormat::Zeta | EditPredictionPromptFormat::Zeta2
            ) {
                Some(EditPredictionProviderConfig::Zed(EditPredictionModel::Zeta))
            } else {
                Some(EditPredictionProviderConfig::Zed(
                    EditPredictionModel::Fim { format },
                ))
            }
        }

        EditPredictionProvider::Mercury => Some(EditPredictionProviderConfig::Zed(
            EditPredictionModel::Mercury,
        )),
        EditPredictionProvider::Experimental(_) => None,
    }
}

/// 根据模型名称自动推断提示格式
fn infer_prompt_format(model: &str) -> Option<EditPredictionPromptFormat> {
    let model_base = model.split(':').next().unwrap_or(model);

    Some(match model_base {
        "codellama" | "code-llama" => EditPredictionPromptFormat::CodeLlama,
        "starcoder" | "starcoder2" | "starcoderbase" => EditPredictionPromptFormat::StarCoder,
        "deepseek-coder" | "deepseek-coder-v2" => EditPredictionPromptFormat::DeepseekCoder,
        "qwen2.5-coder" | "qwen-coder" | "qwen" => EditPredictionPromptFormat::Qwen,
        "codegemma" => EditPredictionPromptFormat::CodeGemma,
        "codestral" | "mistral" => EditPredictionPromptFormat::Codestral,
        "glm" | "glm-4" | "glm-4.5" => EditPredictionPromptFormat::Glm,
        _ => {
            return None;
        }
    })
}

/// 代码补全预测服务配置枚举
#[derive(Copy, Clone, PartialEq, Eq)]
enum EditPredictionProviderConfig {
    Copilot,
    Codestral,
    Zed(EditPredictionModel),
}

impl EditPredictionProviderConfig {
    /// 获取服务名称
    fn name(&self) -> &'static str {
        match self {
            EditPredictionProviderConfig::Copilot => "Copilot",
            EditPredictionProviderConfig::Codestral => "Codestral",
            EditPredictionProviderConfig::Zed(model) => match model {
                EditPredictionModel::Zeta => "Zeta",
                EditPredictionModel::Fim { .. } => "FIM",
                EditPredictionModel::Mercury => "Mercury",
            },
        }
    }
}

/// 清空代码补全预测存储的历史记录
fn clear_edit_prediction_store_edit_history(_: &edit_prediction::ClearHistory, cx: &mut App) {
    if let Some(ep_store) = edit_prediction::EditPredictionStore::try_global(cx) {
        ep_store.update(cx, |ep_store, _| ep_store.clear_history());
    }
}

/// 为所有编辑器分配代码补全预测服务
fn assign_edit_prediction_providers(
    editors: &Rc<RefCell<HashMap<WeakEntity<Editor>, AnyWindowHandle>>>,
    provider_config: Option<EditPredictionProviderConfig>,
    client: &Arc<Client>,
    user_store: Entity<UserStore>,
    cx: &mut App,
) {
    if provider_config == Some(EditPredictionProviderConfig::Codestral) {
        load_codestral_api_key(cx).detach();
    }
    for (editor, window) in editors.borrow().iter() {
        _ = window.update(cx, |_window, window, cx| {
            _ = editor.update(cx, |editor, cx| {
                assign_edit_prediction_provider(
                    editor,
                    provider_config,
                    client,
                    user_store.clone(),
                    window,
                    cx,
                );
            })
        });
    }
}

/// 注册兼容旧版的操作命令
fn register_backward_compatible_actions(editor: &mut Editor, cx: &mut Context<Editor>) {
    // 我们将部分命令重命名为非Copilot专用名称，为保证兼容性
    // 此处重新注册旧名称的命令，避免用户快捷键配置失效
    editor
        .register_action(cx.listener(
            |editor, _: &copilot::Suggest, window: &mut Window, cx: &mut Context<Editor>| {
                editor.show_edit_prediction(&Default::default(), window, cx);
            },
        ))
        .detach();
}

/// 为单个编辑器分配代码补全预测服务
fn assign_edit_prediction_provider(
    editor: &mut Editor,
    provider_config: Option<EditPredictionProviderConfig>,
    client: &Arc<Client>,
    user_store: Entity<UserStore>,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    // 待办：是否仅需要为单缓冲区收集数据？
    let singleton_buffer = editor.buffer().read(cx).as_singleton();

    match provider_config {
        None => {
            editor.set_edit_prediction_provider::<ZedEditPredictionDelegate>(None, window, cx);
        }
        Some(EditPredictionProviderConfig::Copilot) => {
            let ep_store = edit_prediction::EditPredictionStore::global(client, &user_store, cx);
            let Some(project) = editor.project().cloned() else {
                return;
            };
            let copilot =
                ep_store.update(cx, |this, cx| this.start_copilot_for_project(&project, cx));

            if let Some(copilot) = copilot {
                if let Some(buffer) = singleton_buffer
                    && buffer.read(cx).file().is_some()
                {
                    copilot.update(cx, |copilot, cx| {
                        copilot.register_buffer(&buffer, cx);
                    });
                }
                let provider = cx.new(|_| CopilotEditPredictionDelegate::new(copilot));
                editor.set_edit_prediction_provider(Some(provider), window, cx);
            }
        }
        Some(EditPredictionProviderConfig::Codestral) => {
            let http_client = client.http_client();
            let provider = cx.new(|_| CodestralEditPredictionDelegate::new(http_client));
            editor.set_edit_prediction_provider(Some(provider), window, cx);
        }
        Some(EditPredictionProviderConfig::Zed(model)) => {
            let ep_store = edit_prediction::EditPredictionStore::global(client, &user_store, cx);

            if let Some(organization_configuration) =
                user_store.read(cx).current_organization_configuration()
            {
                if !organization_configuration.edit_prediction.is_enabled {
                    editor.set_edit_prediction_provider::<ZedEditPredictionDelegate>(
                        None, window, cx,
                    );

                    return;
                }
            }

            if let Some(project) = editor.project() {
                ep_store.update(cx, |ep_store, cx| {
                    ep_store.set_edit_prediction_model(model);
                    if let Some(buffer) = &singleton_buffer {
                        ep_store.register_buffer(buffer, project, cx);
                    }
                });

                let provider = cx.new(|cx| {
                    ZedEditPredictionDelegate::new(
                        project.clone(),
                        singleton_buffer,
                        &client,
                        &user_store,
                        cx,
                    )
                });
                editor.set_edit_prediction_provider(Some(provider), window, cx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor::MultiBuffer;
    use gpui::{BorrowAppContext, TestAppContext};
    use settings::{EditPredictionProvider, SettingsStore};
    use workspace::AppState;

    #[gpui::test]
    async fn test_subscribe_uses_stale_provider_config_after_settings_change(
        cx: &mut TestAppContext,
    ) {
        let app_state = cx.update(|cx| {
            let app_state = AppState::test(cx);
            client::init(&app_state.client, cx);
            language_model::init(cx);
            client::RefreshLlmTokenListener::register(
                app_state.client.clone(),
                app_state.user_store.clone(),
                cx,
            );
            editor::init(cx);
            app_state
        });

        // 将默认服务设置为None，确保订阅闭包初始化时捕获None
        // 测试默认值为Zed/Zeta1，无项目编辑器中无操作，会掩盖该bug
        cx.update(|cx| {
            cx.update_global::<SettingsStore, _>(|store: &mut SettingsStore, cx| {
                store.update_user_settings(cx, |settings| {
                    settings.project.all_languages.edit_predictions =
                        Some(settings::EditPredictionSettingsContent {
                            provider: Some(EditPredictionProvider::None),
                            ..Default::default()
                        });
                });
            });
        });

        cx.update(|cx| {
            init(app_state.client.clone(), app_state.user_store.clone(), cx);
        });

        // 创建窗口中的编辑器，触发observe_new注册
        let editor = cx.add_window(|window, cx| {
            let buffer = cx.new(|_cx| MultiBuffer::new(language::Capability::ReadWrite));
            Editor::new(editor::EditorMode::full(), buffer, None, window, cx)
        });

        editor
            .update(cx, |editor, _window, _cx| {
                assert!(
                    editor.edit_prediction_provider().is_none(),
                    "设置为None时，编辑器初始应无补全服务"
                );
            })
            .unwrap();

        // 修改设置为Codestral，observe_global闭包更新服务配置并分配给所有编辑器
        cx.update(|cx| {
            cx.update_global::<SettingsStore, _>(|store: &mut SettingsStore, cx| {
                store.update_user_settings(cx, |settings| {
                    settings.project.all_languages.edit_predictions =
                        Some(settings::EditPredictionSettingsContent {
                            provider: Some(EditPredictionProvider::Codestral),
                            ..Default::default()
                        });
                });
            });
        });

        editor
            .update(cx, |editor, _window, _cx| {
                assert!(
                    editor.edit_prediction_provider().is_some(),
                    "设置修改为Codestral后，编辑器应启用补全服务"
                );
            })
            .unwrap();

        // 触发用户信息更新事件，订阅闭包应使用当前服务配置(Codestral)
        // 存在bug时会使用初始化时的旧值(None)并清空服务
        cx.update(|cx| {
            app_state.user_store.update(cx, |_, cx| {
                cx.emit(client::user::Event::PrivateUserInfoUpdated);
            });
        });
        cx.run_until_parked();

        editor
            .update(cx, |editor, _window, _cx| {
                assert!(
                    editor.edit_prediction_provider().is_some(),
                    "BUG：订阅闭包使用了过期的服务配置(None)，而非当前配置(Codestral)"
                );
            })
            .unwrap();
    }
}