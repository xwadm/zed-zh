use acp_thread::AgentSessionListRequest;
use agent::ThreadStore;
use agent_client_protocol::schema as acp;
use chrono::Utc;
use collections::HashSet;
use db::kvp::Dismissable;
use db::sqlez;
use fs::Fs;
use futures::FutureExt as _;
use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, MouseDownEvent,
    Render, SharedString, Task, WeakEntity, Window,
};
use itertools::Itertools as _;
use notifications::status_toast::StatusToast;
use project::{AgentId, AgentRegistryStore, AgentServerStore};
use release_channel::ReleaseChannel;
use remote::RemoteConnectionOptions;
use ui::{
    Checkbox, KeyBinding, ListItem, ListItemSpacing, Modal, ModalFooter, ModalHeader, Section,
    prelude::*,
};
use util::ResultExt;
use workspace::{ModalView, MultiWorkspace, Workspace};

use crate::{
    Agent, AgentPanel,
    agent_connection_store::AgentConnectionStore,
    thread_metadata_store::{ThreadId, ThreadMetadata, ThreadMetadataStore, WorktreePaths},
};

/// ACP线程导入引导标记
pub struct AcpThreadImportOnboarding;
/// 跨频道导入引导标记
pub struct CrossChannelImportOnboarding;

impl AcpThreadImportOnboarding {
    /// 检查是否已关闭该引导
    pub fn dismissed(cx: &App) -> bool {
        <Self as Dismissable>::dismissed(cx)
    }

    /// 关闭该引导
    pub fn dismiss(cx: &mut App) {
        <Self as Dismissable>::set_dismissed(true, cx);
    }
}

impl Dismissable for AcpThreadImportOnboarding {
    const KEY: &'static str = "dismissed-acp-thread-import";
}

impl CrossChannelImportOnboarding {
    /// 检查是否已关闭该引导
    pub fn dismissed(cx: &App) -> bool {
        <Self as Dismissable>::dismissed(cx)
    }

    /// 关闭该引导
    pub fn dismiss(cx: &mut App) {
        <Self as Dismissable>::set_dismissed(true, cx);
    }
}

impl Dismissable for CrossChannelImportOnboarding {
    const KEY: &'static str = "dismissed-cross-channel-thread-import";
}

/// 返回非开发版、非当前发布频道且数据库中至少有一个线程的频道列表
/// 结果适用于构建用户可见的提示信息（例如：从Zed预览版和每日构建版）
pub fn channels_with_threads(cx: &App) -> Vec<ReleaseChannel> {
    let Some(current_channel) = ReleaseChannel::try_global(cx) else {
        return Vec::new();
    };
    let database_dir = paths::database_dir();

    ReleaseChannel::ALL
        .iter()
        .copied()
        .filter(|channel| {
            *channel != current_channel
                && *channel != ReleaseChannel::Dev
                && channel_has_threads(database_dir, *channel)
        })
        .collect()
}

#[derive(Clone)]
struct AgentEntry {
    agent_id: AgentId,
    display_name: SharedString,
    icon_path: Option<SharedString>,
}

/// 线程导入弹窗
pub struct ThreadImportModal {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    multi_workspace: WeakEntity<MultiWorkspace>,
    agent_entries: Vec<AgentEntry>,
    unchecked_agents: HashSet<AgentId>,
    selected_index: Option<usize>,
    is_importing: bool,
    last_error: Option<SharedString>,
}

impl ThreadImportModal {
    /// 创建线程导入弹窗实例
    pub fn new(
        agent_server_store: Entity<AgentServerStore>,
        agent_registry_store: Entity<AgentRegistryStore>,
        workspace: WeakEntity<Workspace>,
        multi_workspace: WeakEntity<MultiWorkspace>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        AcpThreadImportOnboarding::dismiss(cx);

        let agent_entries = agent_server_store
            .read(cx)
            .external_agents()
            .map(|agent_id| {
                let display_name = agent_server_store
                    .read(cx)
                    .agent_display_name(agent_id)
                    .or_else(|| {
                        agent_registry_store
                            .read(cx)
                            .agent(agent_id)
                            .map(|agent| agent.name().clone())
                    })
                    .unwrap_or_else(|| agent_id.0.clone());
                let icon_path = agent_server_store
                    .read(cx)
                    .agent_icon(agent_id)
                    .or_else(|| {
                        agent_registry_store
                            .read(cx)
                            .agent(agent_id)
                            .and_then(|agent| agent.icon_path().cloned())
                    });

                AgentEntry {
                    agent_id: agent_id.clone(),
                    display_name,
                    icon_path,
                }
            })
            .sorted_unstable_by_key(|entry| entry.display_name.to_lowercase())
            .collect::<Vec<_>>();

        Self {
            focus_handle: cx.focus_handle(),
            workspace,
            multi_workspace,
            agent_entries,
            unchecked_agents: HashSet::default(),
            selected_index: None,
            is_importing: false,
            last_error: None,
        }
    }

