use gpui::AnyElement;

use crate::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorderPosition {
    Top,
    Bottom,
}

/// 醒目提示组件，用于展示需要用户重点关注的关键信息
/// 适用于需要用户感知并作出操作决策的业务场景
///
/// # 使用示例
///
/// ```
/// use ui::prelude::*;
/// use ui::{Button, Callout, IconName, Label, Severity};
///
/// let callout = Callout::new()
///     .severity(Severity::Warning)
///     .icon(IconName::Warning)
///     .title("订阅服务即将到期！")
///     .description("当前订阅套餐即将过期，请及时完成续费。")
///     .actions_slot(Button::new("renew", "立即续费"));
/// ```
///
#[derive(IntoElement, RegisterComponent)]
pub struct Callout {
    severity: Severity,
    icon: Option<IconName>,
    title: Option<SharedString>,
    description: Option<SharedString>,
    description_slot: Option<AnyElement>,
    actions_slot: Option<AnyElement>,
    dismiss_action: Option<AnyElement>,
    line_height: Option<Pixels>,
    border_position: BorderPosition,
}

impl Callout {
    /// 初始化默认样式的提示组件
    pub fn new() -> Self {
        Self {
            severity: Severity::Info,
            icon: None,
            title: None,
            description: None,
            description_slot: None,
            actions_slot: None,
            dismiss_action: None,
            line_height: None,
            border_position: BorderPosition::Top,
        }
    }

    /// 设置组件风险等级
    pub fn severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
        self
    }

    /// 设置组件内置图标
    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    /// 设置提示标题
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// 设置文本描述内容
    /// 支持单行/多行文本展示
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// 自定义描述区域插槽
    /// 支持嵌入任意自定义元素（如富文本、markdown 组件）
    /// 该配置优先级高于普通文本描述
    pub fn description_slot(mut self, description: impl IntoElement) -> Self {
        self.description_slot = Some(description.into_any_element());
        self
    }

    /// 设置主操作按钮插槽
    pub fn actions_slot(mut self, action: impl IntoElement) -> Self {
        self.actions_slot = Some(action.into_any_element());
        self
    }

    /// 设置关闭操作按钮
    /// 固定居右展示，通常为图标式关闭按钮
    pub fn dismiss_action(mut self, action: impl IntoElement) -> Self {
        self.dismiss_action = Some(action.into_any_element());
        self
    }

    /// 自定义内容行高
    pub fn line_height(mut self, line_height: Pixels) -> Self {
        self.line_height = Some(line_height);
        self
    }

    /// 设置侧边高亮边框位置
    pub fn border_position(mut self, border_position: BorderPosition) -> Self {
        self.border_position = border_position;
        self
    }
}

impl RenderOnce for Callout {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let line_height = self.line_height.unwrap_or(window.line_height());

        let has_actions = self.actions_slot.is_some() || self.dismiss_action.is_some();

        let (icon, icon_color, bg_color) = match self.severity {
            Severity::Info => (
                IconName::Info,
                Color::Muted,
                cx.theme().status().info_background.opacity(0.1),
            ),
            Severity::Success => (
                IconName::Check,
                Color::Success,
                cx.theme().status().success.opacity(0.1),
            ),
            Severity::Warning => (
                IconName::Warning,
                Color::Warning,
                cx.theme().status().warning_background.opacity(0.2),
            ),
            Severity::Error => (
                IconName::XCircle,
                Color::Error,
                cx.theme().status().error.opacity(0.08),
            ),
        };

        h_flex()
            .min_w_0()
            .w_full()
            .p_2()
            .gap_2()
            .items_start()
            .map(|this| match self.border_position {
                BorderPosition::Top => this.border_t_1(),
                BorderPosition::Bottom => this.border_b_1(),
            })
            .border_color(cx.theme().colors().border)
            .bg(bg_color)
            .overflow_x_hidden()
            .when(self.icon.is_some(), |this| {
                this.child(
                    h_flex()
                        .h(line_height)
                        .justify_center()
                        .child(Icon::new(icon).size(IconSize::Small).color(icon_color)),
                )
            })
            .child(
                v_flex()
                    .min_w_0()
                    .min_h_0()
                    .w_full()
                    .child(
                        h_flex()
                            .min_h(line_height)
                            .w_full()
                            .gap_1()
                            .justify_between()
                            .flex_wrap()
                            .when_some(self.title, |this, title| {
                                this.child(
                                    div()
                                        .min_w_0()
                                        .flex_1()
                                        .child(Label::new(title).size(LabelSize::Small)),
                                )
                            })
                            .when(has_actions, |this| {
                                this.child(
                                    h_flex()
                                        .gap_0p5()
                                        .when_some(self.actions_slot, |this, action| {
                                            this.child(action)
                                        })
                                        .when_some(self.dismiss_action, |this, action| {
                                            this.child(action)
                                        }),
                                )
                            }),
                    )
                    .map(|this| {
                        let base_desc_container = div()
                            .id("callout-description-slot")
                            .w_full()
                            .max_h_32()
                            .flex_1()
                            .overflow_y_scroll()
                            .text_ui_sm(cx);

                        if let Some(description_slot) = self.description_slot {
                            this.child(base_desc_container.child(description_slot))
                        } else if let Some(description) = self.description {
                            this.child(
                                base_desc_container
                                    .text_color(cx.theme().colors().text_muted)
                                    .child(description),
                            )
                        } else {
                            this
                        }
                    }),
            )
    }
}

