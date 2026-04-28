use std::{
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use acp_thread::{AcpThread, AcpThreadEvent, MentionUri, ThreadStatus};
use agent::{ContextServerRegistry, SharedThread, ThreadStore};
use agent_client_protocol::schema as acp;
use agent_servers::AgentServer;
use collections::HashSet;
use db::kvp::{Dismissable, KeyValueStore};
use itertools::Itertools;
use project::AgentId;
use serde::{Deserialize, Serialize};
use settings::{LanguageModelProviderSetting, LanguageModelSelection};

use zed_actions::{
    DecreaseBufferFontSize, IncreaseBufferFontSize, ResetBufferFontSize,
    agent::{
        AddSelectionToThread, ConflictContent, OpenSettings, ReauthenticateAgent, ResetAgentZoom,
        ResetOnboarding, ResolveConflictedFilesWithAgent, ResolveConflictsWithAgent,
        ReviewBranchDiff,
    },
    assistant::{FocusAgent, OpenRulesLibrary, Toggle, ToggleFocus},
};

use crate::DEFAULT_THREAD_TITLE;
use crate::ExpandMessageEditor;
use crate::ManageProfiles;
use crate::agent_connection_store::AgentConnectionStore;
use crate::thread_metadata_store::{ThreadId, ThreadMetadataStore, ThreadMetadataStoreEvent};
use crate::{
    AddContextServer, AgentDiffPane, ConversationView, CopyThreadToClipboard, Follow,
    InlineAssistant, LoadThreadFromClipboard, NewThread, OpenActiveThreadAsMarkdown, OpenAgentDiff,
    ResetTrialEndUpsell, ResetTrialUpsell, ShowAllSidebarThreadMetadata, ShowThreadMetadata,
    ToggleNewThreadMenu, ToggleOptionsMenu,
    agent_configuration::{AgentConfiguration, AssistantConfigurationEvent},
    conversation_view::{AcpThreadViewEvent, ThreadView},
    ui::EndTrialUpsell,
};
use crate::{
    Agent, AgentInitialContent, ExternalSourcePrompt, NewExternalAgentThread,
    NewNativeAgentThreadFromSummary,
};
use agent_settings::AgentSettings;
use ai_onboarding::AgentPanelOnboarding;
use anyhow::Result;
use chrono::{DateTime, Utc};
use client::UserStore;
use cloud_api_types::Plan;
use collections::HashMap;
use editor::{Editor, MultiBuffer};
use extension::ExtensionEvents;
use extension_host::ExtensionStore;
use fs::Fs;
use gpui::{
    Action, Anchor, Animation, AnimationExt, AnyElement, App, AsyncWindowContext, ClipboardItem,
    Entity, EventEmitter, ExternalPaths, FocusHandle, Focusable, KeyContext, Pixels, Subscription,
    Task, UpdateGlobal, WeakEntity, prelude::*, pulsating_between,
};
use language::LanguageRegistry;
use language_model::LanguageModelRegistry;
use project::{Project, ProjectPath, Worktree};
use prompt_store::{PromptStore, UserPromptId};
use rules_library::{RulesLibrary, open_rules_library};
use settings::TerminalDockPosition;
use settings::{Settings, update_settings_file};
use terminal::terminal_settings::TerminalSettings;
use terminal_view::{TerminalView, terminal_panel::TerminalPanel};
use theme_settings::ThemeSettings;
use ui::{
    Button, Callout, ContextMenu, ContextMenuEntry, IconButton, PopoverMenu, PopoverMenuHandle,
    Tab, Tooltip, prelude::*, utils::WithRemSize,
};
use util::ResultExt as _;
use workspace::{
    CollaboratorId, DraggedSelection, DraggedTab, PathList, SerializedPathList,
    ToggleWorkspaceSidebar, ToggleZoom, Workspace, WorkspaceId,
    dock::{DockPosition, Panel, PanelEvent},
};

const AGENT_PANEL_KEY: &str = "agent_panel";
const MIN_PANEL_WIDTH: Pixels = px(300.);
const LAST_USED_AGENT_KEY: &str = "agent_panel__last_used_external_agent";

/// Maximum number of idle threads kept in the agent panel's retained list.
/// Set as a GPUI global to override; otherwise defaults to 5.
pub struct MaxIdleRetainedThreads(pub usize);
impl gpui::Global for MaxIdleRetainedThreads {}

impl MaxIdleRetainedThreads {
    pub fn global(cx: &App) -> usize {
        cx.try_global::<Self>().map_or(5, |g| g.0)
    }
}

#[derive(Serialize, Deserialize)]
struct LastUsedAgent {
    agent: Agent,
}

/// Reads the most recently used agent across all workspaces. Used as a fallback
/// when opening a workspace that has no per-workspace agent preference yet.
fn read_global_last_used_agent(kvp: &KeyValueStore) -> Option<Agent> {
    kvp.read_kvp(LAST_USED_AGENT_KEY)
        .log_err()
        .flatten()
        .and_then(|json| serde_json::from_str::<LastUsedAgent>(&json).log_err())
        .map(|entry| entry.agent)
}

async fn write_global_last_used_agent(kvp: KeyValueStore, agent: Agent) {
    if let Some(json) = serde_json::to_string(&LastUsedAgent { agent }).log_err() {
        kvp.write_kvp(LAST_USED_AGENT_KEY.to_string(), json)
            .await
            .log_err();
    }
}

fn read_serialized_panel(
    workspace_id: workspace::WorkspaceId,
    kvp: &KeyValueStore,
) -> Option<SerializedAgentPanel> {
    let scope = kvp.scoped(AGENT_PANEL_KEY);
    let key = i64::from(workspace_id).to_string();
    scope
        .read(&key)
        .log_err()
        .flatten()
        .and_then(|json| serde_json::from_str::<SerializedAgentPanel>(&json).log_err())
}

async fn save_serialized_panel(
    workspace_id: workspace::WorkspaceId,
    panel: SerializedAgentPanel,
    kvp: KeyValueStore,
) -> Result<()> {
    let scope = kvp.scoped(AGENT_PANEL_KEY);
    let key = i64::from(workspace_id).to_string();
    scope.write(key, serde_json::to_string(&panel)?).await?;
    Ok(())
}

/// Migration: reads the original single-panel format stored under the
/// `"agent_panel"` KVP key before per-workspace keying was introduced.
fn read_legacy_serialized_panel(kvp: &KeyValueStore) -> Option<SerializedAgentPanel> {
    kvp.read_kvp(AGENT_PANEL_KEY)
        .log_err()
        .flatten()
        .and_then(|json| serde_json::from_str::<SerializedAgentPanel>(&json).log_err())
}

#[derive(Serialize, Deserialize, Debug)]
struct SerializedAgentPanel {
    selected_agent: Option<Agent>,
    #[serde(default)]
    last_active_thread: Option<SerializedActiveThread>,
    draft_thread_prompt: Option<Vec<acp::ContentBlock>>,
}

#[derive(Serialize, Deserialize, Debug)]
struct SerializedActiveThread {
    session_id: Option<String>,
    agent_type: Agent,
    title: Option<String>,
    work_dirs: Option<SerializedPathList>,
}

pub fn init(cx: &mut App) {
    cx.observe_new(
        |workspace: &mut Workspace, _window, _cx: &mut Context<Workspace>| {
            workspace
                .register_action(|workspace, action: &NewThread, window, cx| {
                    if let Some(panel) = workspace.panel::<AgentPanel>(cx) {
                        panel.update(cx, |panel, cx| panel.new_thread(action, window, cx));
                        workspace.focus_panel::<AgentPanel>(window, cx);
                    }
                })
                .register_action(
                    |workspace, action: &NewNativeAgentThreadFromSummary, window, cx| {
                        if let Some(panel) = workspace.panel::<AgentPanel>(cx) {
                            panel.update(cx, |panel, cx| {
                                panel.new_native_agent_thread_from_summary(action, window, cx)
                            });
                            workspace.focus_panel::<AgentPanel>(window, cx);
                        }
                    },
                )
                .register_action(|workspace, _: &ExpandMessageEditor, window, cx| {
                    if let Some(panel) = workspace.panel::<AgentPanel>(cx) {
                        workspace.focus_panel::<AgentPanel>(window, cx);
                        panel.update(cx, |panel, cx| panel.expand_message_editor(window, cx));
                    }
                })
                .register_action(|workspace, _: &OpenSettings, window, cx| {
                    if let Some(panel) = workspace.panel::<AgentPanel>(cx) {
                        workspace.focus_panel::<AgentPanel>(window, cx);
                        panel.update(cx, |panel, cx| panel.open_configuration(window, cx));
                    }
                })
                .register_action(|workspace, action: &NewExternalAgentThread, window, cx| {
                    if let Some(panel) = workspace.panel::<AgentPanel>(cx) {
                        workspace.focus_panel::<AgentPanel>(window, cx);
                        panel.update(cx, |panel, cx| {
                            panel.new_external_agent_thread(action, window, cx);
                        });
                    }
                })
                .register_action(|workspace, action: &OpenRulesLibrary, window, cx| {
                    if let Some(panel) = workspace.panel::<AgentPanel>(cx) {
                        workspace.focus_panel::<AgentPanel>(window, cx);
                        panel.update(cx, |panel, cx| {
                            panel.deploy_rules_library(action, window, cx)
                        });
                    }
                })
                .register_action(|workspace, _: &Follow, window, cx| {
                    workspace.follow(CollaboratorId::Agent, window, cx);
                })
                .register_action(|workspace, _: &OpenAgentDiff, window, cx| {
                    let thread = workspace
                        .panel::<AgentPanel>(cx)
                        .and_then(|panel| panel.read(cx).active_conversation_view().cloned())
                        .and_then(|conversation| {
                            conversation
                                .read(cx)
                                .root_thread_view()
                                .map(|r| r.read(cx).thread.clone())
                        });

                    if let Some(thread) = thread {
                        AgentDiffPane::deploy_in_workspace(thread, workspace, window, cx);
                    }
                })
                .register_action(|workspace, _: &ToggleOptionsMenu, window, cx| {
                    if let Some(panel) = workspace.panel::<AgentPanel>(cx) {
                        workspace.focus_panel::<AgentPanel>(window, cx);
                        panel.update(cx, |panel, cx| {
                            panel.toggle_options_menu(&ToggleOptionsMenu, window, cx);
                        });
                    }
                })
                .register_action(|workspace, _: &ToggleNewThreadMenu, window, cx| {
                    if let Some(panel) = workspace.panel::<AgentPanel>(cx) {
                        workspace.focus_panel::<AgentPanel>(window, cx);
                        panel.update(cx, |panel, cx| {
                            panel.toggle_new_thread_menu(&ToggleNewThreadMenu, window, cx);
                        });
                    }
                })
                .register_action(|_workspace, _: &ResetOnboarding, window, cx| {
                    window.dispatch_action(workspace::RestoreBanner.boxed_clone(), cx);
                    window.refresh();
                })
                .register_action(|workspace, _: &ResetTrialUpsell, _window, cx| {
                    if let Some(panel) = workspace.panel::<AgentPanel>(cx) {
                        panel.update(cx, |panel, _| {
                            panel
                                .new_user_onboarding_upsell_dismissed
                                .store(false, Ordering::Release);
                        });
                    }
                    OnboardingUpsell::set_dismissed(false, cx);
                })
                .register_action(|_workspace, _: &ResetTrialEndUpsell, _window, cx| {
                    TrialEndUpsell::set_dismissed(false, cx);
                })
                .register_action(|workspace, _: &ResetAgentZoom, window, cx| {
                    if let Some(panel) = workspace.panel::<AgentPanel>(cx) {
                        panel.update(cx, |panel, cx| {
                            panel.reset_agent_zoom(window, cx);
                        });
                    }
                })
                .register_action(|workspace, _: &CopyThreadToClipboard, window, cx| {
                    if let Some(panel) = workspace.panel::<AgentPanel>(cx) {
                        panel.update(cx, |panel, cx| {
                            panel.copy_thread_to_clipboard(window, cx);
                        });
                    }
                })
                .register_action(|workspace, _: &LoadThreadFromClipboard, window, cx| {
                    if let Some(panel) = workspace.panel::<AgentPanel>(cx) {
                        workspace.focus_panel::<AgentPanel>(window, cx);
                        panel.update(cx, |panel, cx| {
                            panel.load_thread_from_clipboard(window, cx);
                        });
                    }
                })
                .register_action(|workspace, _: &ShowThreadMetadata, window, cx| {
                    if let Some(panel) = workspace.panel::<AgentPanel>(cx) {
                        panel.update(cx, |panel, cx| {
                            panel.show_thread_metadata(&ShowThreadMetadata, window, cx);
                        });
                    }
                })
                .register_action(|workspace, _: &ShowAllSidebarThreadMetadata, window, cx| {
                    if let Some(panel) = workspace.panel::<AgentPanel>(cx) {
                        panel.update(cx, |panel, cx| {
                            panel.show_all_sidebar_thread_metadata(
                                &ShowAllSidebarThreadMetadata,
                                window,
                                cx,
                            );
                        });
                    }
                })
                .register_action(|workspace, action: &ReviewBranchDiff, window, cx| {
                    let Some(panel) = workspace.panel::<AgentPanel>(cx) else {
                        return;
                    };

                    let mention_uri = MentionUri::GitDiff {
                        base_ref: action.base_ref.to_string(),
                    };
                    let diff_uri = mention_uri.to_uri().to_string();

                    let content_blocks = vec![
                        acp::ContentBlock::Text(acp::TextContent::new(
                        "请仔细查看此分支差异。指出你发现的任何问题、潜在漏洞或可优化的地方。\n\n"
                            .to_string(),
                    )),
                        acp::ContentBlock::Resource(acp::EmbeddedResource::new(
                            acp::EmbeddedResourceResource::TextResourceContents(
                                acp::TextResourceContents::new(
                                    action.diff_text.to_string(),
                                    diff_uri,
                                ),
                            ),
                        )),
                    ];

                    workspace.focus_panel::<AgentPanel>(window, cx);

                    panel.update(cx, |panel, cx| {
                        panel.external_thread(
                            None,
                            None,
                            None,
                            None,
                            Some(AgentInitialContent::ContentBlock {
                                blocks: content_blocks,
                                auto_submit: true,
                            }),
                            true,
                            "git_panel",
                            window,
                            cx,
                        );
                    });
                })
                .register_action(
                    |workspace, action: &ResolveConflictsWithAgent, window, cx| {
                        let Some(panel) = workspace.panel::<AgentPanel>(cx) else {
                            return;
                        };

                        let content_blocks = build_conflict_resolution_prompt(&action.conflicts);

                        workspace.focus_panel::<AgentPanel>(window, cx);

                        panel.update(cx, |panel, cx| {
                            panel.external_thread(
                                None,
                                None,
                                None,
                                None,
                                Some(AgentInitialContent::ContentBlock {
                                    blocks: content_blocks,
                                    auto_submit: true,
                                }),
                                true,
                                "git_panel",
                                window,
                                cx,
                            );
                        });
                    },
                )
                .register_action(
                    |workspace, action: &ResolveConflictedFilesWithAgent, window, cx| {
                        let Some(panel) = workspace.panel::<AgentPanel>(cx) else {
                            return;
                        };

                        let content_blocks =
                            build_conflicted_files_resolution_prompt(&action.conflicted_file_paths);

                        workspace.focus_panel::<AgentPanel>(window, cx);

                        panel.update(cx, |panel, cx| {
                            panel.external_thread(
                                None,
                                None,
                                None,
                                None,
                                Some(AgentInitialContent::ContentBlock {
                                    blocks: content_blocks,
                                    auto_submit: true,
                                }),
                                true,
                                "git_panel",
                                window,
                                cx,
                            );
                        });
                    },
                )
                .register_action(
                    |workspace: &mut Workspace, _: &AddSelectionToThread, window, cx| {
                        let active_editor = workspace
                            .active_item(cx)
                            .and_then(|item| item.act_as::<Editor>(cx));
                        let has_editor_selection = active_editor.is_some_and(|editor| {
                            editor.update(cx, |editor, cx| {
                                editor.has_non_empty_selection(&editor.display_snapshot(cx))
                            })
                        });

                        let has_terminal_selection = workspace
                            .active_item(cx)
                            .and_then(|item| item.act_as::<TerminalView>(cx))
                            .is_some_and(|terminal_view| {
                                terminal_view
                                    .read(cx)
                                    .terminal()
                                    .read(cx)
                                    .last_content
                                    .selection_text
                                    .as_ref()
                                    .is_some_and(|text| !text.is_empty())
                            });

                        let has_terminal_panel_selection =
                            workspace.panel::<TerminalPanel>(cx).is_some_and(|panel| {
                                let position = match TerminalSettings::get_global(cx).dock {
                                    TerminalDockPosition::Left => DockPosition::Left,
                                    TerminalDockPosition::Bottom => DockPosition::Bottom,
                                    TerminalDockPosition::Right => DockPosition::Right,
                                };
                                let dock_is_open =
                                    workspace.dock_at_position(position).read(cx).is_open();
                                dock_is_open && !panel.read(cx).terminal_selections(cx).is_empty()
                            });

                        if !has_editor_selection
                            && !has_terminal_selection
                            && !has_terminal_panel_selection
                        {
                            return;
                        }

                        let Some(panel) = workspace.panel::<AgentPanel>(cx) else {
                            return;
                        };

                        if !panel.focus_handle(cx).contains_focused(window, cx) {
                            workspace.toggle_panel_focus::<AgentPanel>(window, cx);
                        }

                        panel.update(cx, |_, cx| {
                            cx.defer_in(window, move |panel, window, cx| {
                                if let Some(conversation_view) = panel.active_conversation_view() {
                                    conversation_view.update(cx, |conversation_view, cx| {
                                        conversation_view.insert_selections(window, cx);
                                    });
                                }
                            });
                        });
                    },
                );
        },
    )
    .detach();
}

fn conflict_resource_block(conflict: &ConflictContent) -> acp::ContentBlock {
    let mention_uri = MentionUri::MergeConflict {
        file_path: conflict.file_path.clone(),
    };
    acp::ContentBlock::Resource(acp::EmbeddedResource::new(
        acp::EmbeddedResourceResource::TextResourceContents(acp::TextResourceContents::new(
            conflict.conflict_text.clone(),
            mention_uri.to_uri().to_string(),
        )),
    ))
}

fn build_conflict_resolution_prompt(conflicts: &[ConflictContent]) -> Vec<acp::ContentBlock> {
    if conflicts.is_empty() {
        return Vec::new();
    }

    let mut blocks = Vec::new();

    if conflicts.len() == 1 {
        let conflict = &conflicts[0];

        blocks.push(acp::ContentBlock::Text(acp::TextContent::new(
            "请解决以下文件中的合并冲突：",
        )));
        let mention = MentionUri::File {
            abs_path: PathBuf::from(conflict.file_path.clone()),
        };
        blocks.push(acp::ContentBlock::ResourceLink(acp::ResourceLink::new(
            mention.name(),
            mention.to_uri(),
        )));

        blocks.push(acp::ContentBlock::Text(acp::TextContent::new(
            indoc::formatdoc!(
                "\n冲突发生在分支 `{ours}`（当前分支）与 `{theirs}`（目标分支）之间。

                请仔细分析两个版本的代码，通过直接编辑文件来解决冲突。
                选择最能保留两处修改意图的解决方案，必要时将两者合理合并。

                ",
                ours = conflict.ours_branch_name,
                theirs = conflict.theirs_branch_name,
            ),
        )));
    } else {
        let n = conflicts.len();
        let unique_files: HashSet<&str> = conflicts.iter().map(|c| c.file_path.as_str()).collect();
        let ours = &conflicts[0].ours_branch_name;
        let theirs = &conflicts[0].theirs_branch_name;
        blocks.push(acp::ContentBlock::Text(acp::TextContent::new(
            indoc::formatdoc!(
                "请解决以下全部 {n} 处合并冲突。

                冲突发生在分支 `{ours}`（当前分支）与 `{theirs}`（目标分支）之间。

                针对每一处冲突，仔细分析两个版本的代码，通过直接编辑文件{suffix}来解决。
                选择最能保留两处修改意图的解决方案，必要时将两者合理合并。

                ",
                suffix = if unique_files.len() > 1 { "s" } else { "" },
            ),
        )));
    }

    for conflict in conflicts {
        blocks.push(conflict_resource_block(conflict));
    }

    blocks
}


fn build_conflicted_files_resolution_prompt(
    conflicted_file_paths: &[String],
) -> Vec<acp::ContentBlock> {
    if conflicted_file_paths.is_empty() {
        return Vec::new();
    }

    let instruction = indoc::indoc!(
        "以下文件存在未解决的合并冲突。请打开每个文件，找到冲突标记（`<<<<<<<` / `=======` / `>>>>>>>`），
        并通过直接编辑文件解决所有冲突。

        选择最能保留两处修改意图的解决方案，必要时将两者合理合并。

        存在冲突的文件：
        ",
    );

    let mut content = vec![acp::ContentBlock::Text(acp::TextContent::new(instruction))];
    for path in conflicted_file_paths {
        let mention = MentionUri::File {
            abs_path: PathBuf::from(path),
        };
        content.push(acp::ContentBlock::ResourceLink(acp::ResourceLink::new(
            mention.name(),
            mention.to_uri(),
        )));
        content.push(acp::ContentBlock::Text(acp::TextContent::new("\n")));
    }
    content
}

/// 格式化时间戳为人类易读格式
fn format_timestamp_human(dt: &DateTime<Utc>) -> String {
    let now = Utc::now();
    let duration = now.signed_duration_since(*dt);

    let relative = if duration.num_seconds() < 0 {
        "未来".to_string()
    } else if duration.num_seconds() < 60 {
        let seconds = duration.num_seconds();
        format!("{seconds} 秒前")
    } else if duration.num_minutes() < 60 {
        let minutes = duration.num_minutes();
        format!("{minutes} 分钟前")
    } else if duration.num_hours() < 24 {
        let hours = duration.num_hours();
        format!("{hours} 小时前")
    } else {
        let days = duration.num_days();
        format!("{days} 天前")
    };

    format!("{} ({})", dt.to_rfc3339(), relative)
}


/// Used for `dev: show thread metadata` action
fn thread_metadata_to_debug_json(
    metadata: &crate::thread_metadata_store::ThreadMetadata,
) -> serde_json::Value {
    serde_json::json!({
        "thread_id": metadata.thread_id,
        "session_id": metadata.session_id.as_ref().map(|s| s.0.to_string()),
        "agent_id": metadata.agent_id.0.to_string(),
        "title": metadata.title.as_ref().map(|t| t.to_string()),
        "updated_at": format_timestamp_human(&metadata.updated_at),
        "created_at": metadata.created_at.as_ref().map(format_timestamp_human),
        "interacted_at": metadata.interacted_at.as_ref().map(format_timestamp_human),
        "worktree_paths": format!("{:?}", metadata.worktree_paths),
        "archived": metadata.archived,
    })
}

pub(crate) struct AgentThread {
    conversation_view: Entity<ConversationView>,
}

enum BaseView {
    Uninitialized,
    AgentThread {
        conversation_view: Entity<ConversationView>,
    },
}

impl From<AgentThread> for BaseView {
    fn from(thread: AgentThread) -> Self {
        BaseView::AgentThread {
            conversation_view: thread.conversation_view,
        }
    }
}

enum OverlayView {
    Configuration,
}

enum VisibleSurface<'a> {
    Uninitialized,
    AgentThread(&'a Entity<ConversationView>),
    Configuration(Option<&'a Entity<AgentConfiguration>>),
}

enum WhichFontSize {
    AgentFont,
    None,
}

impl BaseView {
    pub fn which_font_size_used(&self) -> WhichFontSize {
        WhichFontSize::AgentFont
    }
}

impl OverlayView {
    pub fn which_font_size_used(&self) -> WhichFontSize {
        match self {
            OverlayView::Configuration => WhichFontSize::None,
        }
    }
}

pub struct AgentPanel {
    workspace: WeakEntity<Workspace>,
    /// Workspace id is used as a database key
    workspace_id: Option<WorkspaceId>,
    user_store: Entity<UserStore>,
    project: Entity<Project>,
    fs: Arc<dyn Fs>,
    language_registry: Arc<LanguageRegistry>,
    thread_store: Entity<ThreadStore>,
    prompt_store: Option<Entity<PromptStore>>,
    connection_store: Entity<AgentConnectionStore>,
    context_server_registry: Entity<ContextServerRegistry>,
    configuration: Option<Entity<AgentConfiguration>>,
    configuration_subscription: Option<Subscription>,
    focus_handle: FocusHandle,
    base_view: BaseView,
    overlay_view: Option<OverlayView>,
    draft_thread: Option<Entity<ConversationView>>,
    retained_threads: HashMap<ThreadId, Entity<ConversationView>>,
    new_thread_menu_handle: PopoverMenuHandle<ContextMenu>,
    agent_panel_menu_handle: PopoverMenuHandle<ContextMenu>,
    _extension_subscription: Option<Subscription>,
    _project_subscription: Subscription,
    zoomed: bool,
    pending_serialization: Option<Task<Result<()>>>,
    new_user_onboarding: Entity<AgentPanelOnboarding>,
    new_user_onboarding_upsell_dismissed: AtomicBool,
    selected_agent: Agent,
    _thread_view_subscription: Option<Subscription>,
    _active_thread_focus_subscription: Option<Subscription>,
    show_trust_workspace_message: bool,
    _base_view_observation: Option<Subscription>,
    _draft_editor_observation: Option<Subscription>,
    _thread_metadata_store_subscription: Subscription,
}

impl AgentPanel {
    fn serialize(&mut self, cx: &mut App) {
        let Some(workspace_id) = self.workspace_id else {
            return;
        };

        let selected_agent = self.selected_agent.clone();

        let is_draft_active = self.active_thread_is_draft(cx);
        let last_active_thread = self
            .active_agent_thread(cx)
            .map(|thread| {
                let thread = thread.read(cx);

                let title = thread.title();
                let work_dirs = thread.work_dirs().cloned();
                SerializedActiveThread {
                    session_id: (!is_draft_active).then(|| thread.session_id().0.to_string()),
                    agent_type: self.selected_agent.clone(),
                    title: title.map(|t| t.to_string()),
                    work_dirs: work_dirs.map(|dirs| dirs.serialize()),
                }
            })
            .or_else(|| {
                // The active view may be in `Loading` or `LoadError` — for
                // example, while a restored thread is waiting for a custom
                // agent to finish registering. Without this fallback, a
                // stray `serialize()` triggered during that window would
                // write `session_id=None` and wipe the restored session
                if is_draft_active {
                    return None;
                }
                let conversation_view = self.active_conversation_view()?;
                let session_id = conversation_view.read(cx).root_session_id.clone()?;
                let metadata = ThreadMetadataStore::try_global(cx)
                    .and_then(|store| store.read(cx).entry_by_session(&session_id).cloned());
                Some(SerializedActiveThread {
                    session_id: Some(session_id.0.to_string()),
                    agent_type: self.selected_agent.clone(),
                    title: metadata
                        .as_ref()
                        .and_then(|m| m.title.as_ref())
                        .map(|t| t.to_string()),
                    work_dirs: metadata.map(|m| m.folder_paths().serialize()),
                })
            });

        let kvp = KeyValueStore::global(cx);
        let draft_thread_prompt = self.draft_thread.as_ref().and_then(|conversation| {
            Some(
                conversation
                    .read(cx)
                    .root_thread_view()?
                    .read(cx)
                    .thread
                    .read(cx)
                    .draft_prompt()?
                    .to_vec(),
            )
        });
        self.pending_serialization = Some(cx.background_spawn(async move {
            save_serialized_panel(
                workspace_id,
                SerializedAgentPanel {
                    selected_agent: Some(selected_agent),
                    last_active_thread,
                    draft_thread_prompt,
                },
                kvp,
            )
            .await?;
            anyhow::Ok(())
        }));
    }

    pub fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> Task<Result<Entity<Self>>> {
        let prompt_store = cx.update(|_window, cx| PromptStore::global(cx));
        let kvp = cx.update(|_window, cx| KeyValueStore::global(cx)).ok();
        cx.spawn(async move |cx| {
            let prompt_store = match prompt_store {
                Ok(prompt_store) => prompt_store.await.ok(),
                Err(_) => None,
            };
            let workspace_id = workspace
                .read_with(cx, |workspace, _| workspace.database_id())
                .ok()
                .flatten();

            let (serialized_panel, global_last_used_agent) = cx
                .background_spawn(async move {
                    match kvp {
                        Some(kvp) => {
                            let panel = workspace_id
                                .and_then(|id| read_serialized_panel(id, &kvp))
                                .or_else(|| read_legacy_serialized_panel(&kvp));
                            let global_agent = read_global_last_used_agent(&kvp);
                            (panel, global_agent)
                        }
                        None => (None, None),
                    }
                })
                .await;

            let was_draft_active = serialized_panel
                .as_ref()
                .and_then(|p| p.last_active_thread.as_ref())
                .is_some_and(|t| t.session_id.is_none());

            let last_active_thread = if let Some(thread_info) = serialized_panel
                .as_ref()
                .and_then(|p| p.last_active_thread.as_ref())
            {
                match &thread_info.session_id {
                    Some(session_id_str) => {
                        let session_id = acp::SessionId::new(session_id_str.clone());
                        let is_restorable = cx
                            .update(|_window, cx| {
                                let store = ThreadMetadataStore::global(cx);
                                store
                                    .read(cx)
                                    .entry_by_session(&session_id)
                                    .is_some_and(|entry| !entry.archived)
                            })
                            .unwrap_or(false);
                        if is_restorable {
                            Some(thread_info)
                        } else {
                            log::info!(
                                "最近活跃线程 {} 已归档或不存在，跳过恢复",
                                session_id_str
                            );
                            None
                        }
                    }
                    None => None,
                }
            } else {
                None
            };

            let panel = workspace.update_in(cx, |workspace, window, cx| {
                let panel = cx.new(|cx| Self::new(workspace, prompt_store, window, cx));

                panel.update(cx, |panel, cx| {
                    let is_via_collab = panel.project.read(cx).is_via_collab();

                    // Only apply a non-native global fallback to local projects.
                    // Collab workspaces only support NativeAgent, so inheriting a
                    // custom agent would cause set_active → new_agent_thread_inner
                    // to bypass the collab guard in external_thread.
                    let global_fallback =
                        global_last_used_agent.filter(|agent| !is_via_collab || agent.is_native());

                    if let Some(serialized_panel) = &serialized_panel {
                        if let Some(selected_agent) = serialized_panel.selected_agent.clone() {
                            panel.selected_agent = selected_agent;
                        } else if let Some(agent) = global_fallback {
                            panel.selected_agent = agent;
                        }
                    } else if let Some(agent) = global_fallback {
                        panel.selected_agent = agent;
                    }
                    cx.notify();
                });

                if let Some(thread_info) = last_active_thread {
                    if let Some(session_id_str) = &thread_info.session_id {
                        let agent = thread_info.agent_type.clone();
                        let session_id: acp::SessionId = session_id_str.clone().into();
                        panel.update(cx, |panel, cx| {
                            panel.selected_agent = agent.clone();
                            panel.load_agent_thread(
                                agent,
                                session_id,
                                thread_info.work_dirs.as_ref().map(|dirs| PathList::deserialize(dirs)),
                                thread_info.title.as_ref().map(|t| t.clone().into()),
                                false,
                                "agent_panel",
                                window,
                                cx,
                            );
                        });
                    }
                }

                let draft_prompt = serialized_panel
                    .as_ref()
                    .and_then(|p| p.draft_thread_prompt.clone());

                if draft_prompt.is_some() || was_draft_active {
                    panel.update(cx, |panel, cx| {
                        let agent = if panel.project.read(cx).is_via_collab() {
                            Agent::NativeAgent
                        } else {
                            panel.selected_agent.clone()
                        };
                        let initial_content = draft_prompt.map(|blocks| {
                            AgentInitialContent::ContentBlock {
                                blocks,
                                auto_submit: false,
                            }
                        });
                        let thread = panel.create_agent_thread(
                            agent,
                            None,
                            None,
                            None,
                            initial_content,
                            "agent_panel",
                            window,
                            cx,
                        );
                        panel.draft_thread = Some(thread.conversation_view.clone());
                        panel.observe_draft_editor(&thread.conversation_view, cx);

                        if was_draft_active && last_active_thread.is_none() {
                            panel.set_base_view(
                                BaseView::AgentThread {
                                    conversation_view: thread.conversation_view,
                                },
                                false,
                                window,
                                cx,
                            );
                        }
                    });
                }

                panel
            })?;

            Ok(panel)
        })
    }

    pub(crate) fn new(
        workspace: &Workspace,
        prompt_store: Option<Entity<PromptStore>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let fs = workspace.app_state().fs.clone();
        let user_store = workspace.app_state().user_store.clone();
        let project = workspace.project();
        let language_registry = project.read(cx).languages().clone();
        let client = workspace.client().clone();
        let workspace_id = workspace.database_id();
        let workspace = workspace.weak_handle();

        let context_server_registry =
            cx.new(|cx| ContextServerRegistry::new(project.read(cx).context_server_store(), cx));

        let thread_store = ThreadStore::global(cx);

        let base_view = BaseView::Uninitialized;

        let weak_panel = cx.entity().downgrade();
        let onboarding = cx.new(|cx| {
            AgentPanelOnboarding::new(
                user_store.clone(),
                client,
                move |_window, cx| {
                    weak_panel
                        .update(cx, |panel, cx| {
                            panel.dismiss_ai_onboarding(cx);
                        })
                        .ok();
                },
                cx,
            )
        });

        // Subscribe to extension events to sync agent servers when extensions change
        let extension_subscription = if let Some(extension_events) = ExtensionEvents::try_global(cx)
        {
            Some(
                cx.subscribe(&extension_events, |this, _source, event, cx| match event {
                    extension::Event::ExtensionInstalled(_)
                    | extension::Event::ExtensionUninstalled(_)
                    | extension::Event::ExtensionsInstalledChanged => {
                        this.sync_agent_servers_from_extensions(cx);
                    }
                    _ => {}
                }),
            )
        } else {
            None
        };

        let connection_store = cx.new(|cx| {
            let mut store = AgentConnectionStore::new(project.clone(), cx);
            // Register the native agent right away, so that it is available for
            // the inline assistant etc.
            store.request_connection(
                Agent::NativeAgent,
                Agent::NativeAgent.server(fs.clone(), thread_store.clone()),
                cx,
            );
            store
        });
        let _project_subscription =
            cx.subscribe(&project, |this, _project, event, cx| match event {
                project::Event::WorktreeAdded(_)
                | project::Event::WorktreeRemoved(_)
                | project::Event::WorktreeOrderChanged => {
                    this.update_thread_work_dirs(cx);
                }
                _ => {}
            });

        let _thread_metadata_store_subscription = cx.subscribe(
            &ThreadMetadataStore::global(cx),
            |this, _store, event, cx| {
                let ThreadMetadataStoreEvent::ThreadArchived(thread_id) = event;
                if this.retained_threads.remove(thread_id).is_some() {
                    cx.notify();
                }
            },
        );

        let mut panel = Self {
            workspace_id,
            base_view,
            overlay_view: None,
            workspace,
            user_store,
            project: project.clone(),
            fs: fs.clone(),
            language_registry,
            prompt_store,
            connection_store,
            configuration: None,
            configuration_subscription: None,
            focus_handle: cx.focus_handle(),
            context_server_registry,
            draft_thread: None,
            retained_threads: HashMap::default(),
            new_thread_menu_handle: PopoverMenuHandle::default(),
            agent_panel_menu_handle: PopoverMenuHandle::default(),

            _extension_subscription: extension_subscription,
            _project_subscription,
            zoomed: false,
            pending_serialization: None,
            new_user_onboarding: onboarding,
            thread_store,
            selected_agent: Agent::default(),
            _thread_view_subscription: None,
            _active_thread_focus_subscription: None,
            show_trust_workspace_message: false,
            new_user_onboarding_upsell_dismissed: AtomicBool::new(OnboardingUpsell::dismissed(cx)),
            _base_view_observation: None,
            _draft_editor_observation: None,
            _thread_metadata_store_subscription,
        };

        // Initial sync of agent servers from extensions
        panel.sync_agent_servers_from_extensions(cx);
        panel
    }

    pub fn toggle_focus(
        workspace: &mut Workspace,
        _: &ToggleFocus,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        if workspace
            .panel::<Self>(cx)
            .is_some_and(|panel| panel.read(cx).enabled(cx))
        {
            workspace.toggle_panel_focus::<Self>(window, cx);
        }
    }

    pub fn focus(
        workspace: &mut Workspace,
        _: &FocusAgent,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        if workspace
            .panel::<Self>(cx)
            .is_some_and(|panel| panel.read(cx).enabled(cx))
        {
            workspace.focus_panel::<Self>(window, cx);
        }
    }

    pub fn toggle(
        workspace: &mut Workspace,
        _: &Toggle,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        if workspace
            .panel::<Self>(cx)
            .is_some_and(|panel| panel.read(cx).enabled(cx))
        {
            if !workspace.toggle_panel_focus::<Self>(window, cx) {
                workspace.close_panel::<Self>(window, cx);
            }
        }
    }

    pub(crate) fn prompt_store(&self) -> &Option<Entity<PromptStore>> {
        &self.prompt_store
    }

    pub fn thread_store(&self) -> &Entity<ThreadStore> {
        &self.thread_store
    }

    pub fn connection_store(&self) -> &Entity<AgentConnectionStore> {
        &self.connection_store
    }

    pub fn selected_agent(&self, cx: &App) -> Agent {
        if self.project.read(cx).is_via_collab() {
            Agent::NativeAgent
        } else {
            self.selected_agent.clone()
        }
    }

    pub fn open_thread(
        &mut self,
        session_id: acp::SessionId,
        work_dirs: Option<PathList>,
        title: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.load_agent_thread(
            crate::Agent::NativeAgent,
            session_id,
            work_dirs,
            title,
            true,
            "agent_panel",
            window,
            cx,
        );
    }

    pub(crate) fn context_server_registry(&self) -> &Entity<ContextServerRegistry> {
        &self.context_server_registry
    }

    pub fn is_visible(workspace: &Entity<Workspace>, cx: &App) -> bool {
        let workspace_read = workspace.read(cx);

        workspace_read
            .panel::<AgentPanel>(cx)
            .map(|panel| {
                let panel_id = Entity::entity_id(&panel);

                workspace_read.all_docks().iter().any(|dock| {
                    dock.read(cx)
                        .visible_panel()
                        .is_some_and(|visible_panel| visible_panel.panel_id() == panel_id)
                })
            })
            .unwrap_or(false)
    }

    /// Clear the active view, retaining any running thread in the background.
    pub fn clear_base_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let old_view = std::mem::replace(&mut self.base_view, BaseView::Uninitialized);
        self.retain_running_thread(old_view, cx);
        self.clear_overlay_state();
        self.activate_draft(false, "agent_panel", window, cx);
        self.serialize(cx);
        cx.emit(AgentPanelEvent::ActiveViewChanged);
        cx.notify();
    }

    pub fn new_thread(&mut self, _action: &NewThread, window: &mut Window, cx: &mut Context<Self>) {
        self.activate_draft(true, "agent_panel", window, cx);
    }

    pub fn new_external_agent_thread(
        &mut self,
        action: &NewExternalAgentThread,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(agent) = action.agent.clone() {
            self.selected_agent = agent;
        }
        self.activate_draft(true, "agent_panel", window, cx);
    }

    pub fn activate_draft(
        &mut self,
        focus: bool,
        source: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let draft = self.ensure_draft(source, window, cx);
        if let BaseView::AgentThread { conversation_view } = &self.base_view {
            if conversation_view.entity_id() == draft.entity_id() {
                if focus {
                    self.focus_handle(cx).focus(window, cx);
                }
                return;
            }
        }
        self.set_base_view(
            BaseView::AgentThread {
                conversation_view: draft,
            },
            focus,
            window,
            cx,
        );
    }

    fn ensure_draft(
        &mut self,
        source: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ConversationView> {
        let desired_agent = self.selected_agent(cx);
        if let Some(draft) = &self.draft_thread {
            let agent_matches = *draft.read(cx).agent_key() == desired_agent;
            if agent_matches {
                return draft.clone();
            }
            self.draft_thread = None;
            self._draft_editor_observation = None;
        }
        let previous_content = self.active_initial_content(cx);
        let thread = self.create_agent_thread(
            desired_agent,
            None,
            None,
            None,
            previous_content,
            source,
            window,
            cx,
        );
        self.draft_thread = Some(thread.conversation_view.clone());
        self.observe_draft_editor(&thread.conversation_view, cx);
        thread.conversation_view
    }

    fn observe_draft_editor(
        &mut self,
        conversation_view: &Entity<ConversationView>,
        cx: &mut Context<Self>,
    ) {
        if let Some(acp_thread) = conversation_view.read(cx).root_thread(cx) {
            self._draft_editor_observation = Some(cx.subscribe(
                &acp_thread,
                |this, _, e: &AcpThreadEvent, cx| {
                    if let AcpThreadEvent::PromptUpdated = e {
                        this.serialize(cx);
                    }
                },
            ));
        } else {
            let cv = conversation_view.clone();
            self._draft_editor_observation = Some(cx.observe(&cv, |this, cv, cx| {
                if cv.read(cx).root_thread(cx).is_some() {
                    this.observe_draft_editor(&cv, cx);
                }
            }));
        }
    }

    pub fn create_thread(
        &mut self,
        source: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ThreadId {
        let agent = self.selected_agent(cx);
        let thread = self.create_agent_thread(agent, None, None, None, None, source, window, cx);
        let thread_id = thread.conversation_view.read(cx).thread_id;
        self.retained_threads
            .insert(thread_id, thread.conversation_view);
        thread_id
    }

    pub fn activate_retained_thread(
        &mut self,
        id: ThreadId,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(conversation_view) = self.retained_threads.remove(&id) else {
            return;
        };
        self.set_base_view(
            BaseView::AgentThread { conversation_view },
            focus,
            window,
            cx,
        );
    }

    pub fn remove_thread(&mut self, id: ThreadId, window: &mut Window, cx: &mut Context<Self>) {
        self.retained_threads.remove(&id);
        ThreadMetadataStore::global(cx).update(cx, |store, cx| {
            store.delete(id, cx);
        });

        if self
            .draft_thread
            .as_ref()
            .is_some_and(|d| d.read(cx).thread_id == id)
        {
            self.draft_thread = None;
            self._draft_editor_observation = None;
        }

        if self.active_thread_id(cx) == Some(id) {
            self.clear_overlay_state();
            self.activate_draft(false, "agent_panel", window, cx);
            self.serialize(cx);
            cx.emit(AgentPanelEvent::ActiveViewChanged);
            cx.notify();
        }
    }

    pub fn active_thread_id(&self, cx: &App) -> Option<ThreadId> {
        match &self.base_view {
            BaseView::AgentThread { conversation_view } => {
                Some(conversation_view.read(cx).thread_id)
            }
            _ => None,
        }
    }

    pub fn editor_text(&self, id: ThreadId, cx: &App) -> Option<String> {
        let cv = self
            .retained_threads
            .get(&id)
            .or_else(|| match &self.base_view {
                BaseView::AgentThread { conversation_view }
                    if conversation_view.read(cx).thread_id == id =>
                {
                    Some(conversation_view)
                }
                _ => None,
            })?;
        let tv = cv.read(cx).root_thread_view()?;
        let text = tv.read(cx).message_editor.read(cx).text(cx);
        if text.trim().is_empty() {
            None
        } else {
            Some(text)
        }
    }

    pub fn clear_editor(&self, id: ThreadId, window: &mut Window, cx: &mut Context<Self>) {
        let cv = self
            .retained_threads
            .get(&id)
            .or_else(|| match &self.base_view {
                BaseView::AgentThread { conversation_view }
                    if conversation_view.read(cx).thread_id == id =>
                {
                    Some(conversation_view)
                }
                _ => None,
            });
        let Some(cv) = cv else { return };
        let Some(tv) = cv.read(cx).root_thread_view() else {
            return;
        };
        let editor = tv.read(cx).message_editor.clone();
        editor.update(cx, |editor, cx| {
            editor.clear(window, cx);
        });
    }

    fn new_native_agent_thread_from_summary(
        &mut self,
        action: &NewNativeAgentThreadFromSummary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let session_id = action.from_session_id.clone();

        let Some(content) = Self::initial_content_for_thread_summary(session_id.clone(), cx) else {
            log::error!("No session found for summarization with id {}", session_id);
            return;
        };

        cx.spawn_in(window, async move |this, cx| {
            this.update_in(cx, |this, window, cx| {
                this.external_thread(
                    Some(Agent::NativeAgent),
                    None,
                    None,
                    None,
                    Some(content),
                    true,
                    "agent_panel",
                    window,
                    cx,
                );
                anyhow::Ok(())
            })
        })
        .detach_and_log_err(cx);
    }

    fn initial_content_for_thread_summary(
        session_id: acp::SessionId,
        cx: &App,
    ) -> Option<AgentInitialContent> {
        let thread = ThreadStore::global(cx)
            .read(cx)
            .entries()
            .find(|t| t.id == session_id)?;

        Some(AgentInitialContent::ThreadSummary {
            session_id: thread.id,
            title: Some(thread.title),
        })
    }

    fn external_thread(
        &mut self,
        agent_choice: Option<crate::Agent>,
        resume_session_id: Option<acp::SessionId>,
        work_dirs: Option<PathList>,
        title: Option<SharedString>,
        initial_content: Option<AgentInitialContent>,
        focus: bool,
        source: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let agent = agent_choice.unwrap_or_else(|| self.selected_agent(cx));
        let thread = self.create_agent_thread(
            agent,
            resume_session_id,
            work_dirs,
            title,
            initial_content,
            source,
            window,
            cx,
        );
        self.set_base_view(thread.into(), focus, window, cx);
    }

    fn deploy_rules_library(
        &mut self,
        action: &OpenRulesLibrary,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        open_rules_library(
            self.language_registry.clone(),
            Box::new(PromptLibraryInlineAssist::new(self.workspace.clone())),
            action
                .prompt_to_select
                .map(|uuid| UserPromptId(uuid).into()),
            cx,
        )
        .detach_and_log_err(cx);
    }

    fn expand_message_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(conversation_view) = self.active_conversation_view() else {
            return;
        };

        let Some(active_thread) = conversation_view.read(cx).root_thread_view() else {
            return;
        };

        active_thread.update(cx, |active_thread, cx| {
            active_thread.expand_message_editor(&ExpandMessageEditor, window, cx);
            active_thread.focus_handle(cx).focus(window, cx);
        })
    }

    pub fn go_back(&mut self, _: &workspace::GoBack, window: &mut Window, cx: &mut Context<Self>) {
        if self.overlay_view.is_some() {
            self.clear_overlay(true, window, cx);
            cx.notify();
        }
    }

    pub fn toggle_options_menu(
        &mut self,
        _: &ToggleOptionsMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        self.agent_panel_menu_handle.toggle(window, cx);
    }

    pub fn toggle_new_thread_menu(
        &mut self,
        _: &ToggleNewThreadMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.new_thread_menu_handle.toggle(window, cx);
    }

    pub fn increase_font_size(
        &mut self,
        action: &IncreaseBufferFontSize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_font_size_action(action.persist, px(1.0), cx);
    }

    pub fn decrease_font_size(
        &mut self,
        action: &DecreaseBufferFontSize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_font_size_action(action.persist, px(-1.0), cx);
    }

    fn handle_font_size_action(&mut self, persist: bool, delta: Pixels, cx: &mut Context<Self>) {
        match self.visible_font_size() {
            WhichFontSize::AgentFont => {
                if persist {
                    update_settings_file(self.fs.clone(), cx, move |settings, cx| {
                        let agent_ui_font_size =
                            ThemeSettings::get_global(cx).agent_ui_font_size(cx) + delta;
                        let agent_buffer_font_size =
                            ThemeSettings::get_global(cx).agent_buffer_font_size(cx) + delta;

                        let _ = settings.theme.agent_ui_font_size.insert(
                            f32::from(theme_settings::clamp_font_size(agent_ui_font_size)).into(),
                        );
                        let _ = settings.theme.agent_buffer_font_size.insert(
                            f32::from(theme_settings::clamp_font_size(agent_buffer_font_size))
                                .into(),
                        );
                    });
                } else {
                    theme_settings::adjust_agent_ui_font_size(cx, |size| size + delta);
                    theme_settings::adjust_agent_buffer_font_size(cx, |size| size + delta);
                }
            }
            WhichFontSize::None => {}
        }
    }

    pub fn reset_font_size(
        &mut self,
        action: &ResetBufferFontSize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if action.persist {
            update_settings_file(self.fs.clone(), cx, move |settings, _| {
                settings.theme.agent_ui_font_size = None;
                settings.theme.agent_buffer_font_size = None;
            });
        } else {
            theme_settings::reset_agent_ui_font_size(cx);
            theme_settings::reset_agent_buffer_font_size(cx);
        }
    }

    pub fn reset_agent_zoom(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        theme_settings::reset_agent_ui_font_size(cx);
        theme_settings::reset_agent_buffer_font_size(cx);
    }

    pub fn toggle_zoom(&mut self, _: &ToggleZoom, window: &mut Window, cx: &mut Context<Self>) {
        if self.zoomed {
            cx.emit(PanelEvent::ZoomOut);
        } else {
            if !self.focus_handle(cx).contains_focused(window, cx) {
                cx.focus_self(window);
            }
            cx.emit(PanelEvent::ZoomIn);
        }
    }

    pub(crate) fn open_configuration(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.overlay_view, Some(OverlayView::Configuration)) {
            self.clear_overlay(true, window, cx);
            return;
        }

        let agent_server_store = self.project.read(cx).agent_server_store().clone();
        let context_server_store = self.project.read(cx).context_server_store();
        let fs = self.fs.clone();

        self.configuration = Some(cx.new(|cx| {
            AgentConfiguration::new(
                fs,
                agent_server_store,
                self.connection_store.clone(),
                context_server_store,
                self.context_server_registry.clone(),
                self.language_registry.clone(),
                self.workspace.clone(),
                window,
                cx,
            )
        }));

        if let Some(configuration) = self.configuration.as_ref() {
            self.configuration_subscription = Some(cx.subscribe_in(
                configuration,
                window,
                Self::handle_agent_configuration_event,
            ));
        }

        self.set_overlay(OverlayView::Configuration, true, window, cx);

        if let Some(configuration) = self.configuration.as_ref() {
            configuration.focus_handle(cx).focus(window, cx);
        }
    }

    pub(crate) fn open_active_thread_as_markdown(
        &mut self,
        _: &OpenActiveThreadAsMarkdown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(workspace) = self.workspace.upgrade()
            && let Some(conversation_view) = self.active_conversation_view()
            && let Some(active_thread) = conversation_view.read(cx).active_thread().cloned()
        {
            active_thread.update(cx, |thread, cx| {
                thread
                    .open_thread_as_markdown(workspace, window, cx)
                    .detach_and_log_err(cx);
            });
        }
    }

    fn copy_thread_to_clipboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread) = self.active_native_agent_thread(cx) else {
            Self::show_deferred_toast(&self.workspace, "无可用的活动线程可复制", cx);
            return;
        };

        let workspace = self.workspace.clone();
        let load_task = thread.read(cx).to_db(cx);

        cx.spawn_in(window, async move |_this, cx| {
            let db_thread = load_task.await;
            let shared_thread = SharedThread::from_db_thread(&db_thread);
            let thread_data = shared_thread.to_bytes()?;
            let encoded = base64::Engine::encode(&base64::prelude::BASE64_STANDARD, &thread_data);

            cx.update(|_window, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(encoded));
                if let Some(workspace) = workspace.upgrade() {
                    workspace.update(cx, |workspace, cx| {
                        struct ThreadCopiedToast;
                        workspace.show_toast(
                            workspace::Toast::new(
                                workspace::notifications::NotificationId::unique::<ThreadCopiedToast>(),
                                "线程已复制到剪贴板（已进行Base64编码）",
                            )
                            .autohide(),
                            cx,
                        );
                    });
                }
            })?;

            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    fn show_deferred_toast(
        workspace: &WeakEntity<workspace::Workspace>,
        message: &'static str,
        cx: &mut App,
    ) {
        let workspace = workspace.clone();
        cx.defer(move |cx| {
            if let Some(workspace) = workspace.upgrade() {
                workspace.update(cx, |workspace, cx| {
                    struct ClipboardToast;
                    workspace.show_toast(
                        workspace::Toast::new(
                            workspace::notifications::NotificationId::unique::<ClipboardToast>(),
                            message,
                        )
                        .autohide(),
                        cx,
                    );
                });
            }
        });
    }

    /// 从剪贴板加载线程
    fn load_thread_from_clipboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(clipboard) = cx.read_from_clipboard() else {
            Self::show_deferred_toast(&self.workspace, "剪贴板无内容", cx);
            return;
        };

        let Some(encoded) = clipboard.text() else {
            Self::show_deferred_toast(&self.workspace, "剪贴板不包含文本内容", cx);
            return;
        };

        let thread_data = match base64::Engine::decode(&base64::prelude::BASE64_STANDARD, &encoded)
        {
            Ok(data) => data,
            Err(_) => {
                Self::show_deferred_toast(
                    &self.workspace,
                    "剪贴板内容解码失败（期望为base64格式）",
                    cx,
                );
                return;
            }
        };

        let shared_thread = match SharedThread::from_bytes(&thread_data) {
            Ok(thread) => thread,
            Err(_) => {
                Self::show_deferred_toast(
                    &self.workspace,
                    "从剪贴板解析线程数据失败",
                    cx,
                );
                return;
            }
        };

        let db_thread = shared_thread.to_db_thread();
        let session_id = acp::SessionId::new(uuid::Uuid::new_v4().to_string());
        let thread_store = self.thread_store.clone();
        let title = db_thread.title.clone();
        let workspace = self.workspace.clone();

        cx.spawn_in(window, async move |this, cx| {
            thread_store
                .update(&mut cx.clone(), |store, cx| {
                    store.save_thread(session_id.clone(), db_thread, Default::default(), cx)
                })
                .await?;

            this.update_in(cx, |this, window, cx| {
                this.open_thread(session_id, None, Some(title), window, cx);
            })?;

            this.update_in(cx, |_, _window, cx| {
                if let Some(workspace) = workspace.upgrade() {
                    workspace.update(cx, |workspace, cx| {
                        struct ThreadLoadedToast;
                        workspace.show_toast(
                            workspace::Toast::new(
                                workspace::notifications::NotificationId::unique::<ThreadLoadedToast>(),
                                "已从剪贴板加载线程",
                            )
                            .autohide(),
                            cx,
                        );
                    });
                }
            })?;

            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
    }

    /// 显示线程元数据
    fn show_thread_metadata(
        &mut self,
        _: &ShowThreadMetadata,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(thread_id) = self.active_thread_id(cx) else {
            Self::show_deferred_toast(&self.workspace, "无活动线程", cx);
            return;
        };

        let Some(store) = ThreadMetadataStore::try_global(cx) else {
            Self::show_deferred_toast(&self.workspace, "线程元数据存储不可用", cx);
            return;
        };

        let Some(metadata) = store.read(cx).entry(thread_id).cloned() else {
            Self::show_deferred_toast(&self.workspace, "未找到活动线程的元数据", cx);
            return;
        };

        let json = thread_metadata_to_debug_json(&metadata);
        let text = serde_json::to_string_pretty(&json).unwrap_or_default();
        let title = format!("线程元数据：{}", metadata.display_title());

        self.open_json_buffer(title, text, window, cx);
    }

    /// 显示所有侧边栏线程元数据
    fn show_all_sidebar_thread_metadata(
        &mut self,
        _: &ShowAllSidebarThreadMetadata,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = ThreadMetadataStore::try_global(cx) else {
            Self::show_deferred_toast(&self.workspace, "线程元数据存储不可用", cx);
            return;
        };

    let entries: Vec<serde_json::Value> = store
        .read(cx)
        .entries()
        .filter(|t| !t.archived)
        .map(thread_metadata_to_debug_json)
        .collect();

    let json = serde_json::Value::Array(entries);
    let text = serde_json::to_string_pretty(&json).unwrap_or_default();

    self.open_json_buffer("所有侧边栏线程元数据".to_string(), text, window, cx);
}

    fn open_json_buffer(
        &self,
        title: String,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let json_language = self.language_registry.language_for_name("JSON");
        let project = self.project.clone();
        let workspace = self.workspace.clone();

        window
            .spawn(cx, async move |cx| {
                let json_language = json_language.await.ok();

                let buffer = project
                    .update(cx, |project, cx| {
                        project.create_buffer(json_language, false, cx)
                    })
                    .await?;

                buffer.update(cx, |buffer, cx| {
                    buffer.set_text(text, cx);
                    buffer.set_capability(language::Capability::ReadWrite, cx);
                });

                workspace.update_in(cx, |workspace, window, cx| {
                    let buffer =
                        cx.new(|cx| MultiBuffer::singleton(buffer, cx).with_title(title.clone()));

                    workspace.add_item_to_active_pane(
                        Box::new(cx.new(|cx| {
                            let mut editor =
                                Editor::for_multibuffer(buffer, Some(project.clone()), window, cx);
                            editor.set_breadcrumb_header(title);
                            editor.disable_mouse_wheel_zoom();
                            editor
                        })),
                        None,
                        true,
                        window,
                        cx,
                    );
                })?;

                anyhow::Ok(())
            })
            .detach_and_log_err(cx);
    }

    fn handle_agent_configuration_event(
        &mut self,
        _entity: &Entity<AgentConfiguration>,
        event: &AssistantConfigurationEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            AssistantConfigurationEvent::NewThread(provider) => {
                if LanguageModelRegistry::read_global(cx)
                    .default_model()
                    .is_none_or(|model| model.provider.id() != provider.id())
                    && let Some(model) = provider.default_model(cx)
                {
                    update_settings_file(self.fs.clone(), cx, move |settings, _| {
                        let provider = model.provider_id().0.to_string();
                        let enable_thinking = model.supports_thinking();
                        let effort = model
                            .default_effort_level()
                            .map(|effort| effort.value.to_string());
                        let model = model.id().0.to_string();
                        settings
                            .agent
                            .get_or_insert_default()
                            .set_model(LanguageModelSelection {
                                provider: LanguageModelProviderSetting(provider),
                                model,
                                enable_thinking,
                                effort,
                                speed: None,
                            })
                    });
                }

                self.new_thread(&NewThread, window, cx);
                if let Some((thread, model)) = self
                    .active_native_agent_thread(cx)
                    .zip(provider.default_model(cx))
                {
                    thread.update(cx, |thread, cx| {
                        thread.set_model(model, cx);
                    });
                }
            }
        }
    }

    pub fn workspace_id(&self) -> Option<WorkspaceId> {
        self.workspace_id
    }

    pub fn retained_threads(&self) -> &HashMap<ThreadId, Entity<ConversationView>> {
        &self.retained_threads
    }

    pub fn active_conversation_view(&self) -> Option<&Entity<ConversationView>> {
        match &self.base_view {
            BaseView::AgentThread { conversation_view } => Some(conversation_view),
            _ => None,
        }
    }

    pub fn conversation_views(&self) -> Vec<Entity<ConversationView>> {
        self.active_conversation_view()
            .into_iter()
            .cloned()
            .chain(self.retained_threads.values().cloned())
            .collect()
    }

    pub fn active_thread_view(&self, cx: &App) -> Option<Entity<ThreadView>> {
        let server_view = self.active_conversation_view()?;
        server_view.read(cx).root_thread_view()
    }

    pub fn active_agent_thread(&self, cx: &App) -> Option<Entity<AcpThread>> {
        match &self.base_view {
            BaseView::AgentThread { conversation_view } => {
                conversation_view.read(cx).root_thread(cx)
            }
            _ => None,
        }
    }

    pub fn is_retained_thread(&self, id: &ThreadId) -> bool {
        self.retained_threads.contains_key(id)
    }

    pub fn cancel_thread(&self, thread_id: &ThreadId, cx: &mut Context<Self>) -> bool {
        let conversation_views = self
            .active_conversation_view()
            .into_iter()
            .chain(self.retained_threads.values());

        for conversation_view in conversation_views {
            if *thread_id == conversation_view.read(cx).thread_id {
                if let Some(thread_view) = conversation_view.read(cx).root_thread_view() {
                    thread_view.update(cx, |view, cx| view.cancel_generation(cx));
                    return true;
                }
            }
        }
        false
    }

    /// active thread plus any background threads that are still running or
    /// completed but unseen.
    pub fn parent_threads(&self, cx: &App) -> Vec<Entity<ThreadView>> {
        let mut views = Vec::new();

        if let Some(server_view) = self.active_conversation_view() {
            if let Some(thread_view) = server_view.read(cx).root_thread_view() {
                views.push(thread_view);
            }
        }

        for server_view in self.retained_threads.values() {
            if let Some(thread_view) = server_view.read(cx).root_thread_view() {
                views.push(thread_view);
            }
        }

        views
    }

    fn update_thread_work_dirs(&self, cx: &mut Context<Self>) {
        let new_work_dirs = self.project.read(cx).default_path_list(cx);
        let new_worktree_paths = self.project.read(cx).worktree_paths(cx);

        if let Some(conversation_view) = self.active_conversation_view() {
            conversation_view.update(cx, |conversation_view, cx| {
                conversation_view.set_work_dirs(new_work_dirs.clone(), cx);
            });
        }

        for conversation_view in self.retained_threads.values() {
            conversation_view.update(cx, |conversation_view, cx| {
                conversation_view.set_work_dirs(new_work_dirs.clone(), cx);
            });
        }

        if self.project.read(cx).is_via_collab() {
            return;
        }

        // Update metadata store so threads' path lists stay in sync with
        // the project's current worktrees. Without this, threads saved
        // before a worktree was added would have stale paths and not
        // appear under the correct sidebar group.
        let mut thread_ids: Vec<ThreadId> = self.retained_threads.keys().copied().collect();
        if let Some(active_id) = self.active_thread_id(cx) {
            thread_ids.push(active_id);
        }
        if !thread_ids.is_empty() {
            ThreadMetadataStore::global(cx).update(cx, |store, cx| {
                store.update_worktree_paths(&thread_ids, new_worktree_paths, cx);
            });
        }
    }

    fn retain_running_thread(&mut self, old_view: BaseView, cx: &mut Context<Self>) {
        let BaseView::AgentThread { conversation_view } = old_view else {
            return;
        };

        if self
            .draft_thread
            .as_ref()
            .is_some_and(|d| d.entity_id() == conversation_view.entity_id())
        {
            return;
        }

        let thread_id = conversation_view.read(cx).thread_id;

        if self.retained_threads.contains_key(&thread_id) {
            return;
        }

        self.retained_threads.insert(thread_id, conversation_view);
        self.cleanup_retained_threads(cx);
    }

    fn cleanup_retained_threads(&mut self, cx: &App) {
        let mut potential_removals = self
            .retained_threads
            .iter()
            .filter(|(_id, view)| {
                let Some(thread_view) = view.read(cx).root_thread_view() else {
                    return true;
                };
                let thread = thread_view.read(cx).thread.read(cx);
                thread.connection().supports_load_session() && thread.status() == ThreadStatus::Idle
            })
            .collect::<Vec<_>>();

        let max_idle = MaxIdleRetainedThreads::global(cx);

        potential_removals.sort_unstable_by_key(|(_, view)| view.read(cx).updated_at(cx));
        let n = potential_removals.len().saturating_sub(max_idle);
        let to_remove = potential_removals
            .into_iter()
            .map(|(id, _)| *id)
            .take(n)
            .collect::<Vec<_>>();
        for id in to_remove {
            self.retained_threads.remove(&id);
        }
    }

    pub(crate) fn active_native_agent_thread(&self, cx: &App) -> Option<Entity<agent::Thread>> {
        match &self.base_view {
            BaseView::AgentThread { conversation_view } => {
                conversation_view.read(cx).as_native_thread(cx)
            }
            _ => None,
        }
    }

    fn set_base_view(
        &mut self,
        new_view: BaseView,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.clear_overlay_state();

        let old_view = std::mem::replace(&mut self.base_view, new_view);
        self.retain_running_thread(old_view, cx);

        if let BaseView::AgentThread { conversation_view } = &self.base_view {
            let thread_agent = conversation_view.read(cx).agent_key().clone();
            if self.selected_agent != thread_agent {
                self.selected_agent = thread_agent;
                self.serialize(cx);
            }
        }

        self.refresh_base_view_subscriptions(window, cx);

        if focus {
            self.focus_handle(cx).focus(window, cx);
        }
        cx.emit(AgentPanelEvent::ActiveViewChanged);
    }

    fn set_overlay(
        &mut self,
        overlay: OverlayView,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.overlay_view = Some(overlay);
        if focus {
            self.focus_handle(cx).focus(window, cx);
        }
        cx.emit(AgentPanelEvent::ActiveViewChanged);
    }

    fn clear_overlay(&mut self, focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_overlay_state();

        if focus {
            self.focus_handle(cx).focus(window, cx);
        }
        cx.emit(AgentPanelEvent::ActiveViewChanged);
    }

    fn clear_overlay_state(&mut self) {
        self.overlay_view = None;
        self.configuration_subscription = None;
        self.configuration = None;
    }

    fn refresh_base_view_subscriptions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._base_view_observation = match &self.base_view {
            BaseView::AgentThread { conversation_view } => {
                self._thread_view_subscription =
                    Self::subscribe_to_active_thread_view(conversation_view, window, cx);
                let focus_handle = conversation_view.focus_handle(cx);
                self._active_thread_focus_subscription =
                    Some(cx.on_focus_in(&focus_handle, window, |_this, _window, cx| {
                        cx.emit(AgentPanelEvent::ThreadFocused);
                        cx.notify();
                    }));
                Some(cx.observe_in(
                    conversation_view,
                    window,
                    |this, server_view, window, cx| {
                        this._thread_view_subscription =
                            Self::subscribe_to_active_thread_view(&server_view, window, cx);
                        cx.emit(AgentPanelEvent::ActiveViewChanged);
                        this.serialize(cx);
                        cx.notify();
                    },
                ))
            }
            BaseView::Uninitialized => {
                self._thread_view_subscription = None;
                self._active_thread_focus_subscription = None;
                None
            }
        };
        self.serialize(cx);
    }

    fn visible_surface(&self) -> VisibleSurface<'_> {
        if let Some(overlay_view) = &self.overlay_view {
            return match overlay_view {
                OverlayView::Configuration => {
                    VisibleSurface::Configuration(self.configuration.as_ref())
                }
            };
        }

        match &self.base_view {
            BaseView::Uninitialized => VisibleSurface::Uninitialized,
            BaseView::AgentThread { conversation_view } => {
                VisibleSurface::AgentThread(conversation_view)
            }
        }
    }

    fn is_overlay_open(&self) -> bool {
        self.overlay_view.is_some()
    }

    fn visible_font_size(&self) -> WhichFontSize {
        self.overlay_view.as_ref().map_or_else(
            || self.base_view.which_font_size_used(),
            OverlayView::which_font_size_used,
        )
    }

    fn subscribe_to_active_thread_view(
        server_view: &Entity<ConversationView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Subscription> {
        server_view.read(cx).root_thread_view().map(|tv| {
            cx.subscribe_in(
                &tv,
                window,
                |this, _view, event: &AcpThreadViewEvent, _window, cx| match event {
                    AcpThreadViewEvent::Interacted => {
                        let Some(thread_id) = this.active_thread_id(cx) else {
                            return;
                        };
                        if this.draft_thread.as_ref().is_some_and(|d| {
                            this.active_conversation_view()
                                .is_some_and(|active| active.entity_id() == d.entity_id())
                        }) {
                            this.draft_thread = None;
                            this._draft_editor_observation = None;
                        }
                        this.retained_threads.remove(&thread_id);
                        cx.emit(AgentPanelEvent::ThreadInteracted { thread_id });
                    }
                },
            )
        })
    }

    fn sync_agent_servers_from_extensions(&mut self, cx: &mut Context<Self>) {
        if let Some(extension_store) = ExtensionStore::try_global(cx) {
            let (manifests, extensions_dir) = {
                let store = extension_store.read(cx);
                let installed = store.installed_extensions();
                let manifests: Vec<_> = installed
                    .iter()
                    .map(|(id, entry)| (id.clone(), entry.manifest.clone()))
                    .collect();
                let extensions_dir = paths::extensions_dir().join("installed");
                (manifests, extensions_dir)
            };

            self.project.update(cx, |project, cx| {
                project.agent_server_store().update(cx, |store, cx| {
                    let manifest_refs: Vec<_> = manifests
                        .iter()
                        .map(|(id, manifest)| (id.as_ref(), manifest.as_ref()))
                        .collect();
                    store.sync_extension_agents(manifest_refs, extensions_dir, cx);
                });
            });
        }
    }

    pub fn new_agent_thread_with_external_source_prompt(
        &mut self,
        external_source_prompt: Option<ExternalSourcePrompt>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.external_thread(
            None,
            None,
            None,
            None,
            external_source_prompt.map(AgentInitialContent::from),
            true,
            "agent_panel",
            window,
            cx,
        );
    }

    pub fn load_agent_thread(
        &mut self,
        agent: Agent,
        session_id: acp::SessionId,
        work_dirs: Option<PathList>,
        title: Option<SharedString>,
        focus: bool,
        source: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(store) = ThreadMetadataStore::try_global(cx) {
            let thread_id = store
                .read(cx)
                .entry_by_session(&session_id)
                .map(|t| t.thread_id);
            if let Some(thread_id) = thread_id {
                store.update(cx, |store, cx| {
                    store.unarchive(thread_id, cx);
                });
            }
        }

        let has_session = |cv: &Entity<ConversationView>| -> bool {
            cv.read(cx)
                .root_session_id
                .as_ref()
                .is_some_and(|id| id == &session_id)
        };

        // Check if the active view already has this session.
        if let BaseView::AgentThread { conversation_view } = &self.base_view {
            if has_session(conversation_view) {
                self.clear_overlay_state();
                cx.emit(AgentPanelEvent::ActiveViewChanged);
                return;
            }
        }

        // Check if a retained thread has this session — promote it.
        let retained_key = self
            .retained_threads
            .iter()
            .find(|(_, cv)| has_session(cv))
            .map(|(id, _)| *id);
        if let Some(thread_id) = retained_key {
            if let Some(conversation_view) = self.retained_threads.remove(&thread_id) {
                self.set_base_view(
                    BaseView::AgentThread { conversation_view },
                    focus,
                    window,
                    cx,
                );
                return;
            }
        }

        self.external_thread(
            Some(agent),
            Some(session_id),
            work_dirs,
            title,
            None,
            focus,
            source,
            window,
            cx,
        );
    }

    pub(crate) fn create_agent_thread(
        &mut self,
        agent: Agent,
        resume_session_id: Option<acp::SessionId>,
        work_dirs: Option<PathList>,
        title: Option<SharedString>,
        initial_content: Option<AgentInitialContent>,
        source: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AgentThread {
        self.create_agent_thread_with_server(
            agent,
            None,
            resume_session_id,
            work_dirs,
            title,
            initial_content,
            source,
            window,
            cx,
        )
    }

    fn create_agent_thread_with_server(
        &mut self,
        agent: Agent,
        server_override: Option<Rc<dyn AgentServer>>,
        resume_session_id: Option<acp::SessionId>,
        work_dirs: Option<PathList>,
        title: Option<SharedString>,
        initial_content: Option<AgentInitialContent>,
        source: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AgentThread {
        let existing_metadata = resume_session_id.as_ref().and_then(|sid| {
            ThreadMetadataStore::try_global(cx)
                .and_then(|store| store.read(cx).entry_by_session(sid).cloned())
        });
        let thread_id = existing_metadata
            .as_ref()
            .map(|m| m.thread_id)
            .unwrap_or_else(ThreadId::new);
        let workspace = self.workspace.clone();
        let project = self.project.clone();

        if self.selected_agent != agent {
            self.selected_agent = agent.clone();
            self.serialize(cx);
        }

        cx.background_spawn({
            let kvp = KeyValueStore::global(cx);
            let agent = agent.clone();
            async move {
                write_global_last_used_agent(kvp, agent).await;
            }
        })
        .detach();

        let server = server_override
            .unwrap_or_else(|| agent.server(self.fs.clone(), self.thread_store.clone()));
        let thread_store = server
            .clone()
            .downcast::<agent::NativeAgentServer>()
            .is_some()
            .then(|| self.thread_store.clone());

        let connection_store = self.connection_store.clone();

        let conversation_view = cx.new(|cx| {
            crate::ConversationView::new(
                server,
                connection_store,
                agent,
                resume_session_id,
                Some(thread_id),
                work_dirs,
                title,
                initial_content,
                workspace.clone(),
                project,
                thread_store,
                self.prompt_store.clone(),
                source,
                window,
                cx,
            )
        });

        cx.observe(&conversation_view, |this, server_view, cx| {
            let is_active = this
                .active_conversation_view()
                .is_some_and(|active| active.entity_id() == server_view.entity_id());
            if is_active {
                cx.emit(AgentPanelEvent::ActiveViewChanged);
                this.serialize(cx);
            } else {
                cx.emit(AgentPanelEvent::RetainedThreadChanged);
            }
            cx.notify();
        })
        .detach();

        AgentThread { conversation_view }
    }

    fn active_thread_has_messages(&self, cx: &App) -> bool {
        self.active_agent_thread(cx)
            .is_some_and(|thread| !thread.read(cx).entries().is_empty())
    }

    pub fn active_thread_is_draft(&self, _cx: &App) -> bool {
        self.draft_thread.as_ref().is_some_and(|draft| {
            self.active_conversation_view()
                .is_some_and(|active| active.entity_id() == draft.entity_id())
        })
    }
}

impl Focusable for AgentPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.visible_surface() {
            VisibleSurface::Uninitialized => self.focus_handle.clone(),
            VisibleSurface::AgentThread(conversation_view) => conversation_view.focus_handle(cx),
            VisibleSurface::Configuration(configuration) => {
                if let Some(configuration) = configuration {
                    configuration.focus_handle(cx)
                } else {
                    self.focus_handle.clone()
                }
            }
        }
    }
}