    /// 获取所有代理ID
    fn agent_ids(&self) -> Vec<AgentId> {
        self.agent_entries
            .iter()
            .map(|entry| entry.agent_id.clone())
            .collect()
    }

    /// 切换代理的选中状态
    fn toggle_agent_checked(&mut self, agent_id: AgentId, cx: &mut Context<Self>) {
        if self.unchecked_agents.contains(&agent_id) {
            self.unchecked_agents.remove(&agent_id);
        } else {
            self.unchecked_agents.insert(agent_id);
        }
        cx.notify();
    }

    /// 选择下一个代理
    fn select_next(&mut self, _: &menu::SelectNext, _window: &mut Window, cx: &mut Context<Self>) {
        if self.agent_entries.is_empty() {
            return;
        }
        self.selected_index = Some(match self.selected_index {
            Some(ix) if ix + 1 >= self.agent_entries.len() => 0,
            Some(ix) => ix + 1,
            None => 0,
        });
        cx.notify();
    }

    /// 选择上一个代理
    fn select_previous(
        &mut self,
        _: &menu::SelectPrevious,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.agent_entries.is_empty() {
            return;
        }
        self.selected_index = Some(match self.selected_index {
            Some(0) => self.agent_entries.len() - 1,
            Some(ix) => ix - 1,
            None => self.agent_entries.len() - 1,
        });
        cx.notify();
    }

    /// 确认选择
    fn confirm(&mut self, _: &menu::Confirm, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.selected_index {
            if let Some(entry) = self.agent_entries.get(ix) {
                self.toggle_agent_checked(entry.agent_id.clone(), cx);
            }
        }
    }

    /// 取消弹窗
    fn cancel(&mut self, _: &menu::Cancel, _window: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }

    /// 导入选中代理的线程
    fn import_threads(
        &mut self,
        _: &menu::SecondaryConfirm,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_importing {
            return;
        }

        let Some(multi_workspace) = self.multi_workspace.upgrade() else {
            self.is_importing = false;
            cx.notify();
            return;
        };

        let stores = resolve_agent_connection_stores(&multi_workspace, cx);
        if stores.is_empty() {
            log::error!("未找到可导入的工作空间");
            self.is_importing = false;
            cx.notify();
            return;
        }

        self.is_importing = true;
        self.last_error = None;
        cx.notify();

        let agent_ids = self
            .agent_ids()
            .into_iter()
            .filter(|agent_id| !self.unchecked_agents.contains(agent_id))
            .collect::<Vec<_>>();

        let existing_sessions: HashSet<acp::SessionId> = ThreadMetadataStore::global(cx)
            .read(cx)
            .entries()
            .filter_map(|m| m.session_id.clone())
            .collect();

        let task = find_threads_to_import(agent_ids, existing_sessions, stores, cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| match result {
                Ok(threads) => {
                    let imported_count = threads.len();
                    ThreadMetadataStore::global(cx)
                        .update(cx, |store, cx| store.save_all(threads, cx));
                    this.is_importing = false;
                    this.last_error = None;
                    this.show_imported_threads_toast(imported_count, cx);
                    cx.emit(DismissEvent);
                }
                Err(error) => {
                    this.is_importing = false;
                    this.last_error = Some(error.to_string().into());
                    cx.notify();
                }
            })
        })
        .detach_and_log_err(cx);
    }

    /// 显示线程导入结果提示
    fn show_imported_threads_toast(&self, imported_count: usize, cx: &mut App) {
        let status_toast = if imported_count == 0 {
            StatusToast::new("未找到可导入的线程。", cx, |this, _cx| {
                this.icon(
                    Icon::new(IconName::Info)
                        .size(IconSize::Small)
                        .color(Color::Muted),
                )
                .dismiss_button(true)
            })
        } else {
            let message = if imported_count == 1 {
                "已导入1个线程。".to_string()
            } else {
                format!("已导入{imported_count}个线程。")
            };
            StatusToast::new(message, cx, |this, _cx| {
                this.icon(
                    Icon::new(IconName::Check)
                        .size(IconSize::Small)
                        .color(Color::Success),
                )
                .dismiss_button(true)
            })
        };

        self.workspace
            .update(cx, |workspace, cx| {
                workspace.toggle_status_toast(status_toast, cx);
            })
            .log_err();
    }
}