impl Component for Callout {
    fn scope() -> ComponentScope {
        ComponentScope::DataDisplay
    }

    fn description() -> Option<&'static str> {
        Some(
           "用于展示系统提示类信息，引导用户感知状态并完成操作决策。常用于 AI 对话令牌不足、套餐额度耗尽、版本升级提醒等场景",
        )
    }

    fn preview(_window: &mut Window, _cx: &mut App) -> Option<AnyElement> {
        let single_action = || Button::new("got-it", "确认").label_size(LabelSize::Small);
        let multiple_actions = || {
            h_flex()
                .gap_0p5()
                .child(Button::new("update", "备份并更新").label_size(LabelSize::Small))
                .child(Button::new("dismiss", "关闭提示").label_size(LabelSize::Small))
        };

        let basic_examples = vec![
            single_example(
                "纯标题提示",
                Callout::new()
                    .icon(IconName::Info)
                    .title("系统将于今夜执行维护任务")
                    .actions_slot(single_action())
                    .into_any_element(),
            )
            .width(px(580.)),
            single_example(
                "标题+描述文本",
                Callout::new()
                    .icon(IconName::Warning)
                    .title("当前配置包含废弃参数")
                    .description("系统将自动备份现有配置，并升级至新版格式。")
                    .actions_slot(single_action())
                    .into_any_element(),
            )
            .width(px(580.)),
            single_example(
                "错误提示+多按钮",
                Callout::new()
                    .icon(IconName::Close)
                    .title("对话上下文已达令牌上限")
                    .description("基于会话摘要新建对话，即可继续使用。")
                    .actions_slot(multiple_actions())
                    .into_any_element(),
            )
            .width(px(580.)),
            single_example(
                "多行文本描述",
                Callout::new()
                    .icon(IconName::Sparkle)
                    .title("升级专业版权益")
                    .description("• 无限对话会话\n• 专属技术支持\n• 高级数据统计")
                    .actions_slot(multiple_actions())
                    .into_any_element(),
            )
            .width(px(580.)),
            single_example(
                "超长可滚动文案",
                Callout::new()
                    .severity(Severity::Error)
                    .icon(IconName::XCircle)
                    .title("API 调用异常详情")
                    .description_slot(
                        v_flex().gap_1().children(
                            [
                                "当前已超出接口调用配额限制。",
                                "查阅官方文档获取扩容方案。",
                                "错误明细：",
                                "• 触发限制：指标配额耗尽",
                                "• 剩余额度：0",
                                "• 模型类型：gemini-3.1-pro",
                                "• 建议重试：26.33 秒后",
                                "附加调试信息：",
                                "- 请求标识：abc123def456",
                                "- 异常时间：2024-01-15T10:30:00Z",
                                "- 服务区域：us-central1",
                                "- 接口服务：generativelanguage.googleapis.com",
                                "- 错误码：RESOURCE_EXHAUSTED",
                                "- 冷却时长：26 秒",
                                "该异常由接口配额耗尽触发，需等待冷却或升级套餐。",
                            ]
                            .into_iter()
                            .map(|t| Label::new(t).size(LabelSize::Small).color(Color::Muted)),
                        ),
                    )
                    .actions_slot(single_action())
                    .into_any_element(),
            )
            .width(px(580.)),
        ];

        let severity_examples = vec![
            single_example(
                "普通信息",
                Callout::new()
                    .icon(IconName::Info)
                    .title("系统将于今夜执行维护任务")
                    .actions_slot(single_action())
                    .into_any_element(),
            ),
            single_example(
                "风险警告",
                Callout::new()
                    .severity(Severity::Warning)
                    .icon(IconName::Triangle)
                    .title("系统将于今夜执行维护任务")
                    .actions_slot(single_action())
                    .into_any_element(),
            ),
            single_example(
                "错误异常",
                Callout::new()
                    .severity(Severity::Error)
                    .icon(IconName::XCircle)
                    .title("系统将于今夜执行维护任务")
                    .actions_slot(single_action())
                    .into_any_element(),
            ),
            single_example(
                "操作成功",
                Callout::new()
                    .severity(Severity::Success)
                    .icon(IconName::Check)
                    .title("系统将于今夜执行维护任务")
                    .actions_slot(single_action())
                    .into_any_element(),
            ),
        ];

        Some(
            v_flex()
                .gap_4()
                .child(example_group(basic_examples).vertical())
                .child(example_group_with_title("风险等级展示", severity_examples).vertical())
                .into_any_element(),
        )
    }
}