use crate::prelude::*;
use gpui::{AnyElement, IntoElement, ParentElement, Styled};

/// 提示横幅提供简洁的信息通知，不会打断用户操作
/// 该组件提供四种严重等级，可根据消息内容灵活使用
///
/// # 使用示例
///
/// ```
/// use ui::prelude::*;
/// use ui::{Banner, Button, Icon, IconName, IconSize, Label, Severity};
///
/// Banner::new()
///     .severity(Severity::Success)
///     .children([Label::new("这是一条成功消息")])
///     .action_slot(
///         Button::new("learn-more", "了解更多")
///             .end_icon(Icon::new(IconName::ArrowUpRight).size(IconSize::Small)),
///     );
/// ```
#[derive(IntoElement, RegisterComponent)]
pub struct Banner {
    severity: Severity,
    children: Vec<AnyElement>,
    action_slot: Option<AnyElement>,
    wrap_content: bool,
}

impl Banner {
    /// 创建一个新的默认样式提示横幅组件
    pub fn new() -> Self {
        Self {
            severity: Severity::Info,
            children: Vec::new(),
            action_slot: None,
            wrap_content: false,
        }
    }

    /// 设置提示横幅的严重等级
    pub fn severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
        self
    }

    /// 操作按钮插槽，可放置行动按钮或关闭按钮
    pub fn action_slot(mut self, element: impl IntoElement) -> Self {
        self.action_slot = Some(element.into_any_element());
        self
    }

    /// 设置横幅内容是否自动换行
    pub fn wrap_content(mut self, wrap: bool) -> Self {
        self.wrap_content = wrap;
        self
    }
}

impl ParentElement for Banner {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements)
    }
}

impl RenderOnce for Banner {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let banner = h_flex()
            .min_w_0()
            .py_0p5()
            .gap_1p5()
            .when(self.wrap_content, |this| this.flex_wrap())
            .justify_between()
            .rounded_sm()
            .border_1();

        let (icon, icon_color, bg_color, border_color) = match self.severity {
            Severity::Info => (
                IconName::Info,
                Color::Muted,
                cx.theme().status().info_background.opacity(0.5),
                cx.theme().colors().border.opacity(0.5),
            ),
            Severity::Success => (
                IconName::Check,
                Color::Success,
                cx.theme().status().success.opacity(0.1),
                cx.theme().status().success.opacity(0.2),
            ),
            Severity::Warning => (
                IconName::Warning,
                Color::Warning,
                cx.theme().status().warning_background.opacity(0.5),
                cx.theme().status().warning_border.opacity(0.4),
            ),
            Severity::Error => (
                IconName::XCircle,
                Color::Error,
                cx.theme().status().error.opacity(0.1),
                cx.theme().status().error.opacity(0.2),
            ),
        };

        let mut banner = banner.bg(bg_color).border_color(border_color);

        let icon_and_child = h_flex()
            .items_start()
            .min_w_0()
            .flex_1()
            .gap_1p5()
            .child(
                h_flex()
                    .h(window.line_height())
                    .flex_shrink_0()
                    .child(Icon::new(icon).size(IconSize::XSmall).color(icon_color)),
            )
            .child(div().min_w_0().flex_1().children(self.children));

        if let Some(action_slot) = self.action_slot {
            banner = banner
                .pl_2()
                .pr_1()
                .child(icon_and_child)
                .child(action_slot);
        } else {
            banner = banner.px_2().child(icon_and_child);
        }

        banner
    }
}

impl Component for Banner {
    fn scope() -> ComponentScope {
        ComponentScope::DataDisplay
    }

    fn preview(_window: &mut Window, _cx: &mut App) -> Option<AnyElement> {
        let severity_examples = vec![
            single_example(
                "默认样式",
                Banner::new()
                    .child(Label::new("这是一个无自定义配置的默认横幅"))
                    .into_any_element(),
            ),
            single_example(
                "信息提示",
                Banner::new()
                    .severity(Severity::Info)
                    .child(Label::new("这是一条通知消息"))
                    .action_slot(
                        Button::new("learn-more", "了解更多")
                            .end_icon(Icon::new(IconName::ArrowUpRight).size(IconSize::Small)),
                    )
                    .into_any_element(),
            ),
            single_example(
                "操作成功",
                Banner::new()
                    .severity(Severity::Success)
                    .child(Label::new("操作已成功完成"))
                    .action_slot(Button::new("dismiss", "关闭"))
                    .into_any_element(),
            ),
            single_example(
                "警告提醒",
                Banner::new()
                    .severity(Severity::Warning)
                    .child(Label::new("你的配置文件使用了已弃用的设置项"))
                    .action_slot(Button::new("update", "更新设置"))
                    .into_any_element(),
            ),
            single_example(
                "错误提示",
                Banner::new()
                    .severity(Severity::Error)
                    .child(Label::new("连接错误：无法连接到服务器"))
                    .action_slot(Button::new("reconnect", "重试"))
                    .into_any_element(),
            ),
        ];

        Some(
            example_group(severity_examples)
                .vertical()
                .into_any_element(),
        )
    }
}