impl EventEmitter<DismissEvent> for ThreadImportModal {}

impl Focusable for ThreadImportModal {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ModalView for ThreadImportModal {}

impl Render for ThreadImportModal {
    /// 渲染弹窗界面
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_agents = !self.agent_entries.is_empty();
        let disabled_import_thread = self.is_importing
            || !has_agents
            || self.unchecked_agents.len() == self.agent_entries.len();

        let agent_rows = self
            .agent_entries
            .iter()
            .enumerate()
            .map(|(ix, entry)| {
                let is_checked = !self.unchecked_agents.contains(&entry.agent_id);
                let is_focused = self.selected_index == Some(ix);

                ListItem::new(("thread-import-agent", ix))
                    .rounded()
                    .spacing(ListItemSpacing::Sparse)
                    .focused(is_focused)
                    .disabled(self.is_importing)
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .when(!is_checked, |this| this.opacity(0.6))
                            .child(if let Some(icon_path) = entry.icon_path.clone() {
                                Icon::from_external_svg(icon_path)
                                    .color(Color::Muted)
                                    .size(IconSize::Small)
                            } else {
                                Icon::new(IconName::Sparkle)
                                    .color(Color::Muted)
                                    .size(IconSize::Small)
                            })
                            .child(Label::new(entry.display_name.clone())),
                    )
                    .end_slot(Checkbox::new(
                        ("thread-import-agent-checkbox", ix),
                        if is_checked {
                            ToggleState::Selected
                        } else {
                            ToggleState::Unselected
                        },
                    ))
                    .on_click({
                        let agent_id = entry.agent_id.clone();
                        cx.listener(move |this, _event, _window, cx| {
                            this.toggle_agent_checked(agent_id.clone(), cx);
                        })
                    })
            })
            .collect::<Vec<_>>();

        v_flex()
            .id("thread-import-modal")
            .key_context("ThreadImportModal")
            .w(rems(34.))
            .elevation_3(cx)
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::import_threads))
            .on_any_mouse_down(cx.listener(|this, _: &MouseDownEvent, window, cx| {
                this.focus_handle.focus(window, cx);
            }))
            .child(
                Modal::new("import-threads", None)
                    .header(
                        ModalHeader::new()
                            .headline("导入外部代理线程")
                            .description(
                                "从Claude Agent、Codex等代理导入线程，无论线程是在Zed还是其他客户端中创建的。\
                                选择需要导入的代理，其线程将显示在你的线程历史中。"
                            )
                            .show_dismiss_button(true),

                    )
                    .section(
                        Section::new().child(
                            v_flex()
                                .id("thread-import-agent-list")
                                .max_h(rems_from_px(320.))
                                .pb_1()
                                .overflow_y_scroll()
                                .when(has_agents, |this| this.children(agent_rows))
                                .when(!has_agents, |this| {
                                    this.child(
                                        Label::new("暂无ACP代理可用。")
                                            .color(Color::Muted)
                                            .size(LabelSize::Small),
                                    )
                                }),
                        ),
                    )
                    .footer(
                        ModalFooter::new()
                            .when_some(self.last_error.clone(), |this, error| {
                                this.start_slot(
                                    Label::new(error)
                                        .size(LabelSize::Small)
                                        .color(Color::Error)
                                        .truncate(),
                                )
                            })
                            .end_slot(
                                Button::new("import-threads", "导入线程")
                                    .loading(self.is_importing)
                                    .disabled(disabled_import_thread)
                                    .key_binding(
                                        KeyBinding::for_action(&menu::SecondaryConfirm, cx)
                                            .map(|kb| kb.size(rems_from_px(12.))),
                                    )
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.import_threads(&menu::SecondaryConfirm, window, cx);
                                    })),
                            ),
                    ),
            )
    }
}