fn agent_panel_dock_position(cx: &App) -> DockPosition {
    AgentSettings::get_global(cx).dock.into()
}

pub enum AgentPanelEvent {
    ActiveViewChanged,
    ThreadFocused,
    RetainedThreadChanged,
    ThreadInteracted { thread_id: ThreadId },
}

impl EventEmitter<PanelEvent> for AgentPanel {}
impl EventEmitter<AgentPanelEvent> for AgentPanel {}

impl Panel for AgentPanel {
    /// 持久化名称
    fn persistent_name() -> &'static str {
    "代理面板"
    }

    /// 面板存储键
    fn panel_key() -> &'static str {
        AGENT_PANEL_KEY
    }

    /// 获取面板停靠位置
    fn position(&self, _window: &Window, cx: &App) -> DockPosition {
        agent_panel_dock_position(cx)
    }

    /// 校验面板位置是否有效
    fn position_is_valid(&self, position: DockPosition) -> bool {
        position != DockPosition::Bottom
    }

    /// 设置面板停靠位置
    fn set_position(&mut self, position: DockPosition, _: &mut Window, cx: &mut Context<Self>) {
        let side = match position {
            DockPosition::Left => "left",
            DockPosition::Right | DockPosition::Bottom => "right",
        };
        telemetry::event!("代理面板侧边已切换", side = side);
        settings::update_settings_file(self.fs.clone(), cx, move |settings, _| {
            settings
                .agent
                .get_or_insert_default()
                .set_dock(position.into());
        });
    }

    /// 默认尺寸
    fn default_size(&self, window: &Window, cx: &App) -> Pixels {
        let settings = AgentSettings::get_global(cx);
        match self.position(window, cx) {
            DockPosition::Left | DockPosition::Right => settings.default_width,
            DockPosition::Bottom => settings.default_height,
        }
    }

    /// 最小尺寸
    fn min_size(&self, window: &Window, cx: &App) -> Option<Pixels> {
        match self.position(window, cx) {
            DockPosition::Left | DockPosition::Right => Some(MIN_PANEL_WIDTH),
            DockPosition::Bottom => None,
        }
    }

    /// 支持弹性尺寸
    fn supports_flexible_size(&self) -> bool {
        true
    }

    /// 是否启用弹性尺寸
    fn has_flexible_size(&self, _window: &Window, cx: &App) -> bool {
        AgentSettings::get_global(cx).flexible
    }

    /// 设置弹性尺寸
    fn set_flexible_size(&mut self, flexible: bool, _window: &mut Window, cx: &mut Context<Self>) {
        settings::update_settings_file(self.fs.clone(), cx, move |settings, _| {
            settings
                .agent
                .get_or_insert_default()
                .set_flexible_size(flexible);
        });
    }

    /// 设置激活状态
    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        if active {
            self.ensure_thread_initialized(window, cx);
        }
    }

    /// 远程面板ID
    fn remote_id() -> Option<proto::PanelId> {
        Some(proto::PanelId::AssistantPanel)
    }

    /// 面板图标
    fn icon(&self, _window: &Window, cx: &App) -> Option<IconName> {
        (self.enabled(cx) && AgentSettings::get_global(cx).button).then_some(IconName::ZedAssistant)
    }

    /// 图标提示文本
    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("智能体面板")
    }

    /// 切换操作
    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    /// 激活优先级
    fn activation_priority(&self) -> u32 {
        0
    }

    /// 面板是否启用
    fn enabled(&self, cx: &App) -> bool {
        AgentSettings::get_global(cx).enabled(cx)
    }

    /// 判断是否为智能体面板
    fn is_agent_panel(&self) -> bool {
        true
    }

    /// 是否放大状态
    fn is_zoomed(&self, _window: &Window, _cx: &App) -> bool {
        self.zoomed
    }

    /// 设置放大状态
    fn set_zoomed(&mut self, zoomed: bool, _window: &mut Window, cx: &mut Context<Self>) {
        self.zoomed = zoomed;
        cx.notify();
    }
}

