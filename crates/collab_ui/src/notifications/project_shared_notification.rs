use crate::notification_window_options;
use call::{ActiveCall, room};
use client::User;
use collections::HashMap;
use gpui::{App, Size};
use std::sync::{Arc, Weak};

use ui::{CollabNotification, prelude::*};
use util::ResultExt;
use workspace::AppState;

/// 初始化项目共享通知监听器
/// 监听房间内的项目共享事件，并弹出通知窗口
pub fn init(app_state: &Arc<AppState>, cx: &mut App) {
    let app_state = Arc::downgrade(app_state);
    let active_call = ActiveCall::global(cx);
    let mut notification_windows = HashMap::default();
    
    cx.subscribe(&active_call, move |_, event, cx| match event {
        // 收到远程项目共享邀请
        room::Event::RemoteProjectShared {
            owner,
            project_id,
            worktree_root_names,
        } => {
            let window_size = Size {
                width: px(400.),
                height: px(72.),
            };

            // 在所有屏幕上显示通知
            for screen in cx.displays() {
                let options = notification_window_options(screen, window_size, cx);
                let Some(window) = cx
                    .open_window(options, |_, cx| {
                        cx.new(|_| {
                            ProjectSharedNotification::new(
                                owner.clone(),
                                *project_id,
                                worktree_root_names.clone(),
                                app_state.clone(),
                            )
                        })
                    })
                    .log_err()
                else {
                    continue;
                };
                notification_windows
                    .entry(*project_id)
                    .or_insert(Vec::new())
                    .push(window);
            }
        }

        // 项目共享取消/已加入/邀请已丢弃：关闭对应通知
        room::Event::RemoteProjectUnshared { project_id }
        | room::Event::RemoteProjectJoined { project_id }
        | room::Event::RemoteProjectInvitationDiscarded { project_id } => {
            if let Some(windows) = notification_windows.remove(project_id) {
                for window in windows {
                    window
                        .update(cx, |_, window, _| {
                            window.remove_window();
                        })
                        .ok();
                }
            }
        }

        // 离开房间：关闭所有通知
        room::Event::RoomLeft { .. } => {
            for (_, windows) in notification_windows.drain() {
                for window in windows {
                    window
                        .update(cx, |_, window, _| {
                            window.remove_window();
                        })
                        .ok();
                }
            }
        }
        _ => {}
    })
    .detach();
}

/// 项目共享通知组件
pub struct ProjectSharedNotification {
    project_id: u64,
    worktree_root_names: Vec<String>,
    owner: Arc<User>,
    app_state: Weak<AppState>,
}

impl ProjectSharedNotification {
    /// 创建项目共享通知
    fn new(
        owner: Arc<User>,
        project_id: u64,
        worktree_root_names: Vec<String>,
        app_state: Weak<AppState>,
    ) -> Self {
        Self {
            project_id,
            worktree_root_names,
            owner,
            app_state,
        }
    }

    /// 加入共享的项目
    fn join(&mut self, cx: &mut Context<Self>) {
        if let Some(app_state) = self.app_state.upgrade() {
            workspace::join_in_room_project(self.project_id, self.owner.id, app_state, cx)
                .detach_and_log_err(cx);
        }
    }

    /// 关闭通知，丢弃项目邀请
    fn dismiss(&mut self, cx: &mut Context<Self>) {
        if let Some(active_room) = ActiveCall::global(cx).read(cx).room().cloned() {
            active_room.update(cx, |_, cx| {
                cx.emit(room::Event::RemoteProjectInvitationDiscarded {
                    project_id: self.project_id,
                });
            });
        }
    }
}

impl Render for ProjectSharedNotification {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui_font = theme_settings::setup_ui_font(window, cx);
        let no_worktree_root_names = self.worktree_root_names.is_empty();

        let punctuation = if no_worktree_root_names { "" } else { ":" };
        let main_label = format!(
            "{} 正在与你共享项目{}",
            self.owner.github_login.clone(),
            punctuation
        );

        // 渲染协作通知 UI
        div().size_full().font(ui_font).child(
            CollabNotification::new(
                self.owner.avatar_uri.clone(),
                // 打开/加入按钮
                Button::new("open", "打开").on_click(cx.listener(move |this, _event, _, cx| {
                    this.join(cx);
                })),
                // 关闭按钮
                Button::new("dismiss", "关闭").on_click(cx.listener(
                    move |this, _event, _, cx| {
                        this.dismiss(cx);
                    },
                )),
            )
            .child(Label::new(main_label))
            // 显示项目根目录名称（灰色小字）
            .when(!no_worktree_root_names, |this| {
                this.child(Label::new(self.worktree_root_names.join(", ")).color(Color::Muted))
            }),
        )
    }
}