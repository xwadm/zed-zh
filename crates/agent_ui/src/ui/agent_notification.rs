use gpui::{
    App, Context, EventEmitter, IntoElement, PlatformDisplay, Size, Window,
    WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowKind, WindowOptions,
    linear_color_stop, linear_gradient, point,
};
use release_channel::ReleaseChannel;
use std::rc::Rc;
use ui::{Render, prelude::*};

/// AI 助手通知组件，用于展示来自 Agent 的通知提醒
pub struct AgentNotification {
    title: SharedString,
    caption: SharedString,
    icon: IconName,
    project_name: Option<SharedString>,
}

impl AgentNotification {
    /// 创建新的 AI 助手通知
    /// - title: 通知标题
    /// - caption: 通知描述
    /// - icon: 通知图标
    /// - project_name: 可选的项目名称
    pub fn new(
        title: impl Into<SharedString>,
        caption: impl Into<SharedString>,
        icon: IconName,
        project_name: Option<impl Into<SharedString>>,
    ) -> Self {
        Self {
            title: title.into(),
            caption: caption.into(),
            icon,
            project_name: project_name.map(|name| name.into()),
        }
    }

    /// 生成通知窗口的配置选项（显示在屏幕右上角）
    pub fn window_options(screen: Rc<dyn PlatformDisplay>, cx: &App) -> WindowOptions {
        let size = Size {
            width: px(450.),
            height: px(72.),
        };

        // 通知窗口边距
        let notification_margin_width = px(16.);
        let notification_margin_height = px(-48.);

        // 计算窗口位置：屏幕右上角
        let bounds = gpui::Bounds::<Pixels> {
            origin: screen.bounds().top_right()
                - point(
                    size.width + notification_margin_width,
                    notification_margin_height,
                ),
            size,
        };

        let app_id = ReleaseChannel::global(cx).app_id();

        // 窗口配置：无焦点、弹出式、透明背景、固定位置
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            focus: false,
            show: true,
            kind: WindowKind::PopUp,
            is_movable: false,
            display_id: Some(screen.id()),
            window_background: WindowBackgroundAppearance::Transparent,
            app_id: Some(app_id.to_owned()),
            window_min_size: None,
            window_decorations: Some(WindowDecorations::Client),
            tabbing_identifier: None,
            ..Default::default()
        }
    }
}

/// 通知事件：用户接受 / 关闭通知
pub enum AgentNotificationEvent {
    Accepted,
    Dismissed,
}

impl EventEmitter<AgentNotificationEvent> for AgentNotification {}

impl AgentNotification {
    /// 处理接受通知事件
    pub fn accept(&mut self, cx: &mut Context<Self>) {
        cx.emit(AgentNotificationEvent::Accepted);
    }

    /// 处理关闭通知事件
    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        cx.emit(AgentNotificationEvent::Dismissed);
    }
}

impl Render for AgentNotification {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui_font = theme_settings::setup_ui_font(window, cx);
        let line_height = window.line_height();

        let bg = cx.theme().colors().elevated_surface_background;
        
        // 右侧渐变遮罩，防止文字溢出突兀
        let gradient_overflow = || {
            div()
                .h_full()
                .absolute()
                .w_8()
                .bottom_0()
                .right_0()
                .bg(linear_gradient(
                    90.,
                    linear_color_stop(bg, 1.),
                    linear_color_stop(bg.opacity(0.2), 0.),
                ))
        };

        // 主容器：卡片样式、圆角、阴影
        h_flex()
            .id("agent-notification")
            .size_full()
            .p_3()
            .gap_4()
            .justify_between()
            .elevation_3(cx)
            .text_ui(cx)
            .font(ui_font)
            .border_color(cx.theme().colors().border)
            .rounded_xl()
            // 左侧：图标 + 标题 + 描述 + 项目名
            .child(
                h_flex()
                    .items_start()
                    .gap_2()
                    .flex_1()
                    // 通知图标
                    .child(
                        h_flex().h(line_height).justify_center().child(
                            Icon::new(self.icon)
                                .color(Color::Muted)
                                .size(IconSize::Small),
                        ),
                    )
                    // 文本内容区域
                    .child(
                        v_flex()
                            .flex_1()
                            .max_w(px(300.))
                            // 标题（截断 + 渐变遮罩）
                            .child(
                                div()
                                    .relative()
                                    .text_size(px(14.))
                                    .text_color(cx.theme().colors().text)
                                    .truncate()
                                    .child(self.title.clone())
                                    .child(gradient_overflow()),
                            )
                            // 描述行：项目名（可选）+ 描述文本
                            .child(
                                h_flex()
                                    .relative()
                                    .gap_1p5()
                                    .text_size(px(12.))
                                    .text_color(cx.theme().colors().text_muted)
                                    .truncate()
                                    .when_some(
                                        self.project_name.clone(),
                                        |description, project_name| {
                                            description.child(
                                                h_flex()
                                                    .gap_1p5()
                                                    .child(
                                                        div()
                                                            .max_w_16()
                                                            .truncate()
                                                            .child(project_name),
                                                    )
                                                    .child(
                                                        div().size(px(3.)).rounded_full().bg(cx
                                                            .theme()
                                                            .colors()
                                                            .text
                                                            .opacity(0.5)),
                                                    ),
                                            )
                                        },
                                    )
                                    .child(self.caption.clone())
                                    .child(gradient_overflow()),
                            ),
                    ),
            )
            // 右侧：操作按钮（查看 + 关闭）
            .child(
                v_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        Button::new("open", "查看")
                            .style(ButtonStyle::Tinted(ui::TintColor::Accent))
                            .full_width()
                            .on_click({
                                cx.listener(move |this, _event, _, cx| {
                                    this.accept(cx);
                                })
                            }),
                    )
                    .child(Button::new("dismiss", "关闭").full_width().on_click({
                        cx.listener(move |this, _event, _, cx| {
                            this.dismiss(cx);
                        })
                    })),
            )
    }
}