/// 解析代理连接存储
fn resolve_agent_connection_stores(
    multi_workspace: &Entity<MultiWorkspace>,
    cx: &App,
) -> Vec<Entity<AgentConnectionStore>> {
    let mut stores = Vec::new();
    let mut included_local_store = false;

    for workspace in multi_workspace.read(cx).workspaces() {
        let workspace = workspace.read(cx);
        let project = workspace.project().read(cx);

        // 仅从一个本地工作空间获取数据，因为它们位于同一台机器上
        let include_store = if project.is_remote() {
            true
        } else if project.is_local() && !included_local_store {
            included_local_store = true;
            true
        } else {
            false
        };

        if !include_store {
            continue;
        }

        if let Some(panel) = workspace.panel::<AgentPanel>(cx) {
            stores.push(panel.read(cx).connection_store().clone());
        }
    }

    stores
}

/// 查找可导入的线程
fn find_threads_to_import(
    agent_ids: Vec<AgentId>,
    existing_sessions: HashSet<acp::SessionId>,
    stores: Vec<Entity<AgentConnectionStore>>,
    cx: &mut App,
) -> Task<anyhow::Result<Vec<ThreadMetadata>>> {
    let mut wait_for_connection_tasks = Vec::new();

    for store in stores {
        let remote_connection = store
            .read(cx)
            .project()
            .read(cx)
            .remote_connection_options(cx);

        for agent_id in agent_ids.clone() {
            let agent = Agent::from(agent_id.clone());
            let server = agent.server(<dyn Fs>::global(cx), ThreadStore::global(cx));
            let entry = store.update(cx, |store, cx| store.request_connection(agent, server, cx));

            wait_for_connection_tasks.push(entry.read(cx).wait_for_connection().map({
                let remote_connection = remote_connection.clone();
                move |state| (agent_id, remote_connection, state)
            }));
        }
    }

    cx.spawn(async move |cx| {
        let results = futures::future::join_all(wait_for_connection_tasks).await;

        let mut page_tasks = Vec::new();
        for (agent_id, remote_connection, result) in results {
            let Some(state) = result.log_err() else {
                continue;
            };
            let Some(list) = cx.update(|cx| state.connection.session_list(cx)) else {
                continue;
            };
            page_tasks.push(cx.spawn({
                let list = list.clone();
                async move |cx| collect_all_sessions(agent_id, remote_connection, list, cx).await
            }));
        }

        let sessions_by_agent = futures::future::join_all(page_tasks)
            .await
            .into_iter()
            .filter_map(|result| result.log_err())
            .collect();

        Ok(collect_importable_threads(
            sessions_by_agent,
            existing_sessions,
        ))
    })
}

/// 收集所有会话
async fn collect_all_sessions(
    agent_id: AgentId,
    remote_connection: Option<RemoteConnectionOptions>,
    list: std::rc::Rc<dyn acp_thread::AgentSessionList>,
    cx: &mut gpui::AsyncApp,
) -> anyhow::Result<SessionByAgent> {
    let mut sessions = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let request = AgentSessionListRequest {
            cursor: cursor.clone(),
            ..Default::default()
        };
        let task = cx.update(|cx| list.list_sessions(request, cx));
        let response = task.await?;
        sessions.extend(response.sessions);
        match response.next_cursor {
            Some(next) if Some(&next) != cursor.as_ref() => cursor = Some(next),
            _ => break,
        }
    }
    Ok(SessionByAgent {
        agent_id,
        remote_connection,
        sessions,
    })
}

/// 按代理分组的会话数据
struct SessionByAgent {
    agent_id: AgentId,
    remote_connection: Option<RemoteConnectionOptions>,
    sessions: Vec<acp_thread::AgentSessionInfo>,
}

/// 收集可导入的线程（去重、过滤无效数据）
fn collect_importable_threads(
    sessions_by_agent: Vec<SessionByAgent>,
    mut existing_sessions: HashSet<acp::SessionId>,
) -> Vec<ThreadMetadata> {
    let mut to_insert = Vec::new();
    for SessionByAgent {
        agent_id,
        remote_connection,
        sessions,
    } in sessions_by_agent
    {
        for session in sessions {
            if !existing_sessions.insert(session.session_id.clone()) {
                continue;
            }
            let Some(folder_paths) = session.work_dirs else {
                continue;
            };
            to_insert.push(ThreadMetadata {
                thread_id: ThreadId::new(),
                session_id: Some(session.session_id),
                agent_id: agent_id.clone(),
                title: session.title,
                updated_at: session.updated_at.unwrap_or_else(|| Utc::now()),
                created_at: session.created_at,
                interacted_at: None,
                worktree_paths: WorktreePaths::from_folder_paths(&folder_paths),
                remote_connection: remote_connection.clone(),
                archived: true,
            });
        }
    }
    to_insert
}