impl AgentPanel {
    /// 确保线程已初始化
    fn ensure_thread_initialized(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.base_view, BaseView::Uninitialized) {
            self.activate_draft(false, "agent_panel", window, cx);
        }
    }

    /// 目标面板是否包含有意义的状态
    fn destination_has_meaningful_state(&self, cx: &App) -> bool {
        if self.overlay_view.is_some() || !self.retained_threads.is_empty() {
            return true;
        }

        match &self.base_view {
            BaseView::Uninitialized => false,
            BaseView::AgentThread { conversation_view } => {
                let has_entries = conversation_view
                    .read(cx)
                    .root_thread_view()
                    .is_some_and(|tv| !tv.read(cx).thread.read(cx).entries().is_empty());
                if has_entries {
                    return true;
                }

                conversation_view
                    .read(cx)
                    .root_thread_view()
                    .is_some_and(|thread_view| {
                        let thread_view = thread_view.read(cx);
                        thread_view
                            .thread
                            .read(cx)
                            .draft_prompt()
                            .is_some_and(|draft| !draft.is_empty())
                            || !thread_view
                                .message_editor
                                .read(cx)
                                .text(cx)
                                .trim()
                                .is_empty()
                    })
            }
        }
    }

    /// 获取活动线程的初始内容
    fn active_initial_content(&self, cx: &App) -> Option<AgentInitialContent> {
        self.active_thread_view(cx).and_then(|thread_view| {
            thread_view
                .read(cx)
                .thread
                .read(cx)
                .draft_prompt()
                .map(|draft| AgentInitialContent::ContentBlock {
                    blocks: draft.to_vec(),
                    auto_submit: false,
                })
                .filter(|initial_content| match initial_content {
                    AgentInitialContent::ContentBlock { blocks, .. } => !blocks.is_empty(),
                    _ => true,
                })
                .or_else(|| {
                    let text = thread_view.read(cx).message_editor.read(cx).text(cx);
                    if text.trim().is_empty() {
                        None
                    } else {
                        Some(AgentInitialContent::ContentBlock {
                            blocks: vec![acp::ContentBlock::Text(acp::TextContent::new(text))],
                            auto_submit: false,
                        })
                    }
                })
        })
    }

    /// 从源工作区初始化面板
    fn source_panel_initialization(
        source_workspace: &WeakEntity<Workspace>,
        cx: &App,
    ) -> Option<(Agent, AgentInitialContent)> {
        let source_workspace = source_workspace.upgrade()?;
        let source_panel = source_workspace.read(cx).panel::<AgentPanel>(cx)?;
        let source_panel = source_panel.read(cx);
        let initial_content = source_panel.active_initial_content(cx)?;
        let agent = if source_panel.project.read(cx).is_via_collab() {
            Agent::NativeAgent
        } else {
            source_panel.selected_agent.clone()
        };
        Some((agent, initial_content))
    }

    /// 如需从源工作区初始化面板
    pub fn initialize_from_source_workspace_if_needed(
        &mut self,
        source_workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.destination_has_meaningful_state(cx) {
            return false;
        }

        let Some((agent, initial_content)) =
            Self::source_panel_initialization(&source_workspace, cx)
        else {
            return false;
        };

        let thread = self.create_agent_thread(
            agent,
            None,
            None,
            None,
            Some(initial_content),
            "agent_panel",
            window,
            cx,
        );
        self.draft_thread = Some(thread.conversation_view.clone());
        self.observe_draft_editor(&thread.conversation_view, cx);
        self.set_base_view(thread.into(), false, window, cx);
        true
    }

    /// 渲染标题视图
    fn render_title_view(&self, _window: &mut Window, cx: &Context<Self>) -> AnyElement {
        let content = match self.visible_surface() {
            VisibleSurface::AgentThread(conversation_view) => {
                let server_view_ref = conversation_view.read(cx);
                let native_thread = server_view_ref.as_native_thread(cx);
                let is_generating_title = native_thread
                    .as_ref()
                    .is_some_and(|thread| thread.read(cx).is_generating_title());
                let title_generation_failed = native_thread
                    .as_ref()
                    .is_some_and(|thread| thread.read(cx).has_failed_title_generation());

                if let Some(title_editor) = server_view_ref
                    .root_thread_view()
                    .map(|r| r.read(cx).title_editor.clone())
                {
                    if is_generating_title {
                        Label::new(DEFAULT_THREAD_TITLE)
                            .color(Color::Muted)
                            .truncate()
                            .with_animation(
                                "generating_title",
                                Animation::new(Duration::from_secs(2))
                                    .repeat()
                                    .with_easing(pulsating_between(0.4, 0.8)),
                                |label, delta| label.alpha(delta),
                            )
                            .into_any_element()
                    } else {
                        let editable_title = div()
                            .flex_1()
                            .on_action({
                                let conversation_view = conversation_view.downgrade();
                                move |_: &menu::Confirm, window, cx| {
                                    if let Some(conversation_view) = conversation_view.upgrade() {
                                        conversation_view.focus_handle(cx).focus(window, cx);
                                    }
                                }
                            })
                            .on_action({
                                let conversation_view = conversation_view.downgrade();
                                move |_: &editor::actions::Cancel, window, cx| {
                                    if let Some(conversation_view) = conversation_view.upgrade() {
                                        conversation_view.focus_handle(cx).focus(window, cx);
                                    }
                                }
                            })
                            .child(title_editor);

                        if title_generation_failed {
                            h_flex()
                                .w_full()
                                .gap_1()
                                .items_center()
                                .child(editable_title)
                                .child(
                                    IconButton::new("retry-thread-title", IconName::XCircle)
                                        .icon_color(Color::Error)
                                        .icon_size(IconSize::Small)
                                        .tooltip(Tooltip::text("标题生成失败，重试"))
                                        .on_click({
                                            let conversation_view = conversation_view.clone();
                                            move |_event, _window, cx| {
                                                Self::handle_regenerate_thread_title(
                                                    conversation_view.clone(),
                                                    cx,
                                                );
                                            }
                                        }),
                                )
                                .into_any_element()
                        } else {
                            editable_title.w_full().into_any_element()
                        }
                    }
                } else {
                    Label::new(conversation_view.read(cx).title(cx))
                        .color(Color::Muted)
                        .truncate()
                        .into_any_element()
                }
            }
            VisibleSurface::Configuration(_) => {
                Label::new("设置").truncate().into_any_element()
            }
            VisibleSurface::Uninitialized => Label::new("智能体").truncate().into_any_element(),
        };

        h_flex()
            .key_context("TitleEditor")
            .id("TitleEditor")
            .flex_grow()
            .w_full()
            .max_w_full()
            .overflow_x_scroll()
            .child(content)
            .into_any()
    }

    /// 处理重新生成线程标题
    fn handle_regenerate_thread_title(conversation_view: Entity<ConversationView>, cx: &mut App) {
        conversation_view.update(cx, |conversation_view, cx| {
            if let Some(thread) = conversation_view.as_native_thread(cx) {
                thread.update(cx, |thread, cx| {
                    if !thread.is_generating_title() {
                        thread.generate_title(cx);
                        cx.notify();
                    }
                });
            }
        });
    }

    /// 渲染面板选项菜单
    fn render_panel_options_menu(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let focus_handle = self.focus_handle(cx);

        let conversation_view = match &self.base_view {
            BaseView::AgentThread { conversation_view } => Some(conversation_view.clone()),
            _ => None,
        };

        let can_regenerate_thread_title =
            conversation_view.as_ref().is_some_and(|conversation_view| {
                let conversation_view = conversation_view.read(cx);
                conversation_view.has_user_submitted_prompt(cx)
                    && conversation_view.as_native_thread(cx).is_some()
            });

        let has_auth_methods = match &self.base_view {
            BaseView::AgentThread { conversation_view } => {
                conversation_view.read(cx).has_auth_methods()
            }
            _ => false,
        };

        PopoverMenu::new("agent-options-menu")
            .trigger_with_tooltip(
                IconButton::new("agent-options-menu", IconName::Ellipsis)
                    .icon_size(IconSize::Small),
                {
                    let focus_handle = focus_handle.clone();
                    move |_window, cx| {
                        Tooltip::for_action_in(
                            "切换智能体菜单",
                            &ToggleOptionsMenu,
                            &focus_handle,
                            cx,
                        )
                    }
                },
            )
            .anchor(Anchor::TopRight)
            .with_handle(self.agent_panel_menu_handle.clone())
            .menu({
                move |window, cx| {
                    Some(ContextMenu::build(window, cx, |mut menu, _window, _| {
                        menu = menu.context(focus_handle.clone());

                        if can_regenerate_thread_title {
                            menu = menu.header("当前线程");

                            if let Some(conversation_view) = conversation_view.as_ref() {
                                menu = menu
                                    .entry("重新生成线程标题", None, {
                                        let conversation_view = conversation_view.clone();
                                        move |_, cx| {
                                            Self::handle_regenerate_thread_title(
                                                conversation_view.clone(),
                                                cx,
                                            );
                                        }
                                    })
                                    .separator();
                            }
                        }

                        menu = menu
                            .header("MCP服务器")
                            .action(
                                "查看服务器扩展",
                                Box::new(zed_actions::Extensions {
                                    category_filter: Some(
                                        zed_actions::ExtensionCategoryFilter::ContextServers,
                                    ),
                                    id: None,
                                }),
                            )
                            .action("添加自定义服务器…", Box::new(AddContextServer))
                            .separator()
                            .action("规则", Box::new(OpenRulesLibrary::default()))
                            .action("配置文件", Box::new(ManageProfiles::default()))
                            .action("设置", Box::new(OpenSettings))
                            .separator()
                            .action("切换线程侧边栏", Box::new(ToggleWorkspaceSidebar));

                        if has_auth_methods {
                            menu = menu.action("重新认证", Box::new(ReauthenticateAgent))
                        }

                        menu
                    }))
                }
            })
    }

    /// 渲染工具栏返回按钮
    fn render_toolbar_back_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let focus_handle = self.focus_handle(cx);

        IconButton::new("go-back", IconName::ArrowLeft)
            .icon_size(IconSize::Small)
            .on_click(cx.listener(|this, _, window, cx| {
                this.go_back(&workspace::GoBack, window, cx);
            }))
            .tooltip({
                move |_window, cx| {
                    Tooltip::for_action_in("返回", &workspace::GoBack, &focus_handle, cx)
                }
            })
    }

    /// 渲染工具栏
    fn render_toolbar(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let agent_server_store = self.project.read(cx).agent_server_store().clone();

        let focus_handle = self.focus_handle(cx);

        let (selected_agent_custom_icon, selected_agent_label) =
            if let Agent::Custom { id, .. } = &self.selected_agent {
                let store = agent_server_store.read(cx);
                let icon = store.agent_icon(&id);

                let label = store
                    .agent_display_name(&id)
                    .unwrap_or_else(|| self.selected_agent.label());
                (icon, label)
            } else {
                (None, self.selected_agent.label())
            };

        let active_thread = match &self.base_view {
            BaseView::AgentThread { conversation_view } => {
                conversation_view.read(cx).as_native_thread(cx)
            }
            BaseView::Uninitialized => None,
        };

        let new_thread_menu_builder: Rc<
            dyn Fn(&mut Window, &mut App) -> Option<Entity<ContextMenu>>,
        > = {
            let selected_agent = self.selected_agent.clone();
            let is_agent_selected = move |agent: Agent| selected_agent == agent;

            let workspace = self.workspace.clone();
            let is_via_collab = workspace
                .update(cx, |workspace, cx| {
                    workspace.project().read(cx).is_via_collab()
                })
                .unwrap_or_default();

            let focus_handle = focus_handle.clone();
            let agent_server_store = agent_server_store;

            Rc::new(move |window, cx| {
                let active_thread = active_thread.clone();
                Some(ContextMenu::build(window, cx, |menu, _window, cx| {
                    menu.context(focus_handle.clone())
                        .when_some(active_thread, |this, active_thread| {
                            let thread = active_thread.read(cx);

                            if !thread.is_empty() {
                                let session_id = thread.id().clone();
                                this.item(
                                    ContextMenuEntry::new("从摘要新建")
                                        .icon(IconName::ThreadFromSummary)
                                        .icon_color(Color::Muted)
                                        .handler(move |window, cx| {
                                            window.dispatch_action(
                                                Box::new(NewNativeAgentThreadFromSummary {
                                                    from_session_id: session_id.clone(),
                                                }),
                                                cx,
                                            );
                                        }),
                                )
                            } else {
                                this
                            }
                        })
                        .item(
                            ContextMenuEntry::new("Zed智能体")
                                .when(is_agent_selected(Agent::NativeAgent), |this| {
                                    this.action(Box::new(NewExternalAgentThread { agent: None }))
                                })
                                .icon(IconName::ZedAgent)
                                .icon_color(Color::Muted)
                                .handler({
                                    let workspace = workspace.clone();
                                    move |window, cx| {
                                        if let Some(workspace) = workspace.upgrade() {
                                            workspace.update(cx, |workspace, cx| {
                                                if let Some(panel) =
                                                    workspace.panel::<AgentPanel>(cx)
                                                {
                                                    panel.update(cx, |panel, cx| {
                                                        panel.new_external_agent_thread(
                                                            &NewExternalAgentThread {
                                                                agent: Some(Agent::NativeAgent),
                                                            },
                                                            window,
                                                            cx,
                                                        );
                                                    });
                                                }
                                            });
                                        }
                                    }
                                }),
                        )
                        .map(|mut menu| {
                            let agent_server_store = agent_server_store.read(cx);
                            let registry_store = project::AgentRegistryStore::try_global(cx);
                            let registry_store_ref = registry_store.as_ref().map(|s| s.read(cx));

                            struct AgentMenuItem {
                                id: AgentId,
                                display_name: SharedString,
                            }

                            let agent_items = agent_server_store
                                .external_agents()
                                .map(|agent_id| {
                                    let display_name = agent_server_store
                                        .agent_display_name(agent_id)
                                        .or_else(|| {
                                            registry_store_ref
                                                .as_ref()
                                                .and_then(|store| store.agent(agent_id))
                                                .map(|a| a.name().clone())
                                        })
                                        .unwrap_or_else(|| agent_id.0.clone());
                                    AgentMenuItem {
                                        id: agent_id.clone(),
                                        display_name,
                                    }
                                })
                                .sorted_unstable_by_key(|e| e.display_name.to_lowercase())
                                .collect::<Vec<_>>();

                            if !agent_items.is_empty() {
                                menu = menu.separator().header("外部智能体");
                            }
                            for item in &agent_items {
                                let mut entry = ContextMenuEntry::new(item.display_name.clone());

                                let icon_path =
                                    agent_server_store.agent_icon(&item.id).or_else(|| {
                                        registry_store_ref
                                            .as_ref()
                                            .and_then(|store| store.agent(&item.id))
                                            .and_then(|a| a.icon_path().cloned())
                                    });

                                if let Some(icon_path) = icon_path {
                                    entry = entry.custom_icon_svg(icon_path);
                                } else {
                                    entry = entry.icon(IconName::Sparkle);
                                }

                                entry = entry
                                    .when(
                                        is_agent_selected(Agent::Custom {
                                            id: item.id.clone(),
                                        }),
                                        |this| {
                                            this.action(Box::new(NewExternalAgentThread {
                                                agent: None,
                                            }))
                                        },
                                    )
                                    .icon_color(Color::Muted)
                                    .disabled(is_via_collab)
                                    .handler({
                                        let workspace = workspace.clone();
                                        let agent_id = item.id.clone();
                                        move |window, cx| {
                                            if let Some(workspace) = workspace.upgrade() {
                                                workspace.update(cx, |workspace, cx| {
                                                    if let Some(panel) =
                                                        workspace.panel::<AgentPanel>(cx)
                                                    {
                                                        panel.update(cx, |panel, cx| {
                                                            panel.new_external_agent_thread(
                                                                &NewExternalAgentThread {
                                                                    agent: Some(Agent::Custom {
                                                                        id: agent_id.clone(),
                                                                    }),
                                                                },
                                                                window,
                                                                cx,
                                                            );
                                                        });
                                                    }
                                                });
                                            }
                                        }
                                    });

                                menu = menu.item(entry);
                            }

                            menu
                        })
                        .separator()
                        .item(
                            ContextMenuEntry::new("添加更多智能体")
                                .icon(IconName::Plus)
                                .icon_color(Color::Muted)
                                .handler({
                                    move |window, cx| {
                                        window
                                            .dispatch_action(Box::new(zed_actions::AcpRegistry), cx)
                                    }
                                }),
                        )
                }))
            })
        };

        let is_thread_loading = self
            .active_conversation_view()
            .map(|thread| thread.read(cx).is_loading())
            .unwrap_or(false);

        let has_custom_icon = selected_agent_custom_icon.is_some();
        let selected_agent_custom_icon_for_button = selected_agent_custom_icon.clone();
        let selected_agent_builtin_icon = self.selected_agent.icon();
        let selected_agent_label_for_tooltip = selected_agent_label.clone();

        let selected_agent = div()
            .id("selected_agent_icon")
            .when_some(selected_agent_custom_icon, |this, icon_path| {
                this.px_1().child(
                    Icon::from_external_svg(icon_path)
                        .color(Color::Muted)
                        .size(IconSize::Small),
                )
            })
            .when(!has_custom_icon, |this| {
                this.when_some(selected_agent_builtin_icon, |this, icon| {
                    this.px_1().child(Icon::new(icon).color(Color::Muted))
                })
            })
            .tooltip(move |_, cx| {
                Tooltip::with_meta(
                    selected_agent_label_for_tooltip.clone(),
                    None,
                    "已选智能体",
                    cx,
                )
            });

        let selected_agent = if is_thread_loading {
            selected_agent
                .with_animation(
                    "pulsating-icon",
                    Animation::new(Duration::from_secs(1))
                        .repeat()
                        .with_easing(pulsating_between(0.2, 0.6)),
                    |icon, delta| icon.opacity(delta),
                )
                .into_any_element()
        } else {
            selected_agent.into_any_element()
        };

        let is_empty_state = !self.active_thread_has_messages(cx);

        let is_in_history_or_config = self.is_overlay_open();

        let is_full_screen = self.is_zoomed(window, cx);
        let full_screen_button = if is_full_screen {
            IconButton::new("disable-full-screen", IconName::Minimize)
                .icon_size(IconSize::Small)
                .tooltip(move |_, cx| Tooltip::for_action("退出全屏", &ToggleZoom, cx))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.toggle_zoom(&ToggleZoom, window, cx);
                }))
        } else {
            IconButton::new("enable-full-screen", IconName::Maximize)
                .icon_size(IconSize::Small)
                .tooltip(move |_, cx| Tooltip::for_action("进入全屏", &ToggleZoom, cx))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.toggle_zoom(&ToggleZoom, window, cx);
                }))
        };

        let use_v2_empty_toolbar = is_empty_state && !is_in_history_or_config;

        let max_content_width = AgentSettings::get_global(cx).max_content_width;

        let base_container = h_flex()
            .size_full()
            .when(!is_in_history_or_config, |this| {
                this.when_some(max_content_width, |this, max_w| this.max_w(max_w).mx_auto())
            })
            .flex_none()
            .justify_between()
            .gap_2();

        let toolbar_content = if use_v2_empty_toolbar {
            let (chevron_icon, icon_color, label_color) =
                if self.new_thread_menu_handle.is_deployed() {
                    (IconName::ChevronUp, Color::Accent, Color::Accent)
                } else {
                    (IconName::ChevronDown, Color::Muted, Color::Default)
                };

            let agent_icon = if let Some(icon_path) = selected_agent_custom_icon_for_button {
                Icon::from_external_svg(icon_path)
                    .size(IconSize::Small)
                    .color(icon_color)
            } else {
                let icon_name = selected_agent_builtin_icon.unwrap_or(IconName::ZedAgent);
                Icon::new(icon_name).size(IconSize::Small).color(icon_color)
            };

            let agent_selector_button = Button::new("agent-selector-trigger", selected_agent_label)
                .start_icon(agent_icon)
                .color(label_color)
                .end_icon(
                    Icon::new(chevron_icon)
                        .color(icon_color)
                        .size(IconSize::XSmall),
                );

            let agent_selector_menu = PopoverMenu::new("new_thread_menu")
                .trigger_with_tooltip(agent_selector_button, {
                    move |_window, cx| {
                        Tooltip::for_action_in(
                            "新建线程…",
                            &ToggleNewThreadMenu,
                            &focus_handle,
                            cx,
                        )
                    }
                })
                .menu({
                    let builder = new_thread_menu_builder.clone();
                    move |window, cx| builder(window, cx)
                })
                .with_handle(self.new_thread_menu_handle.clone())
                .anchor(Anchor::TopLeft)
                .offset(gpui::Point {
                    x: px(1.0),
                    y: px(1.0),
                });

            base_container
                .child(
                    h_flex()
                        .size_full()
                        .gap(DynamicSpacing::Base04.rems(cx))
                        .pl(DynamicSpacing::Base04.rems(cx))
                        .child(agent_selector_menu),
                )
                .child(
                    h_flex()
                        .h_full()
                        .flex_none()
                        .gap_1()
                        .pl_1()
                        .pr_1()
                        .child(full_screen_button)
                        .child(self.render_panel_options_menu(window, cx)),
                )
                .into_any_element()
        } else {
            let new_thread_menu = PopoverMenu::new("new_thread_menu")
                .trigger_with_tooltip(
                    IconButton::new("new_thread_menu_btn", IconName::Plus)
                        .icon_size(IconSize::Small),
                    {
                        move |_window, cx| {
                            Tooltip::for_action_in(
                                "新建线程…",
                                &ToggleNewThreadMenu,
                                &focus_handle,
                                cx,
                            )
                        }
                    },
                )
                .anchor(Anchor::TopRight)
                .with_handle(self.new_thread_menu_handle.clone())
                .menu(move |window, cx| new_thread_menu_builder(window, cx));

            base_container
                .child(
                    h_flex()
                        .size_full()
                        .gap(DynamicSpacing::Base04.rems(cx))
                        .pl(DynamicSpacing::Base04.rems(cx))
                        .child(if self.is_overlay_open() {
                            self.render_toolbar_back_button(cx).into_any_element()
                        } else {
                            selected_agent.into_any_element()
                        })
                        .child(self.render_title_view(window, cx)),
                )
                .child(
                    h_flex()
                        .h_full()
                        .flex_none()
                        .gap_1()
                        .pl_1()
                        .pr_1()
                        .child(new_thread_menu)
                        .child(full_screen_button)
                        .child(self.render_panel_options_menu(window, cx)),
                )
                .into_any_element()
        };

        h_flex()
            .id("agent-panel-toolbar")
            .h(Tab::container_height(cx))
            .flex_shrink_0()
            .max_w_full()
            .bg(cx.theme().colors().tab_bar_background)
            .border_b_1()
            .border_color(cx.theme().colors().border)
            .child(toolbar_content)
    }

    /// 是否显示试用结束推广
    fn should_render_trial_end_upsell(&self, cx: &mut Context<Self>) -> bool {
        if TrialEndUpsell::dismissed(cx) {
            return false;
        }

        match &self.base_view {
            BaseView::AgentThread { .. } => {
                if LanguageModelRegistry::global(cx)
                    .read(cx)
                    .default_model()
                    .is_some_and(|model| {
                        model.provider.id() != language_model::ZED_CLOUD_PROVIDER_ID
                    })
                {
                    return false;
                }
            }
            BaseView::Uninitialized => {
                return false;
            }
        }

        let plan = self.user_store.read(cx).plan();
        let has_previous_trial = self.user_store.read(cx).trial_started_at().is_some();

        plan.is_some_and(|plan| plan == Plan::ZedFree) && has_previous_trial
    }

    /// 关闭新用户引导
    fn dismiss_ai_onboarding(&mut self, cx: &mut Context<Self>) {
        self.new_user_onboarding_upsell_dismissed
            .store(true, Ordering::Release);
        OnboardingUpsell::set_dismissed(true, cx);
        cx.notify();
    }

    /// 是否显示新用户引导
    fn should_render_new_user_onboarding(&mut self, cx: &mut Context<Self>) -> bool {
        if self
            .new_user_onboarding_upsell_dismissed
            .load(Ordering::Acquire)
        {
            return false;
        }

        let user_store = self.user_store.read(cx);

        if user_store.plan().is_some_and(|plan| plan == Plan::ZedPro)
            && user_store
                .subscription_period()
                .and_then(|period| period.0.checked_add_days(chrono::Days::new(1)))
                .is_some_and(|date| date < chrono::Utc::now())
        {
            if !self
                .new_user_onboarding_upsell_dismissed
                .load(Ordering::Acquire)
            {
                self.dismiss_ai_onboarding(cx);
            }
            return false;
        }

        let has_configured_non_zed_providers = LanguageModelRegistry::read_global(cx)
            .visible_providers()
            .iter()
            .any(|provider| {
                provider.is_authenticated(cx)
                    && provider.id() != language_model::ZED_CLOUD_PROVIDER_ID
            });

        match &self.base_view {
            BaseView::Uninitialized => false,
            BaseView::AgentThread { conversation_view } => {
                if conversation_view.read(cx).as_native_thread(cx).is_some() {
                    let history_is_empty = ThreadStore::global(cx).read(cx).is_empty();
                    history_is_empty || !has_configured_non_zed_providers
                } else {
                    false
                }
            }
        }
    }

    /// 渲染新用户引导
    fn render_new_user_onboarding(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        if !self.should_render_new_user_onboarding(cx) {
            return None;
        }

        Some(
            div()
                .bg(cx.theme().colors().editor_background)
                .child(self.new_user_onboarding.clone()),
        )
    }

    /// 渲染试用结束推广
    fn render_trial_end_upsell(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        if !self.should_render_trial_end_upsell(cx) {
            return None;
        }

        Some(
            v_flex()
                .absolute()
                .inset_0()
                .size_full()
                .bg(cx.theme().colors().panel_background)
                .opacity(0.85)
                .block_mouse_except_scroll()
                .child(EndTrialUpsell::new(Arc::new({
                    let this = cx.entity();
                    move |_, cx| {
                        this.update(cx, |_this, cx| {
                            TrialEndUpsell::set_dismissed(true, cx);
                            cx.notify();
                        });
                    }
                }))),
        )
    }

    /// 渲染拖拽目标区域
    fn render_drag_target(&self, cx: &Context<Self>) -> Div {
        let is_local = self.project.read(cx).is_local();
        div()
            .invisible()
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .left_0()
            .bg(cx.theme().colors().drop_target_background)
            .drag_over::<DraggedTab>(|this, _, _, _| this.visible())
            .drag_over::<DraggedSelection>(|this, _, _, _| this.visible())
            .when(is_local, |this| {
                this.drag_over::<ExternalPaths>(|this, _, _, _| this.visible())
            })
            .on_drop(cx.listener(move |this, tab: &DraggedTab, window, cx| {
                let item = tab.pane.read(cx).item_for_index(tab.ix);
                let project_paths = item
                    .and_then(|item| item.project_path(cx))
                    .into_iter()
                    .collect::<Vec<_>>();
                this.handle_drop(project_paths, vec![], window, cx);
            }))
            .on_drop(
                cx.listener(move |this, selection: &DraggedSelection, window, cx| {
                    let project_paths = selection
                        .items()
                        .filter_map(|item| this.project.read(cx).path_for_entry(item.entry_id, cx))
                        .collect::<Vec<_>>();
                    this.handle_drop(project_paths, vec![], window, cx);
                }),
            )
            .on_drop(cx.listener(move |this, paths: &ExternalPaths, window, cx| {
                let tasks = paths
                    .paths()
                    .iter()
                    .map(|path| {
                        Workspace::project_path_for_path(this.project.clone(), path, false, cx)
                    })
                    .collect::<Vec<_>>();
                cx.spawn_in(window, async move |this, cx| {
                    let mut paths = vec![];
                    let mut added_worktrees = vec![];
                    let opened_paths = futures::future::join_all(tasks).await;
                    for entry in opened_paths {
                        if let Some((worktree, project_path)) = entry.log_err() {
                            added_worktrees.push(worktree);
                            paths.push(project_path);
                        }
                    }
                    this.update_in(cx, |this, window, cx| {
                        this.handle_drop(paths, added_worktrees, window, cx);
                    })
                    .ok();
                })
                .detach();
            }))
    }

    /// 处理拖拽放置
    fn handle_drop(
        &mut self,
        paths: Vec<ProjectPath>,
        added_worktrees: Vec<Entity<Worktree>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match &self.base_view {
            BaseView::AgentThread { conversation_view } => {
                conversation_view.update(cx, |conversation_view, cx| {
                    conversation_view.insert_dragged_files(paths, added_worktrees, window, cx);
                });
            }
            BaseView::Uninitialized => {}
        }
    }

    /// 渲染工作区信任提示
    fn render_workspace_trust_message(&self, cx: &Context<Self>) -> Option<impl IntoElement> {
        if !self.show_trust_workspace_message {
            return None;
        }

        let description = "为保护你的系统，第三方代码（如MCP服务器）将在你标记此工作区为安全后运行。";

        Some(
            Callout::new()
                .icon(IconName::Warning)
                .severity(Severity::Warning)
                .border_position(ui::BorderPosition::Bottom)
                .title("你当前处于受限模式")
                .description(description)
                .actions_slot(
                    Button::new("open-trust-modal", "配置项目信任")
                        .label_size(LabelSize::Small)
                        .style(ButtonStyle::Outlined)
                        .on_click({
                            cx.listener(move |this, _, window, cx| {
                                this.workspace
                                    .update(cx, |workspace, cx| {
                                        workspace
                                            .show_worktree_trust_security_modal(true, window, cx)
                                    })
                                    .log_err();
                            })
                        }),
                ),
        )
    }

    /// 快捷键上下文
    fn key_context(&self) -> KeyContext {
        let mut key_context = KeyContext::new_with_defaults();
        key_context.add("AgentPanel");
        match &self.base_view {
            BaseView::AgentThread { .. } => key_context.add("acp_thread"),
            BaseView::Uninitialized => {}
        }
        key_context
    }
}

