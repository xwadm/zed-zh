use crate::notification_window_options;
use call::{ActiveCall, IncomingCall};
use futures::StreamExt;
use gpui::{App, WindowHandle, prelude::*};

use std::sync::{Arc, Weak};
use ui::{CollabNotification, prelude::*};
use util::ResultExt;
use workspace::AppState;

/// 初始化来电通知监听
/// 监听全局来电事件，并在所有屏幕上显示通知窗口
pub fn init(app_state: &Arc<AppState>, cx: &mut App) {
    let app_state = Arc::downgrade(app_state);
    let mut incoming_call = ActiveCall::global(cx).read(cx).incoming();
    
    cx.spawn(async move |cx| {
        let mut notification_windows: Vec<WindowHandle<IncomingCallNotification>> = Vec::new();
        
        // 持续监听来电流
        while let Some(incoming_call) = incoming_call.next().await {
            // 关闭所有已存在的通知窗口
            for window in notification_windows.drain(..) {
                window.update(cx, |_, window, _| window.remove_window()).log_err();
            }

            // 收到新来电时，在所有屏幕上显示通知
            if let Some(incoming_call) = incoming_call {
                let unique_screens = cx.update(|cx| cx.displays());
                let window_size = gpui::Size {
                    width: px(400.),
                    height: px(72.),
                };

                // 为每个屏幕创建通知窗口
                for screen in unique_screens {
                    let options = cx.update(|cx| notification_window_options(screen, window_size, cx));
                    if let Ok(window) = cx.open_window(options, |_, cx| {
                        cx.new(|_| IncomingCallNotification::new(incoming_call.clone(), app_state.clone()))
                    }) {
                        notification_windows.push(window);
                    }
                }
            }
        }

        // 退出时清理窗口
        for window in notification_windows.drain(..) {
            window.update(cx, |_, window, _| window.remove_window()).log_err();
        }
    }).detach();
}

/// 来电通知内部状态
struct IncomingCallNotificationState {
    call: IncomingCall,
    app_state: Weak<AppState>,
}

/// 来电通知窗口组件
pub struct IncomingCallNotification {
    state: Arc<IncomingCallNotificationState>,
}

impl IncomingCallNotificationState {
    pub fn new(call: IncomingCall, app_state: Weak<AppState>) -> Self {
        Self { call, app_state }
    }

    /// 响应用户操作：接受 / 拒绝来电
    fn respond(&self, accept: bool, cx: &mut App) {
        let active_call = ActiveCall::global(cx);
        
        if accept {
            // 接受来电：加入通话
            let join = active_call.update(cx, |active_call, cx| active_call.accept_incoming(cx));
            let caller_user_id = self.call.calling_user.id;
            let initial_project_id = self.call.initial_project.as_ref().map(|project| project.id);
            let app_state = self.app_state.clone();
            
            cx.spawn(async move |cx| {
                join.await?;
                // 如果有共享项目，自动加入项目
                if let Some(project_id) = initial_project_id {
                    cx.update(|cx| {
                        if let Some(app_state) = app_state.upgrade() {
                            workspace::join_in_room_project(
                                project_id,
                                caller_user_id,
                                app_state,
                                cx,
                            ).detach_and_log_err(cx);
                        }
                    });
                }
                anyhow::Ok(())
            }).detach_and_log_err(cx);
        } else {
            // 拒绝来电
            active_call.update(cx, |active_call, cx| {
                active_call.decline_incoming(cx).log_err();
            });
        }
    }
}

impl IncomingCallNotification {
    pub fn new(call: IncomingCall, app_state: Weak<AppState>) -> Self {
        Self {
            state: Arc::new(IncomingCallNotificationState::new(call, app_state)),
        }
    }
}

impl Render for IncomingCallNotification {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui_font = theme_settings::setup_ui_font(window, cx);

        // 渲染协作通知 UI
        div().size_full().font(ui_font).child(
            CollabNotification::new(
                self.state.call.calling_user.avatar_uri.clone(),
                // 接受按钮
                Button::new("accept", "接受").on_click({
                    let state = self.state.clone();
                    move |_, _, cx| state.respond(true, cx)
                }),
                // 拒绝按钮
                Button::new("decline", "拒绝").on_click({
                    let state = self.state.clone();
                    move |_, _, cx| state.respond(false, cx)
                }),
            )
            .child(Label::new(format!(
                "{} 正在 Zed 中共享项目",
                self.state.call.calling_user.github_login
            ))),
        )
    }
}