/// 从其他频道导入线程
pub fn import_threads_from_other_channels(_workspace: &mut Workspace, cx: &mut Context<Workspace>) {
    let database_dir = paths::database_dir().clone();
    import_threads_from_other_channels_in(database_dir, cx);
}

/// 在指定目录中从其他频道导入线程
fn import_threads_from_other_channels_in(
    database_dir: std::path::PathBuf,
    cx: &mut Context<Workspace>,
) {
    let current_channel = ReleaseChannel::global(cx);

    let existing_thread_ids: HashSet<ThreadId> = ThreadMetadataStore::global(cx)
        .read(cx)
        .entries()
        .map(|metadata| metadata.thread_id)
        .collect();

    let workspace_handle = cx.weak_entity();
    cx.spawn(async move |_this, cx| {
        let mut imported_threads = Vec::new();

        for channel in &ReleaseChannel::ALL {
            if *channel == current_channel || *channel == ReleaseChannel::Dev {
                continue;
            }

            match read_threads_from_channel(&database_dir, *channel) {
                Ok(threads) => {
                    let new_threads = threads
                        .into_iter()
                        .filter(|thread| !existing_thread_ids.contains(&thread.thread_id));
                    imported_threads.extend(new_threads);
                }
                Err(error) => {
                    log::warn!(
                        "从{}频道数据库读取线程失败：{}",
                        channel.dev_name(),
                        error
                    );
                }
            }
        }

        let imported_count = imported_threads.len();

        cx.update(|cx| {
            ThreadMetadataStore::global(cx)
                .update(cx, |store, cx| store.save_all(imported_threads, cx));

            show_cross_channel_import_toast(&workspace_handle, imported_count, cx);
        })
    })
    .detach();
}

/// 检查指定频道是否存在线程
fn channel_has_threads(database_dir: &std::path::Path, channel: ReleaseChannel) -> bool {
    let db_path = db::db_path(database_dir, channel);
    if !db_path.exists() {
        return false;
    }
    let connection = sqlez::connection::Connection::open_file(&db_path.to_string_lossy());
    connection
        .select_row::<bool>("SELECT 1 FROM sidebar_threads LIMIT 1")
        .ok()
        .and_then(|mut query| query().ok().flatten())
        .unwrap_or(false)
}

/// 从指定频道读取线程元数据
fn read_threads_from_channel(
    database_dir: &std::path::Path,
    channel: ReleaseChannel,
) -> anyhow::Result<Vec<ThreadMetadata>> {
    let db_path = db::db_path(database_dir, channel);
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    let connection = sqlez::connection::Connection::open_file(&db_path.to_string_lossy());
    crate::thread_metadata_store::list_thread_metadata_from_connection(&connection)
}

/// 显示跨频道导入结果提示
fn show_cross_channel_import_toast(
    workspace: &WeakEntity<Workspace>,
    imported_count: usize,
    cx: &mut App,
) {
    let status_toast = if imported_count == 0 {
        StatusToast::new("未找到可导入的新线程。", cx, |this, _cx| {
            this.icon(Icon::new(IconName::Info).color(Color::Muted))
                .dismiss_button(true)
        })
    } else {
        let message = if imported_count == 1 {
            "从其他频道导入了1个线程。".to_string()
        } else {
            format!("从其他频道导入了{imported_count}个线程。")
        };
        StatusToast::new(message, cx, |this, _cx| {
            this.icon(Icon::new(IconName::Check).color(Color::Success))
                .dismiss_button(true)
        })
    };

    workspace
        .update(cx, |workspace, cx| {
            workspace.toggle_status_toast(status_toast, cx);
        })
        .log_err();
}

#[cfg(test)]
mod tests {
    use super::*;
    use acp_thread::AgentSessionInfo;
    use chrono::Utc;
    use gpui::TestAppContext;
    use std::path::Path;
    use workspace::PathList;

    /// 创建测试会话
    fn make_session(
        session_id: &str,
        title: Option<&str>,
        work_dirs: Option<PathList>,
        updated_at: Option<chrono::DateTime<Utc>>,
        created_at: Option<chrono::DateTime<Utc>>,
    ) -> AgentSessionInfo {
        AgentSessionInfo {
            session_id: acp::SessionId::new(session_id),
            title: title.map(|t| SharedString::from(t.to_string())),
            work_dirs,
            updated_at,
            created_at,
            meta: None,
        }
    }