impl Render for AgentPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // WARNING: Changes to this element hierarchy can have
        // non-obvious implications to the layout of children.
        //
        // If you need to change it, please confirm:
        // - The message editor expands (cmd-option-esc) correctly
        // - When expanded, the buttons at the bottom of the panel are displayed correctly
        // - Font size works as expected and can be changed with cmd-+/cmd-
        // - Scrolling in all views works as expected
        // - Files can be dropped into the panel
        let content = v_flex()
            .relative()
            .size_full()
            .justify_between()
            .key_context(self.key_context())
            .on_action(cx.listener(|this, action: &NewThread, window, cx| {
                this.new_thread(action, window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| {
                this.open_configuration(window, cx);
            }))
            .on_action(cx.listener(Self::open_active_thread_as_markdown))
            .on_action(cx.listener(Self::deploy_rules_library))
            .on_action(cx.listener(Self::go_back))
            .on_action(cx.listener(Self::toggle_options_menu))
            .on_action(cx.listener(Self::increase_font_size))
            .on_action(cx.listener(Self::decrease_font_size))
            .on_action(cx.listener(Self::reset_font_size))
            .on_action(cx.listener(Self::toggle_zoom))
            .on_action(cx.listener(|this, _: &ReauthenticateAgent, window, cx| {
                if let Some(conversation_view) = this.active_conversation_view() {
                    conversation_view.update(cx, |conversation_view, cx| {
                        conversation_view.reauthenticate(window, cx)
                    })
                }
            }))
            .child(self.render_toolbar(window, cx))
            .children(self.render_workspace_trust_message(cx))
            .children(self.render_new_user_onboarding(window, cx))
            .map(|parent| match self.visible_surface() {
                VisibleSurface::Uninitialized => parent,
                VisibleSurface::AgentThread(conversation_view) => parent
                    .child(conversation_view.clone())
                    .child(self.render_drag_target(cx)),
                VisibleSurface::Configuration(configuration) => {
                    parent.children(configuration.cloned())
                }
            })
            .children(self.render_trial_end_upsell(window, cx));

        match self.visible_font_size() {
            WhichFontSize::AgentFont => {
                WithRemSize::new(ThemeSettings::get_global(cx).agent_ui_font_size(cx))
                    .size_full()
                    .child(content)
                    .into_any()
            }
            _ => content.into_any(),
        }
    }
}