    /// 测试：跳过已存在的会话
    #[test]
    fn test_collect_skips_sessions_already_in_existing_set() {
        let existing = HashSet::from_iter(vec![acp::SessionId::new("existing-1")]);
        let paths = PathList::new(&[Path::new("/project")]);

        let sessions_by_agent = vec![SessionByAgent {
            agent_id: AgentId::new("agent-a"),
            remote_connection: None,
            sessions: vec![
                make_session(
                    "existing-1",
                    Some("已存在"),
                    Some(paths.clone()),
                    None,
                    None,
                ),
                make_session("new-1", Some("全新线程"), Some(paths), None, None),
            ],
        }];

        let result = collect_importable_threads(sessions_by_agent, existing);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].session_id.as_ref().unwrap().0.as_ref(), "new-1");
        assert_eq!(result[0].display_title(), "全新线程");
    }

    /// 测试：跳过无工作目录的会话
    #[test]
    fn test_collect_skips_sessions_without_work_dirs() {
        let existing = HashSet::default();
        let paths = PathList::new(&[Path::new("/project")]);

        let sessions_by_agent = vec![SessionByAgent {
            agent_id: AgentId::new("agent-a"),
            remote_connection: None,
            sessions: vec![
                make_session("has-dirs", Some("带目录"), Some(paths), None, None),
                make_session("no-dirs", Some("无目录"), None, None, None),
            ],
        }];

        let result = collect_importable_threads(sessions_by_agent, existing);

        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0].session_id.as_ref().unwrap().0.as_ref(),
            "has-dirs"
        );
    }

    /// 测试：所有导入的线程标记为已归档
    #[test]
    fn test_collect_marks_all_imported_threads_as_archived() {
        let existing = HashSet::default();
        let paths = PathList::new(&[Path::new("/project")]);

        let sessions_by_agent = vec![SessionByAgent {
            agent_id: AgentId::new("agent-a"),
            remote_connection: None,
            sessions: vec![
                make_session("s1", Some("线程1"), Some(paths.clone()), None, None),
                make_session("s2", Some("线程2"), Some(paths), None, None),
            ],
        }];

        let result = collect_importable_threads(sessions_by_agent, existing);

        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|t| t.archived));
    }

    /// 测试：为每个会话分配正确的代理ID
    #[test]
    fn test_collect_assigns_correct_agent_id_per_session() {
        let existing = HashSet::default();
        let paths = PathList::new(&[Path::new("/project")]);

        let sessions_by_agent = vec![
            SessionByAgent {
                agent_id: AgentId::new("agent-a"),
                remote_connection: None,
                sessions: vec![make_session(
                    "s1",
                    Some("来自A"),
                    Some(paths.clone()),
                    None,
                    None,
                )],
            },
            SessionByAgent {
                agent_id: AgentId::new("agent-b"),
                remote_connection: None,
                sessions: vec![make_session("s2", Some("来自B"), Some(paths), None, None)],
            },
        ];

        let result = collect_importable_threads(sessions_by_agent, existing);

        assert_eq!(result.len(), 2);
        let s1 = result
            .iter()
            .find(|t| t.session_id.as_ref().map(|s| s.0.as_ref()) == Some("s1"))
            .unwrap();
        let s2 = result
            .iter()
            .find(|t| t.session_id.as_ref().map(|s| s.0.as_ref()) == Some("s2"))
            .unwrap();
        assert_eq!(s1.agent_id.as_ref(), "agent-a");
        assert_eq!(s2.agent_id.as_ref(), "agent-b");
    }

    /// 测试：跨代理去重会话
    #[test]
    fn test_collect_deduplicates_across_agents() {
        let existing = HashSet::default();
        let paths = PathList::new(&[Path::new("/project")]);

        let sessions_by_agent = vec![
            SessionByAgent {
                agent_id: AgentId::new("agent-a"),
                remote_connection: None,
                sessions: vec![make_session(
                    "shared-session",
                    Some("来自A"),
                    Some(paths.clone()),
                    None,
                    None,
                )],
            },
            SessionByAgent {
                agent_id: AgentId::new("agent-b"),
                remote_connection: None,
                sessions: vec![make_session(
                    "shared-session",
                    Some("来自B"),
                    Some(paths),
                    None,
                    None,
                )],
            },
        ];

        let result = collect_importable_threads(sessions_by_agent, existing);

        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0].session_id.as_ref().unwrap().0.as_ref(),
            "shared-session"
        );
        assert_eq!(
            result[0].agent_id.as_ref(),
            "agent-a",
            "优先保留第一个遇到的代理"
        );
    }

    /// 测试：全部已存在时返回空列表
    #[test]
    fn test_collect_all_existing_returns_empty() {
        let paths = PathList::new(&[Path::new("/project")]);
        let existing =
            HashSet::from_iter(vec![acp::SessionId::new("s1"), acp::SessionId::new("s2")]);

        let sessions_by_agent = vec![SessionByAgent {
            agent_id: AgentId::new("agent-a"),
            remote_connection: None,
            sessions: vec![
                make_session("s1", Some("线程1"), Some(paths.clone()), None, None),
                make_session("s2", Some("线程2"), Some(paths), None, None),
            ],
        }];

        let result = collect_importable_threads(sessions_by_agent, existing);
        assert!(result.is_empty());
    }

    /// 创建频道数据库
    fn create_channel_db(
        db_dir: &std::path::Path,
        channel: ReleaseChannel,
    ) -> db::sqlez::connection::Connection {
        let db_path = db::db_path(db_dir, channel);
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let connection = db::sqlez::connection::Connection::open_file(&db_path.to_string_lossy());
        crate::thread_metadata_store::run_thread_metadata_migrations(&connection);
        connection
    }

    /// 插入测试线程
    fn insert_thread(
        connection: &db::sqlez::connection::Connection,
        title: &str,
        updated_at: &str,
        archived: bool,
    ) {
        let thread_id = uuid::Uuid::new_v4();
        let session_id = uuid::Uuid::new_v4().to_string();
        connection
            .exec_bound::<(uuid::Uuid, &str, &str, &str, bool)>(
                "INSERT INTO sidebar_threads \
                 (thread_id, session_id, title, updated_at, archived) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .unwrap()((thread_id, session_id.as_str(), title, updated_at, archived))
        .unwrap();
    }

    /// 测试：频道数据库不存在时返回空
    #[test]
    fn test_returns_empty_when_channel_db_missing() {
        let dir = tempfile::tempdir().unwrap();
        let threads = read_threads_from_channel(dir.path(), ReleaseChannel::Nightly).unwrap();
        assert!(threads.is_empty());
    }

    /// 测试：保留归档状态
    #[test]
    fn test_preserves_archived_state() {
        let dir = tempfile::tempdir().unwrap();
        let connection = create_channel_db(dir.path(), ReleaseChannel::Nightly);

        insert_thread(&connection, "活跃线程", "2025-01-15T10:00:00Z", false);
        insert_thread(&connection, "归档线程", "2025-01-15T09:00:00Z", true);
        drop(connection);

        let threads = read_threads_from_channel(dir.path(), ReleaseChannel::Nightly).unwrap();
        assert_eq!(threads.len(), 2);

        let active = threads
            .iter()
            .find(|t| t.display_title().as_ref() == "活跃线程")
            .unwrap();
        assert!(!active.archived);

        let archived = threads
            .iter()
            .find(|t| t.display_title().as_ref() == "归档线程")
            .unwrap();
        assert!(archived.archived);
    }

    /// 初始化测试环境
    fn init_test(cx: &mut TestAppContext) {
        let fs = fs::FakeFs::new(cx.executor());
        cx.update(|cx| {
            let settings_store = settings::SettingsStore::test(cx);
            cx.set_global(settings_store);
            theme_settings::init(theme::LoadThemes::JustBase, cx);
            release_channel::init("0.0.0".parse().unwrap(), cx);
            <dyn fs::Fs>::set_global(fs, cx);
            ThreadMetadataStore::init_global(cx);
        });
        cx.run_until_parked();
    }

    /// 返回两个非当前、非开发版的发布频道
    fn foreign_channels(cx: &TestAppContext) -> (ReleaseChannel, ReleaseChannel) {
        let current = cx.update(|cx| ReleaseChannel::global(cx));
        let mut channels = ReleaseChannel::ALL
            .iter()
            .copied()
            .filter(|ch| *ch != current && *ch != ReleaseChannel::Dev);
        (channels.next().unwrap(), channels.next().unwrap())
    }

    /// 测试：从其他频道导入线程
    #[gpui::test]
    async fn test_import_threads_from_other_channels(cx: &mut TestAppContext) {
        init_test(cx);

        let dir = tempfile::tempdir().unwrap();
        let database_dir = dir.path().to_path_buf();

        let (channel_a, channel_b) = foreign_channels(cx);

        // 为两个外部频道配置数据库
        let db_a = create_channel_db(dir.path(), channel_a);
        insert_thread(&db_a, "线程A1", "2025-01-15T10:00:00Z", false);
        insert_thread(&db_a, "线程A2", "2025-01-15T11:00:00Z", true);
        drop(db_a);

        let db_b = create_channel_db(dir.path(), channel_b);
        insert_thread(&db_b, "线程B1", "2025-01-15T12:00:00Z", false);
        drop(db_b);

        // 创建工作空间并执行导入
        let fs = fs::FakeFs::new(cx.executor());
        let project = project::Project::test(fs, [], cx).await;
        let multi_workspace =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project, window, cx));
        let workspace_entity = multi_workspace
            .read_with(cx, |mw, _cx| mw.workspace().clone())
            .unwrap();
        let mut vcx = gpui::VisualTestContext::from_window(multi_workspace.into(), cx);

        workspace_entity.update_in(&mut vcx, |_workspace, _window, cx| {
            import_threads_from_other_channels_in(database_dir, cx);
        });
        cx.run_until_parked();

        // 验证所有三个线程已导入存储
        cx.update(|cx| {
            let store = ThreadMetadataStore::global(cx);
            let store = store.read(cx);
            let titles: collections::HashSet<String> = store
                .entries()
                .map(|m| m.display_title().to_string())
                .collect();

            assert_eq!(titles.len(), 3);
            assert!(titles.contains("线程A1"));
            assert!(titles.contains("线程A2"));
            assert!(titles.contains("线程B1"));

            // 验证归档状态保留
            let thread_a2 = store
                .entries()
                .find(|m| m.display_title().as_ref() == "线程A2")
                .unwrap();
            assert!(thread_a2.archived);

            let thread_b1 = store
                .entries()
                .find(|m| m.display_title().as_ref() == "线程B1")
                .unwrap();
            assert!(!thread_b1.archived);
        });
    }

    /// 测试：导入时跳过已存在的线程
    #[gpui::test]
    async fn test_import_skips_already_existing_threads(cx: &mut TestAppContext) {
        init_test(cx);

        let dir = tempfile::tempdir().unwrap();
        let database_dir = dir.path().to_path_buf();

        let (channel_a, _) = foreign_channels(cx);

        // 为外部频道配置数据库
        let db_a = create_channel_db(dir.path(), channel_a);
        insert_thread(&db_a, "线程A", "2025-01-15T10:00:00Z", false);
        insert_thread(&db_a, "线程B", "2025-01-15T11:00:00Z", false);
        drop(db_a);

        // 读取线程并预填充一个到存储
        let foreign_threads = read_threads_from_channel(dir.path(), channel_a).unwrap();
        let thread_a = foreign_threads
            .iter()
            .find(|t| t.display_title().as_ref() == "线程A")
            .unwrap()
            .clone();

        // 预填充线程A
        cx.update(|cx| {
            ThreadMetadataStore::global(cx).update(cx, |store, cx| store.save(thread_a, cx));
        });
        cx.run_until_parked();

        // 执行导入
        let fs = fs::FakeFs::new(cx.executor());
        let project = project::Project::test(fs, [], cx).await;
        let multi_workspace =
            cx.add_window(|window, cx| MultiWorkspace::test_new(project, window, cx));
        let workspace_entity = multi_workspace
            .read_with(cx, |mw, _cx| mw.workspace().clone())
            .unwrap();
        let mut vcx = gpui::VisualTestContext::from_window(multi_workspace.into(), cx);

        workspace_entity.update_in(&mut vcx, |_workspace, _window, cx| {
            import_threads_from_other_channels_in(database_dir, cx);
        });
        cx.run_until_parked();

        // 验证仅导入了线程B（线程A已存在）
        cx.update(|cx| {
            let store = ThreadMetadataStore::global(cx);
            let store = store.read(cx);
            assert_eq!(store.entries().count(), 2);

            let titles: collections::HashSet<String> = store
                .entries()
                .map(|m| m.display_title().to_string())
                .collect();
            assert!(titles.contains("线程A"));
            assert!(titles.contains("线程B"));
        });
    }
}