struct PromptLibraryInlineAssist {
    workspace: WeakEntity<Workspace>,
}

impl PromptLibraryInlineAssist {
    pub fn new(workspace: WeakEntity<Workspace>) -> Self {
        Self { workspace }
    }
}

impl rules_library::InlineAssistDelegate for PromptLibraryInlineAssist {
    fn assist(
        &self,
        prompt_editor: &Entity<Editor>,
        initial_prompt: Option<String>,
        window: &mut Window,
        cx: &mut Context<RulesLibrary>,
    ) {
        InlineAssistant::update_global(cx, |assistant, cx| {
            let Some(workspace) = self.workspace.upgrade() else {
                return;
            };
            let Some(panel) = workspace.read(cx).panel::<AgentPanel>(cx) else {
                return;
            };
            let project = workspace.read(cx).project().downgrade();
            let panel = panel.read(cx);
            let thread_store = panel.thread_store().clone();
            assistant.assist(
                prompt_editor,
                self.workspace.clone(),
                project,
                thread_store,
                None,
                initial_prompt,
                window,
                cx,
            );
        })
    }

    fn focus_agent_panel(
        &self,
        workspace: &mut Workspace,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> bool {
        workspace.focus_panel::<AgentPanel>(window, cx).is_some()
    }
}

struct OnboardingUpsell;

impl Dismissable for OnboardingUpsell {
    const KEY: &'static str = "dismissed-trial-upsell";
}

struct TrialEndUpsell;

impl Dismissable for TrialEndUpsell {
    const KEY: &'static str = "dismissed-trial-end-upsell";
}

/// Test-only helper methods
#[cfg(any(test, feature = "test-support"))]
impl AgentPanel {
    pub fn test_new(workspace: &Workspace, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new(workspace, None, window, cx)
    }

    /// Opens an external thread using an arbitrary AgentServer.
    ///
    /// This is a test-only helper that allows visual tests and integration tests
    /// to inject a stub server without modifying production code paths.
    /// Not compiled into production builds.
    pub fn open_external_thread_with_server(
        &mut self,
        server: Rc<dyn AgentServer>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ext_agent = Agent::Custom {
            id: server.agent_id(),
        };

        let thread = self.create_agent_thread_with_server(
            ext_agent,
            Some(server),
            None,
            None,
            None,
            None,
            "agent_panel",
            window,
            cx,
        );
        self.set_base_view(thread.into(), true, window, cx);
    }

    /// Opens a restored external thread with an arbitrary AgentServer and
    /// a specific `resume_session_id` — as if we just restored from the KVP.
    ///
    /// Test-only helper. Not compiled into production builds.
    pub fn open_restored_thread_with_server(
        &mut self,
        server: Rc<dyn AgentServer>,
        resume_session_id: acp::SessionId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ext_agent = Agent::Custom {
            id: server.agent_id(),
        };

        let thread = self.create_agent_thread_with_server(
            ext_agent,
            Some(server),
            Some(resume_session_id),
            None,
            None,
            None,
            "agent_panel",
            window,
            cx,
        );
        self.set_base_view(thread.into(), true, window, cx);
    }

    /// Returns the currently active thread view, if any.
    ///
    /// This is a test-only accessor that exposes the private `active_thread_view()`
    /// method for test assertions. Not compiled into production builds.
    pub fn active_thread_view_for_tests(&self) -> Option<&Entity<ConversationView>> {
        self.active_conversation_view()
    }

    /// Creates a draft thread using a stub server and sets it as the active view.
    #[cfg(any(test, feature = "test-support"))]
    pub fn open_draft_with_server(
        &mut self,
        server: Rc<dyn AgentServer>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ext_agent = Agent::Custom {
            id: server.agent_id(),
        };
        let thread = self.create_agent_thread_with_server(
            ext_agent,
            Some(server),
            None,
            None,
            None,
            None,
            "agent_panel",
            window,
            cx,
        );
        self.draft_thread = Some(thread.conversation_view.clone());
        self.set_base_view(thread.into(), true, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NewWorktreeBranchTarget;
    use crate::conversation_view::tests::{StubAgentServer, init_test};
    use crate::test_support::{
        active_session_id, active_thread_id, open_thread_with_connection,
        open_thread_with_custom_connection, send_message,
    };
    use acp_thread::{AgentConnection, StubAgentConnection, ThreadStatus, UserMessageId};
    use action_log::ActionLog;
    use anyhow::{Result, anyhow};
    use feature_flags::FeatureFlagAppExt;
    use fs::FakeFs;
    use gpui::{App, TestAppContext, VisualTestContext};
    use parking_lot::Mutex;
    use project::Project;
    use std::any::Any;

    use serde_json::json;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Instant;
    use workspace::MultiWorkspace;

    #[derive(Clone, Default)]
    struct SessionTrackingConnection {
        next_session_number: Arc<Mutex<usize>>,
        sessions: Arc<Mutex<HashSet<acp::SessionId>>>,
    }

    impl SessionTrackingConnection {
        fn new() -> Self {
            Self::default()
        }

        fn create_session(
            self: Rc<Self>,
            session_id: acp::SessionId,
            project: Entity<Project>,
            work_dirs: PathList,
            title: Option<SharedString>,
            cx: &mut App,
        ) -> Entity<AcpThread> {
            self.sessions.lock().insert(session_id.clone());

            let action_log = cx.new(|_| ActionLog::new(project.clone()));
            cx.new(|cx| {
                AcpThread::new(
                    None,
                    title,
                    Some(work_dirs),
                    self,
                    project,
                    action_log,
                    session_id,
                    watch::Receiver::constant(
                        acp::PromptCapabilities::new()
                            .image(true)
                            .audio(true)
                            .embedded_context(true),
                    ),
                    cx,
                )
            })
        }
    }

    impl AgentConnection for SessionTrackingConnection {
        fn agent_id(&self) -> AgentId {
            agent::ZED_AGENT_ID.clone()
        }

        fn telemetry_id(&self) -> SharedString {
            "会话跟踪测试".into()
        }

        fn new_session(
            self: Rc<Self>,
            project: Entity<Project>,
            work_dirs: PathList,
            cx: &mut App,
        ) -> Task<Result<Entity<AcpThread>>> {
            let session_id = {
                let mut next_session_number = self.next_session_number.lock();
                let session_id = acp::SessionId::new(format!(
                    "session-tracking-session-{}",
                    *next_session_number
                ));
                *next_session_number += 1;
                session_id
            };
            let thread = self.create_session(session_id, project, work_dirs, None, cx);
            Task::ready(Ok(thread))
        }

        fn supports_load_session(&self) -> bool {
            true
        }

        /// 加载会话
        fn load_session(
            self: Rc<Self>,
            session_id: acp::SessionId,
            project: Entity<Project>,
            work_dirs: PathList,
            title: Option<SharedString>,
            cx: &mut App,
        ) -> Task<Result<Entity<AcpThread>>> {
            let thread = self.create_session(session_id, project, work_dirs, title, cx);
            thread.update(cx, |thread, cx| {
                thread
                    .handle_session_update(
                        acp::SessionUpdate::UserMessageChunk(acp::ContentChunk::new(
                            "已恢复用户消息".into(),
                        )),
                        cx,
                    )
                    .expect("应应用已恢复的用户消息");
                thread
                    .handle_session_update(
                        acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
                            "已恢复助手消息".into(),
                        )),
                        cx,
                    )
                    .expect("应应用已恢复的助手消息");
            });
            Task::ready(Ok(thread))
        }

        fn supports_close_session(&self) -> bool {
            true
        }

        fn close_session(
            self: Rc<Self>,
            session_id: &acp::SessionId,
            _cx: &mut App,
        ) -> Task<Result<()>> {
            self.sessions.lock().remove(session_id);
            Task::ready(Ok(()))
        }

        fn auth_methods(&self) -> &[acp::AuthMethod] {
            &[]
        }

        fn authenticate(&self, _method_id: acp::AuthMethodId, _cx: &mut App) -> Task<Result<()>> {
            Task::ready(Ok(()))
        }

        fn prompt(
            &self,
            _id: UserMessageId,
            params: acp::PromptRequest,
            _cx: &mut App,
        ) -> Task<Result<acp::PromptResponse>> {
            if !self.sessions.lock().contains(&params.session_id) {
                return Task::ready(Err(anyhow!("未找到会话")));
            }

            Task::ready(Ok(acp::PromptResponse::new(acp::StopReason::EndTurn)))
        }

        fn cancel(&self, _session_id: &acp::SessionId, _cx: &mut App) {}

        fn into_any(self: Rc<Self>) -> Rc<dyn Any> {
            self
        }
    }

    #[gpui::test]
    async fn test_active_thread_serialize_and_load_round_trip(cx: &mut TestAppContext) {
        init_test(cx);
        cx.update(|cx| {
            agent::ThreadStore::init_global(cx);
            language_model::LanguageModelRegistry::test(cx);
        });

        // Create a MultiWorkspace window with two workspaces.
        let fs = FakeFs::new(cx.executor());
        fs.insert_tree("/project_a", json!({ "file.txt": "" }))
            .await;
        let project_a = Project::test(fs.clone(), [Path::new("/project_a")], cx).await;
        let project_b = Project::test(fs, [], cx).await;

        let multi_workspace =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project_a.clone(), window, cx));

        let workspace_a = multi_workspace
            .read_with(cx, |multi_workspace, _cx| {
                multi_workspace.workspace().clone()
            })
            .unwrap();

        let workspace_b = multi_workspace
            .update(cx, |multi_workspace, window, cx| {
                multi_workspace.test_add_workspace(project_b.clone(), window, cx)
            })
            .unwrap();

        workspace_a.update(cx, |workspace, _cx| {
            workspace.set_random_database_id();
        });
        workspace_b.update(cx, |workspace, _cx| {
            workspace.set_random_database_id();
        });

        let cx = &mut VisualTestContext::from_window(multi_workspace.into(), cx);

        // Set up workspace A: with an active thread.
        let panel_a = workspace_a.update_in(cx, |workspace, window, cx| {
            cx.new(|cx| AgentPanel::new(workspace, None, window, cx))
        });

        panel_a.update_in(cx, |panel, window, cx| {
            panel.open_external_thread_with_server(
                Rc::new(StubAgentServer::default_response()),
                window,
                cx,
            );
        });

        cx.run_until_parked();

        panel_a.read_with(cx, |panel, cx| {
            assert!(
                panel.active_agent_thread(cx).is_some(),
                "连接后工作区A应存在活动线程"
            );
        });

        send_message(&panel_a, cx);

        let agent_type_a = panel_a.read_with(cx, |panel, _cx| panel.selected_agent.clone());

        // Set up workspace B: ClaudeCode, no active thread.
        let panel_b = workspace_b.update_in(cx, |workspace, window, cx| {
            cx.new(|cx| AgentPanel::new(workspace, None, window, cx))
        });

        panel_b.update(cx, |panel, _cx| {
            panel.selected_agent = Agent::Custom {
                id: "claude-acp".into(),
            };
        });

        // Serialize both panels.
        panel_a.update(cx, |panel, cx| panel.serialize(cx));
        panel_b.update(cx, |panel, cx| panel.serialize(cx));
        cx.run_until_parked();

        // Load fresh panels for each workspace and verify independent state.
        let async_cx = cx.update(|window, cx| window.to_async(cx));
        let loaded_a = AgentPanel::load(workspace_a.downgrade(), async_cx)
            .await
            .expect("面板A加载应成功");
        cx.run_until_parked();

        let async_cx = cx.update(|window, cx| window.to_async(cx));
        let loaded_b = AgentPanel::load(workspace_b.downgrade(), async_cx)
            .await
            .expect("面板B加载应成功");
        cx.run_until_parked();

        // Workspace A should restore its thread and agent type
        loaded_a.read_with(cx, |panel, _cx| {
            assert_eq!(
                panel.selected_agent, agent_type_a,
                "工作区A的智能体类型应被恢复"
            );
            assert!(
                panel.active_conversation_view().is_some(),
                "工作区A应恢复其活动线程"
            );
        });

        // Workspace B should restore its own agent type but have no active thread.
        loaded_b.read_with(cx, |panel, _cx| {
            assert_eq!(
                panel.selected_agent,
                Agent::Custom {
                    id: "claude-acp".into()
                },
                "工作区B的智能体类型应被恢复"
            );
            assert!(
                panel.active_conversation_view().is_none(),
                "工作区B在无历史对话时应无活动线程"
            );
        });
    }

    #[gpui::test]
    async fn test_non_native_thread_without_metadata_is_not_restored(cx: &mut TestAppContext) {
        init_test(cx);
        cx.update(|cx| {
            agent::ThreadStore::init_global(cx);
            language_model::LanguageModelRegistry::test(cx);
        });

        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;

        let multi_workspace =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));

        let workspace = multi_workspace
            .read_with(cx, |multi_workspace, _cx| {
                multi_workspace.workspace().clone()
            })
            .unwrap();

        workspace.update(cx, |workspace, _cx| {
            workspace.set_random_database_id();
        });

        let cx = &mut VisualTestContext::from_window(multi_workspace.into(), cx);

        let panel = workspace.update_in(cx, |workspace, window, cx| {
            cx.new(|cx| AgentPanel::new(workspace, None, window, cx))
        });

        panel.update_in(cx, |panel, window, cx| {
            panel.open_external_thread_with_server(
                Rc::new(StubAgentServer::default_response()),
                window,
                cx,
            );
        });

        cx.run_until_parked();

        panel.read_with(cx, |panel, cx| {
            assert!(
                panel.active_agent_thread(cx).is_some(),
                "连接后应存在活动线程"
            );
        });

        // Serialize without ever sending a message, so no thread metadata exists.
        panel.update(cx, |panel, cx| panel.serialize(cx));
        cx.run_until_parked();

        let async_cx = cx.update(|window, cx| window.to_async(cx));
        let loaded = AgentPanel::load(workspace.downgrade(), async_cx)
            .await
            .expect("面板加载应成功");
        cx.run_until_parked();

        loaded.read_with(cx, |panel, _cx| {
            assert!(
                panel.active_conversation_view().is_none(),
                "无元数据的线程不应被恢复；面板应无活动线程"
            );
        });
    }

    #[gpui::test]
    async fn test_serialize_preserves_session_id_in_load_error(cx: &mut TestAppContext) {
        use crate::conversation_view::tests::FlakyAgentServer;
        use crate::thread_metadata_store::{ThreadId, ThreadMetadata};
        use chrono::Utc;
        use project::{AgentId as ProjectAgentId, WorktreePaths};

        init_test(cx);
        cx.update(|cx| {
            agent::ThreadStore::init_global(cx);
            language_model::LanguageModelRegistry::test(cx);
        });

        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;

        let multi_workspace =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
        let workspace = multi_workspace
            .read_with(cx, |mw, _cx| mw.workspace().clone())
            .unwrap();
        workspace.update(cx, |workspace, _cx| {
            workspace.set_random_database_id();
        });
        let workspace_id = workspace
            .read_with(cx, |workspace, _cx| workspace.database_id())
            .expect("工作区应拥有数据库ID");

        let cx = &mut VisualTestContext::from_window(multi_workspace.into(), cx);

        // Simulate a previous run that persisted metadata for this session.
        let resume_session_id = acp::SessionId::new("persistent-session");
        cx.update(|_window, cx| {
            ThreadMetadataStore::global(cx).update(cx, |store, cx| {
                store.save(
                    ThreadMetadata {
                        thread_id: ThreadId::new(),
                        session_id: Some(resume_session_id.clone()),
                        agent_id: ProjectAgentId::new("Flaky"),
                        title: Some("持久化对话".into()),
                        updated_at: Utc::now(),
                        created_at: Some(Utc::now()),
                        interacted_at: None,
                        worktree_paths: WorktreePaths::from_folder_paths(&PathList::default()),
                        remote_connection: None,
                        archived: false,
                    },
                    cx,
                );
            });
        });

        let panel = workspace.update_in(cx, |workspace, window, cx| {
            cx.new(|cx| AgentPanel::new(workspace, None, window, cx))
        });

        // Open a restored thread using a flaky server so the initial connect
        // fails and the view lands in LoadError — mirroring the cold-start
        // race against a custom agent over SSH.
        let (server, _fail) =
            FlakyAgentServer::new(StubAgentConnection::new().with_supports_load_session(true));
        panel.update_in(cx, |panel, window, cx| {
            panel.open_restored_thread_with_server(
                Rc::new(server),
                resume_session_id.clone(),
                window,
                cx,
            );
        });
        cx.run_until_parked();

        // Sanity: the view couldn't connect, so no live AcpThread exists.
        panel.read_with(cx, |panel, cx| {
            assert!(
                panel.active_agent_thread(cx).is_none(),
                "不稳定服务器故障时，active_agent_thread 应为 None"
            );
            let conversation_view = panel
                .active_conversation_view()
                .expect("面板应仍保留活动的 ConversationView");
            assert_eq!(
                conversation_view.read(cx).root_session_id.as_ref(),
                Some(&resume_session_id),
                "ConversationView 应仍持有恢复的会话ID"
            );
        });

        // Serialize while in LoadError. Before the fix this wrote
        // `session_id=None` to the KVP and permanently lost the session.
        panel.update(cx, |panel, cx| panel.serialize(cx));
        cx.run_until_parked();

        let kvp = cx.update(|_window, cx| KeyValueStore::global(cx));
        let serialized: Option<SerializedAgentPanel> = cx
            .background_spawn(async move { read_serialized_panel(workspace_id, &kvp) })
            .await;
        let serialized_session_id = serialized
            .as_ref()
            .and_then(|p| p.last_active_thread.as_ref())
            .and_then(|t| t.session_id.clone());
            assert_eq!(
            serialized_session_id,
            Some(resume_session_id.0.to_string()),
            "即使 ConversationView 处于加载错误状态，serialize() 也必须保留已恢复的会话ID；\
            否则键值对会被清空，重启后问题依然存在"
        );
    }

    /// Extracts the text from a Text content block, panicking if it's not Text.
    fn expect_text_block(block: &acp::ContentBlock) -> &str {
        match block {
            acp::ContentBlock::Text(t) => t.text.as_str(),
            other => panic!("预期为文本块，实际得到 {:?}", other),
        }
    }

    /// Extracts the (text_content, uri) from a Resource content block, panicking
    /// if it's not a TextResourceContents resource.
    /// 预期为资源块
    fn expect_resource_block(block: &acp::ContentBlock) -> (&str, &str) {
        match block {
            acp::ContentBlock::Resource(r) => match &r.resource {
                acp::EmbeddedResourceResource::TextResourceContents(t) => {
                    (t.text.as_str(), t.uri.as_str())
                }
                other => panic!("预期为文本资源内容，实际得到 {:?}", other),
            },
            other => panic!("预期为资源块，实际得到 {:?}", other),
        }
    }

    #[test]
    /// 测试构建单个冲突的解决提示
    fn test_build_conflict_resolution_prompt_single_conflict() {
        let conflicts = vec![ConflictContent {
            file_path: "src/main.rs".to_string(),
            conflict_text: "<<<<<<< HEAD\nlet x = 1;\n=======\nlet x = 2;\n>>>>>>> feature"
                .to_string(),
            ours_branch_name: "HEAD".to_string(),
            theirs_branch_name: "feature".to_string(),
        }];

        let blocks = build_conflict_resolution_prompt(&conflicts);
        // 2 个文本块 + 1 个资源链接 + 1 个冲突资源块
        assert_eq!(
            blocks.len(),
            4,
            "预期 2 个文本块 + 1 个资源链接 + 1 个资源块"
        );

        let intro_text = expect_text_block(&blocks[0]);
        assert!(
            intro_text.contains("请解决以下文件中的合并冲突"),
            "提示应包含单个冲突的介绍文本"
        );

        match &blocks[1] {
            acp::ContentBlock::ResourceLink(link) => {
                assert!(
                    link.uri.contains("file://"),
                    "资源链接 URI 应使用文件协议"
                );
                assert!(
                    link.uri.contains("main.rs"),
                    "资源链接 URI 应引用文件路径"
                );
            }
            other => panic!("预期为资源链接块，实际得到 {:?}", other),
        }

        let body_text = expect_text_block(&blocks[2]);
        assert!(
            body_text.contains("`HEAD`（当前分支）"),
            "提示应提及当前分支"
        );
        assert!(
            body_text.contains("`feature`（合并分支）"),
            "提示应提及合并分支"
        );
        assert!(
            body_text.contains("直接编辑文件"),
            "提示应指示智能体直接编辑文件"
        );

        let (resource_text, resource_uri) = expect_resource_block(&blocks[3]);
        assert!(
            resource_text.contains("<<<<<<< HEAD"),
            "资源应包含冲突文本"
        );
        assert!(
            resource_uri.contains("merge-conflict"),
            "资源 URI 应使用合并冲突协议"
        );
        assert!(
            resource_uri.contains("main.rs"),
            "资源 URI 应引用文件路径"
        );
    }

    #[test]
    /// 测试构建同一文件多个冲突的解决提示
    fn test_build_conflict_resolution_prompt_multiple_conflicts_same_file() {
        let conflicts = vec![
            ConflictContent {
                file_path: "src/lib.rs".to_string(),
                conflict_text: "<<<<<<< main\nfn a() {}\n=======\nfn a_v2() {}\n>>>>>>> dev"
                    .to_string(),
                ours_branch_name: "main".to_string(),
                theirs_branch_name: "dev".to_string(),
            },
            ConflictContent {
                file_path: "src/lib.rs".to_string(),
                conflict_text: "<<<<<<< main\nfn b() {}\n=======\nfn b_v2() {}\n>>>>>>> dev"
                    .to_string(),
                ours_branch_name: "main".to_string(),
                theirs_branch_name: "dev".to_string(),
            },
        ];

        let blocks = build_conflict_resolution_prompt(&conflicts);
        // 1 个指令文本块 + 2 个资源块
        assert_eq!(blocks.len(), 3, "预期 1 个文本块 + 2 个资源块");

        let text = expect_text_block(&blocks[0]);
        assert!(
            text.contains("全部 2 个合并冲突"),
            "提示应包含冲突总数"
        );
        assert!(
            text.contains("`main`（当前分支）"),
            "提示应提及当前分支"
        );
        assert!(
            text.contains("`dev`（合并分支）"),
            "提示应提及合并分支"
        );
        // 单个文件，使用单数“文件”而非复数
        assert!(
            text.contains("直接编辑文件"),
            "单个文件应使用单数'文件'"
        );

        let (resource_a, _) = expect_resource_block(&blocks[1]);
        let (resource_b, _) = expect_resource_block(&blocks[2]);
        assert!(
            resource_a.contains("fn a()"),
            "第一个资源应包含第一个冲突内容"
        );
        assert!(
            resource_b.contains("fn b()"),
            "第二个资源应包含第二个冲突内容"
        );
    }

    #[test]
    fn test_build_conflict_resolution_prompt_multiple_conflicts_different_files() {
        let conflicts = vec![
            ConflictContent {
                file_path: "src/a.rs".to_string(),
                conflict_text: "<<<<<<< main\nA\n=======\nB\n>>>>>>> dev".to_string(),
                ours_branch_name: "main".to_string(),
                theirs_branch_name: "dev".to_string(),
            },
            ConflictContent {
                file_path: "src/b.rs".to_string(),
                conflict_text: "<<<<<<< main\nC\n=======\nD\n>>>>>>> dev".to_string(),
                ours_branch_name: "main".to_string(),
                theirs_branch_name: "dev".to_string(),
            },
        ];

        let blocks = build_conflict_resolution_prompt(&conflicts);
        // 1 Text instruction + 2 Resource blocks
        assert_eq!(blocks.len(), 3, "预期 1 个文本块 + 2 个资源块");

        let text = expect_text_block(&blocks[0]);
        assert!(
            text.contains("直接编辑这些文件"),
            "多个文件应使用复数'文件'"
        );

        let (_, uri_a) = expect_resource_block(&blocks[1]);
        let (_, uri_b) = expect_resource_block(&blocks[2]);
        assert!(
            uri_a.contains("a.rs"),
            "第一个资源URI应引用a.rs"
        );
        assert!(
            uri_b.contains("b.rs"),
            "第二个资源URI应引用b.rs"
        );
    }

    #[test]
    fn test_build_conflicted_files_resolution_prompt_file_paths_only() {
        let file_paths = vec![
            "src/main.rs".to_string(),
            "src/lib.rs".to_string(),
            "tests/integration.rs".to_string(),
        ];

        let blocks = build_conflicted_files_resolution_prompt(&file_paths);
        // 1 instruction Text block + (ResourceLink + newline Text) per file
        assert_eq!(
            blocks.len(),
            1 + (file_paths.len() * 2),
            "预期为指令文本 + 资源链接和分隔符"
        );

        let text = expect_text_block(&blocks[0]);
        assert!(
            text.contains("未解决的合并冲突"),
            "提示应描述任务内容"
        );
        assert!(
            text.contains("冲突标记"),
            "提示应提及冲突标记"
        );

        for (index, path) in file_paths.iter().enumerate() {
            let link_index = 1 + (index * 2);
            let newline_index = link_index + 1;

            match &blocks[link_index] {
                acp::ContentBlock::ResourceLink(link) => {
                    assert!(
                        link.uri.contains("file://"),
                        "资源链接 URI 应使用文件协议"
                    );
                    assert!(
                        link.uri.contains(path),
                        "资源链接 URI 应引用文件路径：{path}"
                    );
                }
                other => panic!(
                    "索引 {} 处预期为资源链接块，实际得到 {:?}",
                    link_index, other
                ),
            }

            let separator = expect_text_block(&blocks[newline_index]);
            assert_eq!(
                separator, "\n",
                "每个文件后预期为换行分隔符"
            );
        }
    }

    #[test]
    fn test_build_conflict_resolution_prompt_empty_conflicts() {
        let blocks = build_conflict_resolution_prompt(&[]);
        assert!(
            blocks.is_empty(),
            "空冲突不应生成任何内容块，实际生成了 {} 个块",
            blocks.len()
        );
    }

    #[test]
    fn test_build_conflicted_files_resolution_prompt_empty_paths() {
        let blocks = build_conflicted_files_resolution_prompt(&[]);
        assert!(
            blocks.is_empty(),
            "空路径不应生成任何内容块，实际生成了 {} 个块",
            blocks.len()
        );
    }

    #[test]
    fn test_conflict_resource_block_structure() {
        let conflict = ConflictContent {
            file_path: "src/utils.rs".to_string(),
            conflict_text: "<<<<<<< HEAD\nold code\n=======\nnew code\n>>>>>>> branch".to_string(),
            ours_branch_name: "HEAD".to_string(),
            theirs_branch_name: "branch".to_string(),
        };

        let block = conflict_resource_block(&conflict);
        let (text, uri) = expect_resource_block(&block);

        assert_eq!(
        text, conflict.conflict_text,
            "资源文本应为原始冲突内容"
        );
        assert!(
            uri.starts_with("zed:///agent/merge-conflict"),
            "URI 应使用 zed 合并冲突协议，实际为：{uri}"
        );
        assert!(uri.contains("utils.rs"), "URI 应包含文件路径");
    }

    fn open_generating_thread_with_loadable_connection(
        panel: &Entity<AgentPanel>,
        connection: &StubAgentConnection,
        cx: &mut VisualTestContext,
    ) -> (acp::SessionId, ThreadId) {
        open_thread_with_custom_connection(panel, connection.clone(), cx);
        let session_id = active_session_id(panel, cx);
        let thread_id = active_thread_id(panel, cx);
        send_message(panel, cx);
        cx.update(|_, cx| {
            connection.send_update(
                session_id.clone(),
                acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new("done".into())),
                cx,
            );
        });
        cx.run_until_parked();
        (session_id, thread_id)
    }

    fn open_idle_thread_with_non_loadable_connection(
        panel: &Entity<AgentPanel>,
        connection: &StubAgentConnection,
        cx: &mut VisualTestContext,
    ) -> (acp::SessionId, ThreadId) {
        open_thread_with_custom_connection(panel, connection.clone(), cx);
        let session_id = active_session_id(panel, cx);
        let thread_id = active_thread_id(panel, cx);

        connection.set_next_prompt_updates(vec![acp::SessionUpdate::AgentMessageChunk(
            acp::ContentChunk::new("done".into()),
        )]);
        send_message(panel, cx);

        (session_id, thread_id)
    }

    #[gpui::test]
    async fn test_draft_promotion_creates_metadata_and_new_session_on_reload(
        cx: &mut TestAppContext,
    ) {
        init_test(cx);
        cx.update(|cx| {
            agent::ThreadStore::init_global(cx);
            language_model::LanguageModelRegistry::test(cx);
        });

        let fs = FakeFs::new(cx.executor());
        fs.insert_tree("/project", json!({ "file.txt": "" })).await;
        let project = Project::test(fs.clone(), [Path::new("/project")], cx).await;

        let multi_workspace =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));

        let workspace = multi_workspace
            .read_with(cx, |mw, _cx| mw.workspace().clone())
            .unwrap();

        workspace.update(cx, |workspace, _cx| {
            workspace.set_random_database_id();
        });

        let cx = &mut VisualTestContext::from_window(multi_workspace.into(), cx);

        let panel = workspace.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| AgentPanel::new(workspace, None, window, cx));
            workspace.add_panel(panel.clone(), window, cx);
            panel
        });

        // Register a shared stub connection and use Agent::Stub so the draft
        // (and any reloaded draft) uses it.
        let stub_connection =
            crate::test_support::set_stub_agent_connection(StubAgentConnection::new());
        stub_connection.set_next_prompt_updates(vec![acp::SessionUpdate::AgentMessageChunk(
            acp::ContentChunk::new("Response".into()),
        )]);
        panel.update_in(cx, |panel, window, cx| {
            panel.selected_agent = Agent::Stub;
            panel.activate_draft(true, "agent_panel", window, cx);
        });
        cx.run_until_parked();

        // Verify the thread is considered a draft.
        panel.read_with(cx, |panel, cx| {
            assert!(
                panel.active_thread_is_draft(cx),
                "发送任何消息前，会话应处于草稿状态"
            );
            assert!(
                panel.draft_thread.is_some(),
                "draft_thread 字段应已设置"
            );
        });
        let draft_session_id = active_session_id(&panel, cx);
        let thread_id = active_thread_id(&panel, cx);

        // No metadata should exist yet for a draft.
        cx.update(|_window, cx| {
            let store = ThreadMetadataStore::global(cx).read(cx);
            assert!(
                store.entry(thread_id).is_none(),
                "草稿会话在存储中不应包含元数据"
            );
        });

        // Set draft prompt and serialize — the draft should survive a round-trip
        // with its prompt intact but a fresh ACP session.
        let draft_prompt_blocks = vec![acp::ContentBlock::Text(acp::TextContent::new(
            "Hello from draft",
        ))];
        panel.update(cx, |panel, cx| {
            let thread = panel.active_agent_thread(cx).unwrap();
            thread.update(cx, |thread, cx| {
                thread.set_draft_prompt(Some(draft_prompt_blocks.clone()), cx);
            });
            panel.serialize(cx);
        });
        cx.run_until_parked();

        let async_cx = cx.update(|window, cx| window.to_async(cx));
        let reloaded_panel = AgentPanel::load(workspace.downgrade(), async_cx)
            .await
            .expect("带草稿的面板加载应成功");
        cx.run_until_parked();

        reloaded_panel.read_with(cx, |panel, cx| {
            assert!(
                panel.active_thread_is_draft(cx),
                "重新加载后面板应仍将草稿显示为活动状态"
            );
            assert!(
                panel.draft_thread.is_some(),
                "重新加载后面板应保留 draft_thread"
            );
        });

        let reloaded_session_id = active_session_id(&reloaded_panel, cx);
        assert_ne!(
            reloaded_session_id, draft_session_id,
            "重新加载的草稿应拥有全新的ACP会话ID"
        );

        let restored_text = reloaded_panel.read_with(cx, |panel, cx| {
            let thread_id = panel.active_thread_id(cx).unwrap();
            panel.editor_text(thread_id, cx)
        });
        assert_eq!(
            restored_text.as_deref(),
            Some("Hello from draft"),
            "草稿提示文本应在序列化过程中被保留"
        );

        // Send a message on the reloaded panel — this promotes the draft to a real thread.
        let panel = reloaded_panel;
        let draft_session_id = reloaded_session_id;
        let thread_id = active_thread_id(&panel, cx);
        send_message(&panel, cx);

        // Verify promotion: draft_thread is cleared, metadata exists.
        panel.read_with(cx, |panel, cx| {
            assert!(
                !panel.active_thread_is_draft(cx),
                "发送消息后，会话不应再为草稿状态"
            );
            assert!(
                panel.draft_thread.is_none(),
                "升级完成后，draft_thread 应为 None"
            );
            assert_eq!(
                panel.active_thread_id(cx),
                Some(thread_id),
                "升级后应保持相同的活动会话ID"
            );
        });

        cx.update(|_window, cx| {
            let store = ThreadMetadataStore::global(cx).read(cx);
            let metadata = store
                .entry(thread_id)
                .expect("已升级的会话应包含元数据");
            assert!(
                metadata.session_id.is_some(),
                "已升级的会话元数据应包含有效的会话ID"
            );
            assert_eq!(
                metadata.session_id.as_ref().unwrap(),
                &draft_session_id,
                "元数据中的会话ID应与线程的ACP会话一致"
            );
        });

        // Serialize the panel, then reload it.
        panel.update(cx, |panel, cx| panel.serialize(cx));
        cx.run_until_parked();

        let async_cx = cx.update(|window, cx| window.to_async(cx));
        let loaded_panel = AgentPanel::load(workspace.downgrade(), async_cx)
            .await
            .expect("面板加载应成功");
        cx.run_until_parked();

        // The loaded panel should restore the real thread (not the draft).
        loaded_panel.read_with(cx, |panel, cx| {
            let active_id = panel.active_thread_id(cx);
            assert_eq!(
                active_id,
                Some(thread_id),
                "加载后的面板应恢复已升级的会话"
            );
            assert!(
                !panel.active_thread_is_draft(cx),
                "恢复的会话不应为草稿状态"
            );
        });
    }

    async fn setup_panel(cx: &mut TestAppContext) -> (Entity<AgentPanel>, VisualTestContext) {
        init_test(cx);
        cx.update(|cx| {
            agent::ThreadStore::init_global(cx);
            language_model::LanguageModelRegistry::test(cx);
        });

        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs.clone(), [], cx).await;

        let multi_workspace =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));

        let workspace = multi_workspace
            .read_with(cx, |mw, _cx| mw.workspace().clone())
            .unwrap();

        let mut cx = VisualTestContext::from_window(multi_workspace.into(), cx);

        let panel = workspace.update_in(&mut cx, |workspace, window, cx| {
            cx.new(|cx| AgentPanel::new(workspace, None, window, cx))
        });

        (panel, cx)
    }

    #[gpui::test]
    async fn test_running_thread_retained_when_navigating_away(cx: &mut TestAppContext) {
        let (panel, mut cx) = setup_panel(cx).await;

        let connection_a = StubAgentConnection::new();
        open_thread_with_connection(&panel, connection_a.clone(), &mut cx);
        send_message(&panel, &mut cx);

        let session_id_a = active_session_id(&panel, &cx);
        let thread_id_a = active_thread_id(&panel, &cx);

        // Send a chunk to keep thread A generating (don't end the turn).
        cx.update(|_, cx| {
            connection_a.send_update(
                session_id_a.clone(),
                acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new("chunk".into())),
                cx,
            );
        });
        cx.run_until_parked();

        // Verify thread A is generating.
        panel.read_with(&cx, |panel, cx| {
            let thread = panel.active_agent_thread(cx).unwrap();
            assert_eq!(thread.read(cx).status(), ThreadStatus::Generating);
            assert!(panel.retained_threads.is_empty());
        });

        // Open a new thread B — thread A should be retained in background.
        let connection_b = StubAgentConnection::new();
        open_thread_with_connection(&panel, connection_b, &mut cx);

        panel.read_with(&cx, |panel, _cx| {
            assert_eq!(
                panel.retained_threads.len(),
                1,
                "运行中的会话A应保留在 retained_threads 中"
            );
            assert!(
                panel.retained_threads.contains_key(&thread_id_a),
                "保留的会话应以会话A的ID作为键"
            );
        });
    }

    #[gpui::test]
    async fn test_idle_non_loadable_thread_retained_when_navigating_away(cx: &mut TestAppContext) {
        let (panel, mut cx) = setup_panel(cx).await;

        let connection_a = StubAgentConnection::new();
        connection_a.set_next_prompt_updates(vec![acp::SessionUpdate::AgentMessageChunk(
            acp::ContentChunk::new("Response".into()),
        )]);
        open_thread_with_connection(&panel, connection_a, &mut cx);
        send_message(&panel, &mut cx);

        let weak_view_a = panel.read_with(&cx, |panel, _cx| {
            panel.active_conversation_view().unwrap().downgrade()
        });
        let thread_id_a = active_thread_id(&panel, &cx);

        // Thread A should be idle (auto-completed via set_next_prompt_updates).
        panel.read_with(&cx, |panel, cx| {
            let thread = panel.active_agent_thread(cx).unwrap();
            assert_eq!(thread.read(cx).status(), ThreadStatus::Idle);
        });

        // Open a new thread B — thread A should be retained because it is not loadable.
        let connection_b = StubAgentConnection::new();
        open_thread_with_connection(&panel, connection_b, &mut cx);

        panel.read_with(&cx, |panel, _cx| {
            assert_eq!(
                panel.retained_threads.len(),
                1,
                "空闲且不可加载的会话 A 应保留在 retained_threads 中"
            );
            assert!(
                panel.retained_threads.contains_key(&thread_id_a),
                "保留的会话应以会话 A 的 ID 作为键"
            );
        });

        assert!(
            weak_view_a.upgrade().is_some(),
            "空闲且不可加载的连接视图仍应被保留"
        );
    }

    #[gpui::test]
    async fn test_background_thread_promoted_via_load(cx: &mut TestAppContext) {
        let (panel, mut cx) = setup_panel(cx).await;

        let connection_a = StubAgentConnection::new();
        open_thread_with_connection(&panel, connection_a.clone(), &mut cx);
        send_message(&panel, &mut cx);

        let session_id_a = active_session_id(&panel, &cx);
        let thread_id_a = active_thread_id(&panel, &cx);

        // Keep thread A generating.
        cx.update(|_, cx| {
            connection_a.send_update(
                session_id_a.clone(),
                acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new("chunk".into())),
                cx,
            );
        });
        cx.run_until_parked();

        // Open thread B — thread A goes to background.
        let connection_b = StubAgentConnection::new();
        open_thread_with_connection(&panel, connection_b, &mut cx);
        send_message(&panel, &mut cx);

        let thread_id_b = active_thread_id(&panel, &cx);

        panel.read_with(&cx, |panel, _cx| {
            assert_eq!(panel.retained_threads.len(), 1);
            assert!(panel.retained_threads.contains_key(&thread_id_a));
        });

        // Load thread A back via load_agent_thread — should promote from background.
        panel.update_in(&mut cx, |panel, window, cx| {
            panel.load_agent_thread(
                panel.selected_agent(cx),
                session_id_a.clone(),
                None,
                None,
                true,
                "agent_panel",
                window,
                cx,
            );
        });

        // Thread A should now be the active view, promoted from background.
        let active_session = active_session_id(&panel, &cx);
        assert_eq!(
            active_session, session_id_a,
            "升级后会话 A 应为活动会话"
        );

        panel.read_with(&cx, |panel, _cx| {
            assert!(
                !panel.retained_threads.contains_key(&thread_id_a),
                "已升级的会话 A 不应再保留在 retained_threads 中"
            );
            assert!(
                panel.retained_threads.contains_key(&thread_id_b),
                "会话 B（空闲、不可加载）应继续保留在 retained_threads 中"
            );
        });
    }

    #[gpui::test]
    async fn test_reopening_visible_thread_keeps_thread_usable(cx: &mut TestAppContext) {
        let (panel, mut cx) = setup_panel(cx).await;
        cx.run_until_parked();

        panel.update(&mut cx, |panel, cx| {
            panel.connection_store.update(cx, |store, cx| {
                store.restart_connection(
                    Agent::NativeAgent,
                    Rc::new(StubAgentServer::new(SessionTrackingConnection::new())),
                    cx,
                );
            });
        });
        cx.run_until_parked();

        panel.update_in(&mut cx, |panel, window, cx| {
            panel.external_thread(
                Some(Agent::NativeAgent),
                None,
                None,
                None,
                None,
                true,
                "agent_panel",
                window,
                cx,
            );
        });
        cx.run_until_parked();
        send_message(&panel, &mut cx);

        let session_id = active_session_id(&panel, &cx);

        panel.update_in(&mut cx, |panel, window, cx| {
            panel.open_thread(session_id.clone(), None, None, window, cx);
        });
        cx.run_until_parked();

        send_message(&panel, &mut cx);

        panel.read_with(&cx, |panel, cx| {
        // 获取当前活动的对话视图，重新打开后可见的对话应保持打开状态
        let active_view = panel
            .active_conversation_view()
            .expect("重新打开后可见的对话应保持开启");
        
        // 转换为已连接状态，UI中可见的对话应保持连接
        let connected = active_view
            .read(cx)
            .as_connected()
            .expect("UI中可见的对话应保持连接状态");
        
        // 验证线程无错误，重新打开可见会话应保持线程可用
        assert!(
            !connected.has_thread_error(cx),
            "重新打开已可见的会话应保持线程可用"
        );
    });
    }

    #[gpui::test]
    async fn test_initial_content_for_thread_summary_uses_own_session_id(cx: &mut TestAppContext) {
        init_test(cx);
        cx.update(|cx| {
            agent::ThreadStore::init_global(cx);
            language_model::LanguageModelRegistry::test(cx);
        });

        let source_session_id = acp::SessionId::new("source-thread-session");
        let source_title: SharedString = "Source Thread Title".into();
        let db_thread = agent::DbThread {
            title: source_title.clone(),
            messages: Vec::new(),
            updated_at: Utc::now(),
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
        };

        let thread_store = cx.update(|cx| ThreadStore::global(cx));
        thread_store
            .update(cx, |store, cx| {
                store.save_thread(
                    source_session_id.clone(),
                    db_thread,
                    PathList::default(),
                    cx,
                )
            })
            .await
            .expect("保存源线程应成功");
        cx.run_until_parked();

        thread_store.read_with(cx, |store, _cx| {
            let entry = store
                .thread_from_session_id(&source_session_id)
                .expect("已保存的线程应存在于存储中");
            assert!(
                entry.parent_session_id.is_none(),
                "已保存的线程是无父会话的根线程"
            );
        });

        let content = cx
            .update(|cx| {
                AgentPanel::initial_content_for_thread_summary(source_session_id.clone(), cx)
            })
            .expect("应为根线程生成初始内容");

        match content {
            AgentInitialContent::ThreadSummary { session_id, title } => {
                assert_eq!(
                    session_id, source_session_id,
                    "线程摘要引用应使用源线程自身的会话ID"
                );
                assert_eq!(title, Some(source_title.clone()));
            }
            _ => panic!("期望得到 AgentInitialContent::ThreadSummary 类型"),
        }

        // Unknown session ids should still produce no content.
        let missing = cx.update(|cx| {
            AgentPanel::initial_content_for_thread_summary(
                acp::SessionId::new("does-not-exist"),
                cx,
            )
        });
        assert!(
            missing.is_none(),
            "未知的会话ID不应生成初始内容"
        );
    }

    #[gpui::test]
    async fn test_cleanup_retained_threads_keeps_five_most_recent_idle_loadable_threads(
        cx: &mut TestAppContext,
    ) {
        let (panel, mut cx) = setup_panel(cx).await;
        let connection = StubAgentConnection::new()
            .with_supports_load_session(true)
            .with_agent_id("loadable-stub".into())
            .with_telemetry_id("loadable-stub".into());
        let mut session_ids = Vec::new();
        let mut thread_ids = Vec::new();

        for _ in 0..7 {
            let (session_id, thread_id) =
                open_generating_thread_with_loadable_connection(&panel, &connection, &mut cx);
            session_ids.push(session_id);
            thread_ids.push(thread_id);
        }

        let base_time = Instant::now();

        for session_id in session_ids.iter().take(6) {
            connection.end_turn(session_id.clone(), acp::StopReason::EndTurn);
        }
        cx.run_until_parked();

        panel.update(&mut cx, |panel, cx| {
            for (index, thread_id) in thread_ids.iter().take(6).enumerate() {
                let conversation_view = panel
                    .retained_threads
                    .get(thread_id)
                    .expect("保留的线程应存在")
                    .clone();
                conversation_view.update(cx, |view, cx| {
                    view.set_updated_at(base_time + Duration::from_secs(index as u64), cx);
                });
            }
            panel.cleanup_retained_threads(cx);
        });

        panel.read_with(&cx, |panel, _cx| {
        // 验证清理后最多保留 5 个空闲可加载的线程
        assert_eq!(
            panel.retained_threads.len(),
            5,
            "清理操作应最多保留五个空闲可加载的保留线程"
        );
        
        // 最早的空闲可加载线程应被移除
        assert!(
            !panel.retained_threads.contains_key(&thread_ids[0]),
            "最早的空闲可加载保留线程应被清理移除"
        );
        
        // 验证较新的 5 个空闲可加载线程均被保留
        for thread_id in &thread_ids[1..6] {
            assert!(
                panel.retained_threads.contains_key(thread_id),
                "较新的空闲可加载保留线程应被保留"
            );
        }
        
        // 活动线程不应被存储为保留线程
        assert!(
            !panel.retained_threads.contains_key(&thread_ids[6]),
            "活动线程不应同时作为保留线程存储"
        );
    });
    }

    #[gpui::test]
    async fn test_cleanup_retained_threads_preserves_idle_non_loadable_threads(
        cx: &mut TestAppContext,
    ) {
        let (panel, mut cx) = setup_panel(cx).await;

        let non_loadable_connection = StubAgentConnection::new();
        let (_non_loadable_session_id, non_loadable_thread_id) =
            open_idle_thread_with_non_loadable_connection(
                &panel,
                &non_loadable_connection,
                &mut cx,
            );

        let loadable_connection = StubAgentConnection::new()
            .with_supports_load_session(true)
            .with_agent_id("loadable-stub".into())
            .with_telemetry_id("loadable-stub".into());
        let mut loadable_session_ids = Vec::new();
        let mut loadable_thread_ids = Vec::new();

        for _ in 0..7 {
            let (session_id, thread_id) = open_generating_thread_with_loadable_connection(
                &panel,
                &loadable_connection,
                &mut cx,
            );
            loadable_session_ids.push(session_id);
            loadable_thread_ids.push(thread_id);
        }

        let base_time = Instant::now();

        for session_id in loadable_session_ids.iter().take(6) {
            loadable_connection.end_turn(session_id.clone(), acp::StopReason::EndTurn);
        }
        cx.run_until_parked();

        panel.update(&mut cx, |panel, cx| {
            for (index, thread_id) in loadable_thread_ids.iter().take(6).enumerate() {
                let conversation_view = panel
                    .retained_threads
                    .get(thread_id)
                    .expect("保留的线程应存在")
                    .clone();
                conversation_view.update(cx, |view, cx| {
                    view.set_updated_at(base_time + Duration::from_secs(index as u64), cx);
                });
            }
            panel.cleanup_retained_threads(cx);
        });

        panel.read_with(&cx, |panel, _cx| {
        // 验证清理后保留总数：5个可加载 + 1个不可加载 = 6个
        assert_eq!(
            panel.retained_threads.len(),
            6,
            "清理后应保留5个可加载空闲线程 + 1个不可加载空闲线程"
        );
        
        // 不可加载的空闲线程不会被清理
        assert!(
            panel.retained_threads.contains_key(&non_loadable_thread_id),
            "不可加载的空闲保留线程不应被清理"
        );
        
        // 最早的可加载空闲线程仍会被移除
        assert!(
            !panel.retained_threads.contains_key(&loadable_thread_ids[0]),
            "最早的可加载空闲线程应被移除"
        );
        
        // 较新的5个可加载空闲线程全部保留
        for thread_id in &loadable_thread_ids[1..6] {
            assert!(
                panel.retained_threads.contains_key(thread_id),
                "较新的可加载空闲线程应被保留"
            );
        }
        
        // 活动线程不会被存入保留线程列表
        assert!(
            !panel.retained_threads.contains_key(&loadable_thread_ids[6]),
            "活动的可加载线程不应作为保留线程存储"
        );
    });
    }

    #[test]
    fn test_deserialize_agent_variants() {
        // PascalCase (legacy AgentType format, persisted in panel state)
        assert_eq!(
            serde_json::from_str::<Agent>(r#""NativeAgent""#).unwrap(),
            Agent::NativeAgent,
        );
        assert_eq!(
            serde_json::from_str::<Agent>(r#"{"Custom":{"name":"my-agent"}}"#).unwrap(),
            Agent::Custom {
                id: "my-agent".into(),
            },
        );

        // Legacy TextThread variant deserializes to NativeAgent
        assert_eq!(
            serde_json::from_str::<Agent>(r#""TextThread""#).unwrap(),
            Agent::NativeAgent,
        );

        // snake_case (canonical format)
        assert_eq!(
            serde_json::from_str::<Agent>(r#""native_agent""#).unwrap(),
            Agent::NativeAgent,
        );
        assert_eq!(
            serde_json::from_str::<Agent>(r#"{"custom":{"name":"my-agent"}}"#).unwrap(),
            Agent::Custom {
                id: "my-agent".into(),
            },
        );

        // Serialization uses snake_case
        assert_eq!(
            serde_json::to_string(&Agent::NativeAgent).unwrap(),
            r#""native_agent""#,
        );
        assert_eq!(
            serde_json::to_string(&Agent::Custom {
                id: "my-agent".into()
            })
            .unwrap(),
            r#"{"custom":{"name":"my-agent"}}"#,
        );
    }

    #[gpui::test]
    fn test_resolve_worktree_branch_target() {
        let resolved = git_ui::worktree_service::resolve_worktree_branch_target(
            &NewWorktreeBranchTarget::ExistingBranch {
                name: "feature".to_string(),
            },
        );
        assert_eq!(resolved, Some("feature".to_string()));

        let resolved = git_ui::worktree_service::resolve_worktree_branch_target(
            &NewWorktreeBranchTarget::CurrentBranch,
        );
        assert_eq!(resolved, None);
    }

    #[gpui::test]
    async fn test_work_dirs_update_when_worktrees_change(cx: &mut TestAppContext) {
        use crate::thread_metadata_store::ThreadMetadataStore;

        init_test(cx);
        cx.update(|cx| {
            agent::ThreadStore::init_global(cx);
            language_model::LanguageModelRegistry::test(cx);
        });

        // Set up a project with one worktree.
        let fs = FakeFs::new(cx.executor());
        fs.insert_tree("/project_a", json!({ "file.txt": "" }))
            .await;
        let project = Project::test(fs.clone(), [Path::new("/project_a")], cx).await;

        let multi_workspace =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
        let workspace = multi_workspace
            .read_with(cx, |mw, _cx| mw.workspace().clone())
            .unwrap();
        let mut cx = VisualTestContext::from_window(multi_workspace.into(), cx);

        let panel = workspace.update_in(&mut cx, |workspace, window, cx| {
            cx.new(|cx| AgentPanel::new(workspace, None, window, cx))
        });

        // Open thread A and send a message. With empty next_prompt_updates it
        // stays generating, so opening B will move A to retained_threads.
        let connection_a = StubAgentConnection::new().with_agent_id("agent-a".into());
        open_thread_with_custom_connection(&panel, connection_a.clone(), &mut cx);
        send_message(&panel, &mut cx);
        let session_id_a = active_session_id(&panel, &cx);
        let thread_id_a = active_thread_id(&panel, &cx);

        // Open thread C — thread A (generating) moves to background.
        // Thread C completes immediately (idle), then opening B moves C to background too.
        let connection_c = StubAgentConnection::new().with_agent_id("agent-c".into());
        connection_c.set_next_prompt_updates(vec![acp::SessionUpdate::AgentMessageChunk(
            acp::ContentChunk::new("done".into()),
        )]);
        open_thread_with_custom_connection(&panel, connection_c.clone(), &mut cx);
        send_message(&panel, &mut cx);
        let thread_id_c = active_thread_id(&panel, &cx);

        // Open thread B — thread C (idle, non-loadable) is retained in background.
        let connection_b = StubAgentConnection::new().with_agent_id("agent-b".into());
        open_thread_with_custom_connection(&panel, connection_b.clone(), &mut cx);
        send_message(&panel, &mut cx);
        let session_id_b = active_session_id(&panel, &cx);
        let _thread_id_b = active_thread_id(&panel, &cx);

        let metadata_store = cx.update(|_, cx| ThreadMetadataStore::global(cx));

        panel.read_with(&cx, |panel, _cx| {
            // 验证线程 A 存在于保留线程列表中
            assert!(
                panel.retained_threads.contains_key(&thread_id_a),
                "线程 A 应存在于 retained_threads 中"
            );
            // 验证线程 C 存在于保留线程列表中
            assert!(
                panel.retained_threads.contains_key(&thread_id_c),
                "线程 C 应存在于 retained_threads 中"
            );
        });

        // Verify initial work_dirs for thread B contain only /project_a.
        let initial_b_paths = panel.read_with(&cx, |panel, cx| {
            let thread = panel.active_agent_thread(cx).unwrap();
            thread.read(cx).work_dirs().cloned().unwrap()
        });
        assert_eq!(
            initial_b_paths.ordered_paths().collect::<Vec<_>>(),
            vec![&PathBuf::from("/project_a")],
            "线程B初始应仅包含 /project_a"
        );

        // Now add a second worktree to the project.
        fs.insert_tree("/project_b", json!({ "other.txt": "" }))
            .await;
        let (new_tree, _) = project
            .update(&mut cx, |project, cx| {
                project.find_or_create_worktree("/project_b", true, cx)
            })
            .await
            .unwrap();
        cx.read(|cx| new_tree.read(cx).as_local().unwrap().scan_complete())
            .await;
        cx.run_until_parked();

        // Verify thread B's (active) work_dirs now include both worktrees.
        let updated_b_paths = panel.read_with(&cx, |panel, cx| {
            let thread = panel.active_agent_thread(cx).unwrap();
            thread.read(cx).work_dirs().cloned().unwrap()
        });
        let mut b_paths_sorted = updated_b_paths.ordered_paths().cloned().collect::<Vec<_>>();
        b_paths_sorted.sort();
        assert_eq!(
            b_paths_sorted,
            vec![PathBuf::from("/project_a"), PathBuf::from("/project_b")],
            "添加 /project_b 后，线程 B 的 work_dirs 应包含两个工作树"
        );

        // Verify thread A's (background) work_dirs are also updated.
        let updated_a_paths = panel.read_with(&cx, |panel, cx| {
            let bg_view = panel.retained_threads.get(&thread_id_a).unwrap();
            let root_thread = bg_view.read(cx).root_thread_view().unwrap();
            root_thread
                .read(cx)
                .thread
                .read(cx)
                .work_dirs()
                .cloned()
                .unwrap()
        });
        let mut a_paths_sorted = updated_a_paths.ordered_paths().cloned().collect::<Vec<_>>();
        a_paths_sorted.sort();
        assert_eq!(
            a_paths_sorted,
            vec![PathBuf::from("/project_a"), PathBuf::from("/project_b")],
            "添加 /project_b 后，线程 A 的 work_dirs 应包含两个工作树"
        );

        // Verify thread idle C was also updated.
        let updated_c_paths = panel.read_with(&cx, |panel, cx| {
            let bg_view = panel.retained_threads.get(&thread_id_c).unwrap();
            let root_thread = bg_view.read(cx).root_thread_view().unwrap();
            root_thread
                .read(cx)
                .thread
                .read(cx)
                .work_dirs()
                .cloned()
                .unwrap()
        });
        let mut c_paths_sorted = updated_c_paths.ordered_paths().cloned().collect::<Vec<_>>();
        c_paths_sorted.sort();
        assert_eq!(
            c_paths_sorted,
            vec![PathBuf::from("/project_a"), PathBuf::from("/project_b")],
            "添加 /project_b 后，空闲后台的线程 C 的 work_dirs 应包含两个工作树"
        );

        // Verify the metadata store reflects the new paths for running threads only.
        cx.run_until_parked();
        for (label, session_id) in [("thread B", &session_id_b), ("thread A", &session_id_a)] {
            let metadata_paths = metadata_store.read_with(&cx, |store, _cx| {
                let metadata = store
                    .entry_by_session(session_id)
                    .unwrap_or_else(|| panic!(" {label} 线程元数据应存在"));
                metadata.folder_paths().clone()
            });
            let mut sorted = metadata_paths.ordered_paths().cloned().collect::<Vec<_>>();
            sorted.sort();
            assert_eq!(
                sorted,
                vec![PathBuf::from("/project_a"), PathBuf::from("/project_b")],
                "{label} 线程元数据的 folder_paths 应包含两个工作树"
            );
        }

        // Now remove a worktree and verify work_dirs shrink.
        let worktree_b_id = new_tree.read_with(&cx, |tree, _| tree.id());
        project.update(&mut cx, |project, cx| {
            project.remove_worktree(worktree_b_id, cx);
        });
        cx.run_until_parked();

        let after_remove_b = panel.read_with(&cx, |panel, cx| {
            let thread = panel.active_agent_thread(cx).unwrap();
            thread.read(cx).work_dirs().cloned().unwrap()
        });
        assert_eq!(
            after_remove_b.ordered_paths().collect::<Vec<_>>(),
            vec![&PathBuf::from("/project_a")],
            "移除 /project_b 后，线程 B 的 work_dirs 应恢复为仅包含 /project_a"
        );

        let after_remove_a = panel.read_with(&cx, |panel, cx| {
            let bg_view = panel.retained_threads.get(&thread_id_a).unwrap();
            let root_thread = bg_view.read(cx).root_thread_view().unwrap();
            root_thread
                .read(cx)
                .thread
                .read(cx)
                .work_dirs()
                .cloned()
                .unwrap()
        });
        assert_eq!(
            after_remove_a.ordered_paths().collect::<Vec<_>>(),
            vec![&PathBuf::from("/project_a")],
            "移除 /project_b 后，线程 A 的 work_dirs 应恢复为仅包含 /project_a"
        );
    }

    #[gpui::test]
    async fn test_new_workspace_inherits_global_last_used_agent(cx: &mut TestAppContext) {
        init_test(cx);
        cx.update(|cx| {
            agent::ThreadStore::init_global(cx);
            language_model::LanguageModelRegistry::test(cx);
            // Use an isolated DB so parallel tests can't overwrite our global key.
            cx.set_global(db::AppDatabase::test_new());
        });

        let custom_agent = Agent::Custom {
            id: "my-preferred-agent".into(),
        };

        // Write a known agent to the global KVP to simulate a user who has
        // previously used this agent in another workspace.
        let kvp = cx.update(|cx| KeyValueStore::global(cx));
        write_global_last_used_agent(kvp, custom_agent.clone()).await;

        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs.clone(), [], cx).await;

        let multi_workspace =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));

        let workspace = multi_workspace
            .read_with(cx, |multi_workspace, _cx| {
                multi_workspace.workspace().clone()
            })
            .unwrap();

        workspace.update(cx, |workspace, _cx| {
            workspace.set_random_database_id();
        });

        let cx = &mut VisualTestContext::from_window(multi_workspace.into(), cx);

        // Load the panel via `load()`, which reads the global fallback
        // asynchronously when no per-workspace state exists.
        let async_cx = cx.update(|window, cx| window.to_async(cx));
        let panel = AgentPanel::load(workspace.downgrade(), async_cx)
            .await
            .expect("面板加载应成功");
        cx.run_until_parked();

        panel.read_with(cx, |panel, _cx| {
            assert_eq!(
                panel.selected_agent, custom_agent,
                "新工作区应继承全局最后使用的智能体"
            );
        });
    }

    #[gpui::test]
    async fn test_workspaces_maintain_independent_agent_selection(cx: &mut TestAppContext) {
        init_test(cx);
        cx.update(|cx| {
            agent::ThreadStore::init_global(cx);
            language_model::LanguageModelRegistry::test(cx);
        });

        let fs = FakeFs::new(cx.executor());
        let project_a = Project::test(fs.clone(), [], cx).await;
        let project_b = Project::test(fs, [], cx).await;

        let multi_workspace =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project_a.clone(), window, cx));

        let workspace_a = multi_workspace
            .read_with(cx, |multi_workspace, _cx| {
                multi_workspace.workspace().clone()
            })
            .unwrap();

        let workspace_b = multi_workspace
            .update(cx, |multi_workspace, window, cx| {
                multi_workspace.test_add_workspace(project_b.clone(), window, cx)
            })
            .unwrap();

        workspace_a.update(cx, |workspace, _cx| {
            workspace.set_random_database_id();
        });
        workspace_b.update(cx, |workspace, _cx| {
            workspace.set_random_database_id();
        });

        let cx = &mut VisualTestContext::from_window(multi_workspace.into(), cx);

        let agent_a = Agent::Custom {
            id: "agent-alpha".into(),
        };
        let agent_b = Agent::Custom {
            id: "agent-beta".into(),
        };

        // Set up workspace A with agent_a
        let panel_a = workspace_a.update_in(cx, |workspace, window, cx| {
            cx.new(|cx| AgentPanel::new(workspace, None, window, cx))
        });
        panel_a.update(cx, |panel, _cx| {
            panel.selected_agent = agent_a.clone();
        });

        // Set up workspace B with agent_b
        let panel_b = workspace_b.update_in(cx, |workspace, window, cx| {
            cx.new(|cx| AgentPanel::new(workspace, None, window, cx))
        });
        panel_b.update(cx, |panel, _cx| {
            panel.selected_agent = agent_b.clone();
        });

        // Serialize both panels
        panel_a.update(cx, |panel, cx| panel.serialize(cx));
        panel_b.update(cx, |panel, cx| panel.serialize(cx));
        cx.run_until_parked();

        // Load fresh panels from serialized state and verify independence
        let async_cx = cx.update(|window, cx| window.to_async(cx));
        let loaded_a = AgentPanel::load(workspace_a.downgrade(), async_cx)
            .await
            .expect("面板 A 加载应成功");
        cx.run_until_parked();

        let async_cx = cx.update(|window, cx| window.to_async(cx));
        let loaded_b = AgentPanel::load(workspace_b.downgrade(), async_cx)
            .await
            .expect("面板 B 加载应成功");;
        cx.run_until_parked();

        // 读取工作区A的面板状态，验证它恢复了正确的代理 agent_a
        loaded_a.read_with(cx, |panel, _cx| {
            assert_eq!(
                panel.selected_agent, agent_a,
                "工作区A应恢复为 agent-alpha，而非 agent-beta"
            );
        });

        // 读取工作区B的面板状态，验证它恢复了正确的代理 agent_b
        loaded_b.read_with(cx, |panel, _cx| {
            assert_eq!(
                panel.selected_agent, agent_b,
                "工作区B应恢复为 agent-beta，而非 agent-alpha"
            );
        });
    }

    #[gpui::test]
    async fn test_new_thread_uses_workspace_selected_agent(cx: &mut TestAppContext) {
        init_test(cx);
        cx.update(|cx| {
            agent::ThreadStore::init_global(cx);
            language_model::LanguageModelRegistry::test(cx);
        });

        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs.clone(), [], cx).await;

        let multi_workspace =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));

        let workspace = multi_workspace
            .read_with(cx, |multi_workspace, _cx| {
                multi_workspace.workspace().clone()
            })
            .unwrap();

        workspace.update(cx, |workspace, _cx| {
            workspace.set_random_database_id();
        });

        let cx = &mut VisualTestContext::from_window(multi_workspace.into(), cx);

        let custom_agent = Agent::Custom {
            id: "my-custom-agent".into(),
        };

        let panel = workspace.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| AgentPanel::new(workspace, None, window, cx));
            workspace.add_panel(panel.clone(), window, cx);
            panel
        });

        // Set selected_agent to a custom agent
        panel.update(cx, |panel, _cx| {
            panel.selected_agent = custom_agent.clone();
        });

        // Call new_thread, which internally calls external_thread(None, ...)
        // This resolves the agent from self.selected_agent
        panel.update_in(cx, |panel, window, cx| {
            panel.new_thread(&NewThread, window, cx);
        });

        panel.read_with(cx, |panel, _cx| {
        // 验证创建新线程后，选中的代理保持为自定义代理不变
        assert_eq!(
            panel.selected_agent, custom_agent,
            "执行 new_thread 后，selected_agent 应保持为自定义代理"
        );
        // 验证已成功创建一个线程（活跃对话视图存在）
        assert!(
            panel.active_conversation_view().is_some(),
            "应已创建一个线程"
        );
    });
    }

    #[gpui::test]
    async fn test_draft_replaced_when_selected_agent_changes(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        cx.update(|cx| {
            agent::ThreadStore::init_global(cx);
            language_model::LanguageModelRegistry::test(cx);
            <dyn fs::Fs>::set_global(fs.clone(), cx);
        });

        let project = Project::test(fs.clone(), [], cx).await;

        let multi_workspace =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));

        let workspace = multi_workspace
            .read_with(cx, |multi_workspace, _cx| {
                multi_workspace.workspace().clone()
            })
            .unwrap();

        workspace.update(cx, |workspace, _cx| {
            workspace.set_random_database_id();
        });

        let cx = &mut VisualTestContext::from_window(multi_workspace.into(), cx);

        let panel = workspace.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| AgentPanel::new(workspace, None, window, cx));
            workspace.add_panel(panel.clone(), window, cx);
            panel
        });

        // Create a draft with the default NativeAgent.
        panel.update_in(cx, |panel, window, cx| {
            panel.activate_draft(true, "agent_panel", window, cx);
        });

        let first_draft_id = panel.read_with(cx, |panel, cx| {
            assert!(panel.draft_thread.is_some());
            assert_eq!(panel.selected_agent, Agent::NativeAgent);
            let draft = panel.draft_thread.as_ref().unwrap();
            assert_eq!(*draft.read(cx).agent_key(), Agent::NativeAgent);
            draft.entity_id()
        });

        // Switch selected_agent to a custom agent, then activate_draft again.
        // The stale NativeAgent draft should be replaced.
        let custom_agent = Agent::Custom {
            id: "my-custom-agent".into(),
        };
        panel.update_in(cx, |panel, window, cx| {
            panel.selected_agent = custom_agent.clone();
            panel.activate_draft(true, "agent_panel", window, cx);
        });

        panel.read_with(cx, |panel, cx| {
        // 获取草稿线程，不存在则直接报错
        let draft = panel.draft_thread.as_ref().expect("草稿应存在");

        // 验证创建了全新的草稿（ID 与旧草稿不同）
        assert_ne!(
            draft.entity_id(),
            first_draft_id,
            "应创建一个新的草稿"
        );

        // 验证新草稿使用了指定的自定义代理
        assert_eq!(
            *draft.read(cx).agent_key(),
            custom_agent,
            "新草稿应使用自定义代理"
        );
        });
        // Calling activate_draft again with the same agent should return the
        // cached draft (no replacement).
        let second_draft_id = panel.read_with(cx, |panel, _cx| {
            panel.draft_thread.as_ref().unwrap().entity_id()
        });

        panel.update_in(cx, |panel, window, cx| {
            panel.activate_draft(true, "agent_panel", window, cx);
        });

        panel.read_with(cx, |panel, _cx| {
            assert_eq!(
                panel.draft_thread.as_ref().unwrap().entity_id(),
                second_draft_id,
                "当代理未发生变化时，应复用原有草稿"
            );
        });
    }

    #[gpui::test]
    async fn test_activate_draft_preserves_typed_content(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        cx.update(|cx| {
            agent::ThreadStore::init_global(cx);
            language_model::LanguageModelRegistry::test(cx);
            <dyn fs::Fs>::set_global(fs.clone(), cx);
        });

        let project = Project::test(fs.clone(), [], cx).await;

        let multi_workspace =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));

        let workspace = multi_workspace
            .read_with(cx, |multi_workspace, _cx| {
                multi_workspace.workspace().clone()
            })
            .unwrap();

        workspace.update(cx, |workspace, _cx| {
            workspace.set_random_database_id();
        });

        let cx = &mut VisualTestContext::from_window(multi_workspace.into(), cx);

        let panel = workspace.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| AgentPanel::new(workspace, None, window, cx));
            workspace.add_panel(panel.clone(), window, cx);
            panel
        });

        // Create a draft using the Stub agent, which connects synchronously.
        panel.update_in(cx, |panel, window, cx| {
            panel.selected_agent = Agent::Stub;
            panel.activate_draft(true, "agent_panel", window, cx);
        });
        cx.run_until_parked();

        let initial_draft_id = panel.read_with(cx, |panel, _cx| {
            panel.draft_thread.as_ref().unwrap().entity_id()
        });

        // Type some text into the draft editor.
        let thread_view = panel.read_with(cx, |panel, cx| panel.active_thread_view(cx).unwrap());
        let message_editor = thread_view.read_with(cx, |view, _cx| view.message_editor.clone());
        message_editor.update_in(cx, |editor, window, cx| {
            editor.set_text("Don't lose me!", window, cx);
        });

        // Press cmd-n (activate_draft again with the same agent).
        cx.dispatch_action(NewExternalAgentThread { agent: None });
        cx.run_until_parked();

        // The draft entity should not have changed.
        panel.read_with(cx, |panel, _cx| {
            assert_eq!(
                panel.draft_thread.as_ref().unwrap().entity_id(),
                initial_draft_id,
                "当已处于草稿状态时，cmd-n 不应替换原有草稿"
            );
        });

        // The editor content should be preserved.
        let thread_id = panel.read_with(cx, |panel, cx| panel.active_thread_id(cx).unwrap());
        let text = panel.read_with(cx, |panel, cx| panel.editor_text(thread_id, cx));
        assert_eq!(
            text.as_deref(),
            Some("Don't lose me!"),
            "在草稿上按下 cmd-n 时，已输入的内容应被保留"
        );
    }

    #[gpui::test]
    async fn test_draft_content_carried_over_when_switching_agents(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        cx.update(|cx| {
            agent::ThreadStore::init_global(cx);
            language_model::LanguageModelRegistry::test(cx);
            <dyn fs::Fs>::set_global(fs.clone(), cx);
        });

        let project = Project::test(fs.clone(), [], cx).await;

        let multi_workspace =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));

        let workspace = multi_workspace
            .read_with(cx, |multi_workspace, _cx| {
                multi_workspace.workspace().clone()
            })
            .unwrap();

        workspace.update(cx, |workspace, _cx| {
            workspace.set_random_database_id();
        });

        let cx = &mut VisualTestContext::from_window(multi_workspace.into(), cx);

        let panel = workspace.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| AgentPanel::new(workspace, None, window, cx));
            workspace.add_panel(panel.clone(), window, cx);
            panel
        });

        // Create a draft with a custom stub server that connects synchronously.
        panel.update_in(cx, |panel, window, cx| {
            panel.open_draft_with_server(
                Rc::new(StubAgentServer::new(StubAgentConnection::new())),
                window,
                cx,
            );
        });
        cx.run_until_parked();

        let initial_draft_id = panel.read_with(cx, |panel, _cx| {
            panel.draft_thread.as_ref().unwrap().entity_id()
        });

        // Type text into the first draft's editor.
        let thread_view = panel.read_with(cx, |panel, cx| panel.active_thread_view(cx).unwrap());
        let message_editor = thread_view.read_with(cx, |view, _cx| view.message_editor.clone());
        message_editor.update_in(cx, |editor, window, cx| {
            editor.set_text("carry me over", window, cx);
        });

        // Switch to a different agent. ensure_draft should extract the typed
        // content from the old draft and pre-fill the new one.
        cx.dispatch_action(NewExternalAgentThread {
            agent: Some(Agent::Stub),
        });
        cx.run_until_parked();

        // A new draft should have been created for the Stub agent.
        panel.read_with(cx, |panel, cx| {
            // 获取草稿线程，预期草稿必须存在
            let draft = panel.draft_thread.as_ref().expect("草稿应存在");
            assert_ne!(
                draft.entity_id(),
                initial_draft_id,
                "应为新代理创建新的草稿"
            );
            assert_eq!(
                *draft.read(cx).agent_key(),
                Agent::Stub,
                "新草稿应使用新的代理"
            );
        });

        // The new draft's editor should contain the text typed in the old draft.
        let thread_id = panel.read_with(cx, |panel, cx| panel.active_thread_id(cx).unwrap());
        let text = panel.read_with(cx, |panel, cx| panel.editor_text(thread_id, cx));
        assert_eq!(
            text.as_deref(),
            Some("carry me over"),
            "内容应被延续到新代理的草稿中"
        );
    }

    #[gpui::test]
    async fn test_rollback_all_succeed_returns_ok(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        cx.update(|cx| {
            cx.update_flags(true, vec!["agent-v2".to_string()]);
            agent::ThreadStore::init_global(cx);
            language_model::LanguageModelRegistry::test(cx);
            <dyn fs::Fs>::set_global(fs.clone(), cx);
        });

        fs.insert_tree(
            "/project",
            json!({
                ".git": {},
                "src": { "main.rs": "fn main() {}" }
            }),
        )
        .await;

        let project = Project::test(fs.clone(), [Path::new("/project")], cx).await;
        cx.executor().run_until_parked();

        let repository = project.read_with(cx, |project, cx| {
            project.repositories(cx).values().next().unwrap().clone()
        });

        let multi_workspace =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));

        let path_a = PathBuf::from("/worktrees/branch/project_a");
        let path_b = PathBuf::from("/worktrees/branch/project_b");

        let (sender_a, receiver_a) = futures::channel::oneshot::channel::<Result<()>>();
        let (sender_b, receiver_b) = futures::channel::oneshot::channel::<Result<()>>();
        sender_a.send(Ok(())).unwrap();
        sender_b.send(Ok(())).unwrap();

        let creation_infos = vec![
            (repository.clone(), path_a.clone(), receiver_a),
            (repository.clone(), path_b.clone(), receiver_b),
        ];

        let fs_clone = fs.clone();
        let result = multi_workspace
            .update(cx, |_, window, cx| {
                window.spawn(cx, async move |cx| {
                    git_ui::worktree_service::await_and_rollback_on_failure(
                        creation_infos,
                        fs_clone,
                        cx,
                    )
                    .await
                })
            })
            .unwrap()
            .await;

        let paths = result.expect("全部成功时应返回 Ok");
        assert_eq!(paths, vec![path_a, path_b]);
    }

    #[gpui::test]
    async fn test_rollback_on_failure_attempts_all_worktrees(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        cx.update(|cx| {
            cx.update_flags(true, vec!["agent-v2".to_string()]);
            agent::ThreadStore::init_global(cx);
            language_model::LanguageModelRegistry::test(cx);
            <dyn fs::Fs>::set_global(fs.clone(), cx);
        });

        fs.insert_tree(
            "/project",
            json!({
                ".git": {},
                "src": { "main.rs": "fn main() {}" }
            }),
        )
        .await;

        let project = Project::test(fs.clone(), [Path::new("/project")], cx).await;
        cx.executor().run_until_parked();

        let repository = project.read_with(cx, |project, cx| {
            project.repositories(cx).values().next().unwrap().clone()
        });

        // Actually create a worktree so it exists in FakeFs for rollback to find.
        let success_path = PathBuf::from("/worktrees/branch/project");
        cx.update(|cx| {
            repository.update(cx, |repo, _| {
                repo.create_worktree(
                    git::repository::CreateWorktreeTarget::NewBranch {
                        branch_name: "branch".to_string(),
                        base_sha: None,
                    },
                    success_path.clone(),
                )
            })
        })
        .await
        .unwrap()
        .unwrap();
        cx.executor().run_until_parked();

        // Verify the worktree directory exists before rollback.
        assert!(
            fs.is_dir(&success_path).await,
            "工作树目录在回滚前应存在"
        );

        let multi_workspace =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));

        // Build creation_infos: one success, one failure.
        let failed_path = PathBuf::from("/worktrees/branch/failed_project");

        let (sender_ok, receiver_ok) = futures::channel::oneshot::channel::<Result<()>>();
        let (sender_err, receiver_err) = futures::channel::oneshot::channel::<Result<()>>();
        sender_ok.send(Ok(())).unwrap();
        sender_err
            .send(Err(anyhow!("分支已存在")))
            .unwrap();

        let creation_infos = vec![
            (repository.clone(), success_path.clone(), receiver_ok),
            (repository.clone(), failed_path.clone(), receiver_err),
        ];

        let fs_clone = fs.clone();
        let result = multi_workspace
            .update(cx, |_, window, cx| {
                window.spawn(cx, async move |cx| {
                    git_ui::worktree_service::await_and_rollback_on_failure(
                        creation_infos,
                        fs_clone,
                        cx,
                    )
                    .await
                })
            })
            .unwrap()
            .await;

        assert!(
            result.is_err(),
            "任意创建失败时应返回错误"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("branch already exists"),
            "错误应包含原始失败原因: {err_msg}"
        );

        // The successful worktree should have been rolled back by git.
        cx.executor().run_until_parked();
        assert!(
            !fs.is_dir(&success_path).await,
            "成功创建的工作树目录应被回滚操作删除"
        );
    }

    #[gpui::test]
    async fn test_rollback_on_canceled_receiver(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        cx.update(|cx| {
            cx.update_flags(true, vec!["agent-v2".to_string()]);
            agent::ThreadStore::init_global(cx);
            language_model::LanguageModelRegistry::test(cx);
            <dyn fs::Fs>::set_global(fs.clone(), cx);
        });

        fs.insert_tree(
            "/project",
            json!({
                ".git": {},
                "src": { "main.rs": "fn main() {}" }
            }),
        )
        .await;

        let project = Project::test(fs.clone(), [Path::new("/project")], cx).await;
        cx.executor().run_until_parked();

        let repository = project.read_with(cx, |project, cx| {
            project.repositories(cx).values().next().unwrap().clone()
        });

        let multi_workspace =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));

        let path = PathBuf::from("/worktrees/branch/project");

        // Drop the sender to simulate a canceled receiver.
        let (_sender, receiver) = futures::channel::oneshot::channel::<Result<()>>();
        drop(_sender);

        let creation_infos = vec![(repository.clone(), path.clone(), receiver)];

        let fs_clone = fs.clone();
        let result = multi_workspace
            .update(cx, |_, window, cx| {
                window.spawn(cx, async move |cx| {
                    git_ui::worktree_service::await_and_rollback_on_failure(
                        creation_infos,
                        fs_clone,
                        cx,
                    )
                    .await
                })
            })
            .unwrap()
            .await;

        assert!(
            result.is_err(),
            "接收方取消时应返回错误"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("canceled"),
            "错误应包含取消相关信息: {err_msg}"
        );
    }

    #[gpui::test]
    async fn test_rollback_cleans_up_orphan_directories(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        cx.update(|cx| {
            cx.update_flags(true, vec!["agent-v2".to_string()]);
            agent::ThreadStore::init_global(cx);
            language_model::LanguageModelRegistry::test(cx);
            <dyn fs::Fs>::set_global(fs.clone(), cx);
        });

        fs.insert_tree(
            "/project",
            json!({
                ".git": {},
                "src": { "main.rs": "fn main() {}" }
            }),
        )
        .await;

        let project = Project::test(fs.clone(), [Path::new("/project")], cx).await;
        cx.executor().run_until_parked();

        let repository = project.read_with(cx, |project, cx| {
            project.repositories(cx).values().next().unwrap().clone()
        });

        let multi_workspace =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));

        // Simulate the orphan state: create_dir_all was called but git
        // worktree add failed, leaving a directory with leftover files.
        let orphan_path = PathBuf::from("/worktrees/branch/orphan_project");
        fs.insert_tree(
            "/worktrees/branch/orphan_project",
            json!({ "leftover.txt": "junk" }),
        )
        .await;

        assert!(
            fs.is_dir(&orphan_path).await,
            "孤立目录在回滚前应存在"
        );

        let (sender, receiver) = futures::channel::oneshot::channel::<Result<()>>();
        sender.send(Err(anyhow!("hook failed"))).unwrap();

        let creation_infos = vec![(repository.clone(), orphan_path.clone(), receiver)];

        let fs_clone = fs.clone();
        let result = multi_workspace
            .update(cx, |_, window, cx| {
                window.spawn(cx, async move |cx| {
                    git_ui::worktree_service::await_and_rollback_on_failure(
                        creation_infos,
                        fs_clone,
                        cx,
                    )
                    .await
                })
            })
            .unwrap()
            .await;

        cx.executor().run_until_parked();

        assert!(result.is_err());
        assert!(
            !fs.is_dir(&orphan_path).await,
            "孤立工作树目录应被文件系统清理操作删除"
        );
    }

    #[gpui::test]
    async fn test_selected_agent_syncs_when_navigating_between_threads(cx: &mut TestAppContext) {
        let (panel, mut cx) = setup_panel(cx).await;

        let stub_agent = Agent::Custom { id: "Test".into() };

        // Open thread A and send a message so it is retained.
        let connection_a = StubAgentConnection::new();
        connection_a.set_next_prompt_updates(vec![acp::SessionUpdate::AgentMessageChunk(
            acp::ContentChunk::new("response a".into()),
        )]);
        open_thread_with_connection(&panel, connection_a, &mut cx);
        let session_id_a = active_session_id(&panel, &cx);
        send_message(&panel, &mut cx);
        cx.run_until_parked();

        panel.read_with(&cx, |panel, _cx| {
            assert_eq!(panel.selected_agent, stub_agent);
        });

        // Open thread B with a different agent — thread A goes to retained.
        let custom_agent = Agent::Custom {
            id: "my-custom-agent".into(),
        };
        let connection_b = StubAgentConnection::new()
            .with_agent_id("my-custom-agent".into())
            .with_telemetry_id("my-custom-agent".into());
        connection_b.set_next_prompt_updates(vec![acp::SessionUpdate::AgentMessageChunk(
            acp::ContentChunk::new("response b".into()),
        )]);
        open_thread_with_custom_connection(&panel, connection_b, &mut cx);
        send_message(&panel, &mut cx);
        cx.run_until_parked();

        panel.read_with(&cx, |panel, _cx| {
            assert_eq!(
                panel.selected_agent, custom_agent,
                "选中的代理应已切换为自定义代理"
            );
        });

        // Navigate back to thread A via load_agent_thread.
        panel.update_in(&mut cx, |panel, window, cx| {
            panel.load_agent_thread(
                stub_agent.clone(),
                session_id_a.clone(),
                None,
                None,
                true,
                "agent_panel",
                window,
                cx,
            );
        });

        panel.read_with(&cx, |panel, _cx| {
            assert_eq!(
                panel.selected_agent, stub_agent,
                "选中的代理应同步回线程A的代理"
            );
        });
    }

    #[gpui::test]
    async fn test_classify_worktrees_skips_non_git_root_with_nested_repo(cx: &mut TestAppContext) {
        init_test(cx);
        cx.update(|cx| {
            agent::ThreadStore::init_global(cx);
            language_model::LanguageModelRegistry::test(cx);
        });

        let fs = FakeFs::new(cx.executor());
        fs.insert_tree(
            "/repo_a",
            json!({
                ".git": {},
                "src": { "main.rs": "" }
            }),
        )
        .await;
        fs.insert_tree(
            "/repo_b",
            json!({
                ".git": {},
                "src": { "lib.rs": "" }
            }),
        )
        .await;
        // `plain_dir` is NOT a git repo, but contains a nested git repo.
        fs.insert_tree(
            "/plain_dir",
            json!({
                "nested_repo": {
                    ".git": {},
                    "src": { "lib.rs": "" }
                }
            }),
        )
        .await;

        let project = Project::test(
            fs.clone(),
            [
                Path::new("/repo_a"),
                Path::new("/repo_b"),
                Path::new("/plain_dir"),
            ],
            cx,
        )
        .await;

        // Let the worktree scanner discover all `.git` directories.
        cx.executor().run_until_parked();

        let multi_workspace =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));

        let workspace = multi_workspace
            .read_with(cx, |mw, _cx| mw.workspace().clone())
            .unwrap();

        let cx = &mut VisualTestContext::from_window(multi_workspace.into(), cx);

        let panel = workspace.update_in(cx, |workspace, window, cx| {
            cx.new(|cx| AgentPanel::new(workspace, None, window, cx))
        });

        cx.run_until_parked();

        panel.read_with(cx, |panel, cx| {
            let (git_repos, non_git_paths) =
                git_ui::worktree_service::classify_worktrees(panel.project.read(cx), cx);

            let git_work_dirs: Vec<PathBuf> = git_repos
                .iter()
                .map(|repo| repo.read(cx).work_directory_abs_path.to_path_buf())
                .collect();

            assert_eq!(
            git_repos.len(),
            2,
            "仅 repo_a 和 repo_b 应被归类为 Git 仓库，\
            实际得到：{git_work_dirs:?}"
            );
            assert!(
            git_work_dirs.contains(&PathBuf::from("/repo_a")),
            "repo_a 应存在于 git_repos 中：{git_work_dirs:?}"
            );
            assert!(
            git_work_dirs.contains(&PathBuf::from("/repo_b")),
            "repo_b 应存在于 git_repos 中：{git_work_dirs:?}"
            );

            assert_eq!(
            non_git_paths,
            vec![PathBuf::from("/plain_dir")],
            "plain_dir 应被归类为非 Git 路径\
            （不匹配其内部的 nested_repo）"
            );
        });
    }
    #[gpui::test]
    async fn test_vim_search_does_not_steal_focus_from_agent_panel(cx: &mut TestAppContext) {
        init_test(cx);
        cx.update(|cx| {
            agent::ThreadStore::init_global(cx);
            language_model::LanguageModelRegistry::test(cx);
            vim::init(cx);
            search::init(cx);

            // Enable vim mode
            settings::SettingsStore::update_global(cx, |store, cx| {
                store.update_user_settings(cx, |s| s.vim_mode = Some(true));
            });

            // Load vim keybindings
            let mut vim_key_bindings =
                settings::KeymapFile::load_asset_allow_partial_failure("keymaps/vim.json", cx)
                    .unwrap();
            for key_binding in &mut vim_key_bindings {
                key_binding.set_meta(settings::KeybindSource::Vim.meta());
            }
            cx.bind_keys(vim_key_bindings);
        });

        // Create a project with a file so we have a buffer in the center pane.
        let fs = FakeFs::new(cx.executor());
        fs.insert_tree("/project", json!({ "file.txt": "hello world" }))
            .await;
        let project = Project::test(fs.clone(), [Path::new("/project")], cx).await;

        let multi_workspace =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
        let workspace = multi_workspace
            .read_with(cx, |mw, _cx| mw.workspace().clone())
            .unwrap();
        let mut cx = VisualTestContext::from_window(multi_workspace.into(), cx);

        // Open a file in the center pane.
        workspace
            .update_in(&mut cx, |workspace, window, cx| {
                workspace.open_paths(
                    vec![PathBuf::from("/project/file.txt")],
                    workspace::OpenOptions::default(),
                    None,
                    window,
                    cx,
                )
            })
            .await;
        cx.run_until_parked();

        // Add a BufferSearchBar to the center pane's toolbar, as a real
        // workspace would have.
        workspace.update_in(&mut cx, |workspace, window, cx| {
            workspace.active_pane().update(cx, |pane, cx| {
                pane.toolbar().update(cx, |toolbar, cx| {
                    let search_bar = cx.new(|cx| search::BufferSearchBar::new(None, window, cx));
                    toolbar.add_item(search_bar, window, cx);
                });
            });
        });

        // Create the agent panel and add it to the workspace.
        let panel = workspace.update_in(&mut cx, |workspace, window, cx| {
            let panel = cx.new(|cx| AgentPanel::new(workspace, None, window, cx));
            workspace.add_panel(panel.clone(), window, cx);
            panel
        });

        // Open a thread so the panel has an active editor.
        open_thread_with_connection(&panel, StubAgentConnection::new(), &mut cx);

        // Focus the agent panel.
        workspace.update_in(&mut cx, |workspace, window, cx| {
            workspace.focus_panel::<AgentPanel>(window, cx);
        });
        cx.run_until_parked();

        // Verify the agent panel has focus.
        workspace.update_in(&mut cx, |_, window, cx| {
            assert!(
                panel.read(cx).focus_handle(cx).contains_focused(window, cx),
                "按下 '/' 前应先聚焦代理面板"
            );
        });

        // Press '/' — the vim search keybinding.
        cx.simulate_keystrokes("/");

        // Focus should remain on the agent panel.
        workspace.update_in(&mut cx, |_, window, cx| {
            assert!(
                panel.read(cx).focus_handle(cx).contains_focused(window, cx),
                "按下 '/' 后焦点应保留在代理面板上"
            );
        });
    }

    /// Connection that tracks closed sessions and detects prompts against
    /// sessions that no longer exist, used to reproduce session disassociation.
    #[derive(Clone, Default)]
    struct DisassociationTrackingConnection {
        next_session_number: Arc<Mutex<usize>>,
        sessions: Arc<Mutex<HashSet<acp::SessionId>>>,
        closed_sessions: Arc<Mutex<Vec<acp::SessionId>>>,
        missing_prompt_sessions: Arc<Mutex<Vec<acp::SessionId>>>,
    }

    impl DisassociationTrackingConnection {
        fn new() -> Self {
            Self::default()
        }

        fn create_session(
            self: Rc<Self>,
            session_id: acp::SessionId,
            project: Entity<Project>,
            work_dirs: PathList,
            title: Option<SharedString>,
            cx: &mut App,
        ) -> Entity<AcpThread> {
            self.sessions.lock().insert(session_id.clone());

            let action_log = cx.new(|_| ActionLog::new(project.clone()));
            cx.new(|cx| {
                AcpThread::new(
                    None,
                    title,
                    Some(work_dirs),
                    self,
                    project,
                    action_log,
                    session_id,
                    watch::Receiver::constant(
                        acp::PromptCapabilities::new()
                            .image(true)
                            .audio(true)
                            .embedded_context(true),
                    ),
                    cx,
                )
            })
        }
    }

    impl AgentConnection for DisassociationTrackingConnection {
        fn agent_id(&self) -> AgentId {
            agent::ZED_AGENT_ID.clone()
        }

        fn telemetry_id(&self) -> SharedString {
            "disassociation-tracking-test".into()
        }

        fn new_session(
            self: Rc<Self>,
            project: Entity<Project>,
            work_dirs: PathList,
            cx: &mut App,
        ) -> Task<Result<Entity<AcpThread>>> {
            let session_id = {
                let mut next_session_number = self.next_session_number.lock();
                let session_id = acp::SessionId::new(format!(
                    "disassociation-tracking-session-{}",
                    *next_session_number
                ));
                *next_session_number += 1;
                session_id
            };
            let thread = self.create_session(session_id, project, work_dirs, None, cx);
            Task::ready(Ok(thread))
        }

        fn supports_load_session(&self) -> bool {
            true
        }

        fn load_session(
            self: Rc<Self>,
            session_id: acp::SessionId,
            project: Entity<Project>,
            work_dirs: PathList,
            title: Option<SharedString>,
            cx: &mut App,
        ) -> Task<Result<Entity<AcpThread>>> {
            let thread = self.create_session(session_id, project, work_dirs, title, cx);
            thread.update(cx, |thread, cx| {
                // 处理用户消息块更新
                thread
                    .handle_session_update(
                        acp::SessionUpdate::UserMessageChunk(acp::ContentChunk::new(
                            "已恢复用户消息".into(),
                        )),
                        cx,
                    )
                    .expect("已恢复的用户消息应生效");
                // 处理助手消息块更新
                thread
                    .handle_session_update(
                        acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
                            "已恢复助手消息".into(),
                        )),
                        cx,
                    )
                    .expect("已恢复的助手消息应生效");
            });
            Task::ready(Ok(thread))
        }

        fn supports_close_session(&self) -> bool {
            true
        }

        fn close_session(
            self: Rc<Self>,
            session_id: &acp::SessionId,
            _cx: &mut App,
        ) -> Task<Result<()>> {
            self.sessions.lock().remove(session_id);
            self.closed_sessions.lock().push(session_id.clone());
            Task::ready(Ok(()))
        }

        fn auth_methods(&self) -> &[acp::AuthMethod] {
            &[]
        }

        fn authenticate(&self, _method_id: acp::AuthMethodId, _cx: &mut App) -> Task<Result<()>> {
            Task::ready(Ok(()))
        }

        fn prompt(
            &self,
            _id: UserMessageId,
            params: acp::PromptRequest,
            _cx: &mut App,
        ) -> Task<Result<acp::PromptResponse>> {
            if !self.sessions.lock().contains(&params.session_id) {
                self.missing_prompt_sessions.lock().push(params.session_id);
                return Task::ready(Err(anyhow!("会话未找到")));
            }

            Task::ready(Ok(acp::PromptResponse::new(acp::StopReason::EndTurn)))
        }

        fn cancel(&self, _session_id: &acp::SessionId, _cx: &mut App) {}

        fn into_any(self: Rc<Self>) -> Rc<dyn Any> {
            self
        }
    }

    async fn setup_workspace_panel(
        cx: &mut TestAppContext,
    ) -> (Entity<Workspace>, Entity<AgentPanel>, VisualTestContext) {
        init_test(cx);
        cx.update(|cx| {
            agent::ThreadStore::init_global(cx);
            language_model::LanguageModelRegistry::test(cx);
        });

        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs.clone(), [], cx).await;

        let multi_workspace =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));

        let workspace = multi_workspace
            .read_with(cx, |mw, _cx| mw.workspace().clone())
            .unwrap();

        let mut cx = VisualTestContext::from_window(multi_workspace.into(), cx);

        let panel = workspace.update_in(&mut cx, |workspace, window, cx| {
            let panel = cx.new(|cx| AgentPanel::new(workspace, None, window, cx));
            workspace.add_panel(panel.clone(), window, cx);
            panel
        });

        (workspace, panel, cx)
    }

    /// Reproduces the retained-thread reset race:
    ///
    /// 1. Thread A is active and Connected.
    /// 2. User switches to thread B → A goes to retained_threads.
    /// 3. A thread_error is set on retained A's thread view.
    /// 4. AgentServersUpdated fires → retained A's handle_agent_servers_updated
    ///    sees has_thread_error=true → calls reset() → close_all_sessions →
    ///    session X removed, state = Loading.
    /// 5. User reopens thread X via open_thread → load_agent_thread checks
    ///    retained A's has_session → returns false (state is Loading) →
    ///    creates new ConversationView C.
    /// 6. Both A's reload task and C's load task complete → both call
    ///    load_session(X) → both get Connected with session X.
    /// 7. A is eventually cleaned up → on_release → close_all_sessions →
    ///    removes session X.
    /// 8. C sends → "Session not found".
    #[gpui::test]
    async fn test_retained_thread_reset_race_disassociates_session(cx: &mut TestAppContext) {
        let (_workspace, panel, mut cx) = setup_workspace_panel(cx).await;
        cx.run_until_parked();

        let connection = DisassociationTrackingConnection::new();
        panel.update(&mut cx, |panel, cx| {
            panel.connection_store.update(cx, |store, cx| {
                store.restart_connection(
                    Agent::Stub,
                    Rc::new(StubAgentServer::new(connection.clone())),
                    cx,
                );
            });
        });
        cx.run_until_parked();

        // Step 1: Open thread A and send a message.
        panel.update_in(&mut cx, |panel, window, cx| {
            panel.external_thread(
                Some(Agent::Stub),
                None,
                None,
                None,
                None,
                true,
                "agent_panel",
                window,
                cx,
            );
        });
        cx.run_until_parked();
        send_message(&panel, &mut cx);

        let session_id_a = active_session_id(&panel, &cx);
        let _thread_id_a = active_thread_id(&panel, &cx);

        // Step 2: Open thread B → A goes to retained_threads.
        panel.update_in(&mut cx, |panel, window, cx| {
            panel.external_thread(
                Some(Agent::Stub),
                None,
                None,
                None,
                None,
                true,
                "agent_panel",
                window,
                cx,
            );
        });
        cx.run_until_parked();
        send_message(&panel, &mut cx);

        // Confirm A is retained.
        panel.read_with(&cx, |panel, _cx| {
            assert!(
                panel.retained_threads.contains_key(&_thread_id_a),
                "切换到线程B后，线程A应保留在 retained_threads 中"
            );
        });

        // Step 3: Set a thread_error on retained A's active thread view.
        // This simulates an API error that occurred before the user switched
        // away, or a transient failure.
        let retained_conversation_a = panel.read_with(&cx, |panel, _cx| {
            panel
                .retained_threads
                .get(&_thread_id_a)
                .expect("线程A应被保留")
                .clone()
        });
        retained_conversation_a.update(&mut cx, |conversation, cx| {
            if let Some(thread_view) = conversation.active_thread() {
                thread_view.update(cx, |view, cx| {
                    view.handle_thread_error(
                        crate::conversation_view::ThreadError::Other {
                            message: "模拟错误".into(),
                            acp_error_code: None,
                        },
                        cx,
                    );
                });
            }
        });

        // Confirm the thread error is set.
        retained_conversation_a.read_with(&cx, |conversation, cx| {
            let connected = conversation.as_connected().expect("应处于已连接状态");
            assert!(
                connected.has_thread_error(cx),
                "保留的会话A应存在线程错误"
            );
        });

        // Step 4: Emit AgentServersUpdated → retained A's
        // handle_agent_servers_updated sees has_thread_error=true,
        // calls reset(), which closes session X and sets state=Loading.
        //
        // Critically, we do NOT call run_until_parked between the emit
        // and open_thread. The emit's synchronous effects (event delivery
        // → reset() → close_all_sessions → state=Loading) happen during
        // the update's flush_effects. But the async reload task spawned
        // by initial_state has NOT been polled yet.
        panel.update(&mut cx, |panel, cx| {
            panel.project.update(cx, |project, cx| {
                project
                    .agent_server_store()
                    .update(cx, |_store, cx| cx.emit(project::AgentServersUpdated));
            });
        });
        // After this update returns, the retained ConversationView is in
        // Loading state (reset ran synchronously), but its async reload
        // task hasn't executed yet.

        // Step 5: Immediately open thread X via open_thread, BEFORE
        // the retained view's async reload completes. load_agent_thread
        // checks retained A's has_session → returns false (state is
        // Loading) → creates a NEW ConversationView C for session X.
        panel.update_in(&mut cx, |panel, window, cx| {
            panel.open_thread(session_id_a.clone(), None, None, window, cx);
        });

        // NOW settle everything: both async tasks (A's reload and C's load)
        // complete, both register session X.
        cx.run_until_parked();

        // Verify session A is the active session via C.
        panel.read_with(&cx, |panel, cx| {
            let active_session = panel
                .active_agent_thread(cx)
                .map(|t| t.read(cx).session_id().clone());
            assert_eq!(
                active_session,
                Some(session_id_a.clone()),
                "执行 open_thread 后，会话A应成为当前活动会话"
            );
        });

        // Step 6: Force the retained ConversationView A to be dropped
        // while the active view (C) still has the same session.
        // We can't use remove_thread because C shares the same ThreadId
        // and remove_thread would kill the active view too. Instead,
        // directly remove from retained_threads and drop the handle
        // so on_release → close_all_sessions fires only on A.
        drop(retained_conversation_a);
        panel.update(&mut cx, |panel, _cx| {
            panel.retained_threads.remove(&_thread_id_a);
        });
        cx.run_until_parked();

        // The key assertion: sending messages on the ACTIVE view (C)
        // must succeed. If the session was disassociated by A's cleanup,
        // this will fail with "Session not found".
        send_message(&panel, &mut cx);
        send_message(&panel, &mut cx);

        let missing = connection.missing_prompt_sessions.lock().clone();
        assert!(
            missing.is_empty(),
            "保留的线程重置竞争后，会话不应解除关联，\
             缺失的提示会话：{:?}",
            missing
        );

        panel.read_with(&cx, |panel, cx| {
            let active_view = panel
                .active_conversation_view()
                .expect("会话应保持打开状态");
            let connected = active_view
                .read(cx)
                .as_connected()
                .expect("会话应处于已连接状态");
            assert!(
                !connected.has_thread_error(cx),
                "会话不应存在线程错误"
            );
        });
    }

    #[gpui::test]
    async fn test_initialize_from_source_transfers_draft_to_fresh_panel(cx: &mut TestAppContext) {
        init_test(cx);
        cx.update(|cx| {
            agent::ThreadStore::init_global(cx);
            language_model::LanguageModelRegistry::test(cx);
        });

        let fs = FakeFs::new(cx.executor());
        let project_a = Project::test(fs.clone(), [], cx).await;
        let project_b = Project::test(fs.clone(), [], cx).await;

        let multi_workspace =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project_a.clone(), window, cx));

        let workspace_a = multi_workspace
            .read_with(cx, |mw, _cx| mw.workspace().clone())
            .unwrap();

        let workspace_b = multi_workspace
            .update(cx, |multi_workspace, window, cx| {
                multi_workspace.test_add_workspace(project_b.clone(), window, cx)
            })
            .unwrap();

        let cx = &mut VisualTestContext::from_window(multi_workspace.into(), cx);

        // Set up panel_a with an active thread and type draft text.
        let panel_a = workspace_a.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| AgentPanel::new(workspace, None, window, cx));
            workspace.add_panel(panel.clone(), window, cx);
            panel
        });
        cx.run_until_parked();

        panel_a.update_in(cx, |panel, window, cx| {
            panel.open_external_thread_with_server(
                Rc::new(StubAgentServer::default_response()),
                window,
                cx,
            );
        });
        cx.run_until_parked();

        let thread_view_a =
            panel_a.read_with(cx, |panel, cx| panel.active_thread_view(cx).unwrap());
        let editor_a = thread_view_a.read_with(cx, |view, _cx| view.message_editor.clone());
        editor_a.update_in(cx, |editor, window, cx| {
            editor.set_text("Draft from workspace A", window, cx);
        });

        // Set up panel_b on workspace_b — starts as a fresh, empty panel.
        let panel_b = workspace_b.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| AgentPanel::new(workspace, None, window, cx));
            workspace.add_panel(panel.clone(), window, cx);
            panel
        });
        cx.run_until_parked();

        // Initializing panel_b from workspace_a should transfer the draft,
        // even if panel_b already has an auto-created empty draft thread
        // (which set_active creates during add_panel).
        let transferred = panel_b.update_in(cx, |panel, window, cx| {
            panel.initialize_from_source_workspace_if_needed(workspace_a.downgrade(), window, cx)
        });
        assert!(
            transferred,
            "全新的目标面板应接收源内容"
        );

        // Verify the panel was initialized: the base_view should now be an
        // AgentThread (not Uninitialized) and a draft_thread should be set.
        // We can't check the message editor text directly because the thread
        // needs a connected server session (not available in unit tests without
        // a stub server). The `transferred == true` return already proves that
        // source_panel_initialization read the content successfully.
        panel_b.read_with(cx, |panel, _cx| {
            assert!(
                panel.active_conversation_view().is_some(),
                "初始化后面板B应存在会话视图"
            );
            assert!(
                panel.draft_thread.is_some(),
                "初始化后面板B应设置草稿线程"
            );
        });
    }

    #[gpui::test]
    async fn test_initialize_from_source_does_not_overwrite_existing_content(
        cx: &mut TestAppContext,
    ) {
        init_test(cx);
        cx.update(|cx| {
            agent::ThreadStore::init_global(cx);
            language_model::LanguageModelRegistry::test(cx);
        });

        let fs = FakeFs::new(cx.executor());
        let project_a = Project::test(fs.clone(), [], cx).await;
        let project_b = Project::test(fs.clone(), [], cx).await;

        let multi_workspace =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project_a.clone(), window, cx));

        let workspace_a = multi_workspace
            .read_with(cx, |mw, _cx| mw.workspace().clone())
            .unwrap();

        let workspace_b = multi_workspace
            .update(cx, |multi_workspace, window, cx| {
                multi_workspace.test_add_workspace(project_b.clone(), window, cx)
            })
            .unwrap();

        let cx = &mut VisualTestContext::from_window(multi_workspace.into(), cx);

        // Set up panel_a with draft text.
        let panel_a = workspace_a.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| AgentPanel::new(workspace, None, window, cx));
            workspace.add_panel(panel.clone(), window, cx);
            panel
        });
        cx.run_until_parked();

        panel_a.update_in(cx, |panel, window, cx| {
            panel.open_external_thread_with_server(
                Rc::new(StubAgentServer::default_response()),
                window,
                cx,
            );
        });
        cx.run_until_parked();

        let thread_view_a =
            panel_a.read_with(cx, |panel, cx| panel.active_thread_view(cx).unwrap());
        let editor_a = thread_view_a.read_with(cx, |view, _cx| view.message_editor.clone());
        editor_a.update_in(cx, |editor, window, cx| {
            editor.set_text("Draft from workspace A", window, cx);
        });

        // Set up panel_b with its OWN content — this is a non-fresh panel.
        let panel_b = workspace_b.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| AgentPanel::new(workspace, None, window, cx));
            workspace.add_panel(panel.clone(), window, cx);
            panel
        });
        cx.run_until_parked();

        panel_b.update_in(cx, |panel, window, cx| {
            panel.open_external_thread_with_server(
                Rc::new(StubAgentServer::default_response()),
                window,
                cx,
            );
        });
        cx.run_until_parked();

        let thread_view_b =
            panel_b.read_with(cx, |panel, cx| panel.active_thread_view(cx).unwrap());
        let editor_b = thread_view_b.read_with(cx, |view, _cx| view.message_editor.clone());
        editor_b.update_in(cx, |editor, window, cx| {
            editor.set_text("Existing work in workspace B", window, cx);
        });

        // Attempting to initialize panel_b from workspace_a should be rejected
        // because panel_b already has meaningful content.
        let transferred = panel_b.update_in(cx, |panel, window, cx| {
            panel.initialize_from_source_workspace_if_needed(workspace_a.downgrade(), window, cx)
        });
        assert!(
            !transferred,
            "已有内容的目标面板不应被覆盖"
        );

        // Verify panel_b still has its original content.
        panel_b.read_with(cx, |panel, cx| {
            let thread_view = panel
                .active_thread_view(cx)
                .expect("面板B应仍保留其线程视图");
            let text = thread_view.read(cx).message_editor.read(cx).text(cx);
            assert_eq!(
                text, "Existing work in workspace B",
                "目标面板的内容应被保留"
            );
        });
    }
}
