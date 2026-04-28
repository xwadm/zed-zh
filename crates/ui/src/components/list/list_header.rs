use std::sync::Arc;

use crate::{Disclosure, prelude::*};
use component::{Component, ComponentScope, example_group_with_title, single_example};
use gpui::{AnyElement, ClickEvent};
use theme::UiDensity;

#[derive(IntoElement, RegisterComponent)]
pub struct ListHeader {
    /// 头部标题文本
    label: SharedString,
    /// 标题左侧插槽：用于放置图标、头像等内容
    start_slot: Option<AnyElement>,
    /// 标题右侧插槽：通常位于头部最右侧，可放置按钮、展开箭头、头像组等
    end_slot: Option<AnyElement>,
    /// 鼠标悬浮时显示的右侧插槽，显示时会覆盖 end_slot
    end_hover_slot: Option<AnyElement>,
    /// 展开/收起状态
    toggle: Option<bool>,
    /// 展开/收起状态变更回调
    on_toggle: Option<Arc<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
    /// 是否启用内边距缩进
    inset: bool,
    /// 是否选中状态
    selected: bool,
}

impl ListHeader {
    /// 创建列表头部组件
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            start_slot: None,
            end_slot: None,
            end_hover_slot: None,
            inset: false,
            toggle: None,
            on_toggle: None,
            selected: false,
        }
    }

    /// 设置展开/收起状态
    pub fn toggle(mut self, toggle: impl Into<Option<bool>>) -> Self {
        self.toggle = toggle.into();
        self
    }

    /// 设置展开/收起状态的点击回调
    pub fn on_toggle(
        mut self,
        on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_toggle = Some(Arc::new(on_toggle));
        self
    }

    /// 设置左侧插槽内容
    pub fn start_slot<E: IntoElement>(mut self, start_slot: impl Into<Option<E>>) -> Self {
        self.start_slot = start_slot.into().map(IntoElement::into_any_element);
        self
    }

    /// 设置右侧插槽内容
    pub fn end_slot<E: IntoElement>(mut self, end_slot: impl Into<Option<E>>) -> Self {
        self.end_slot = end_slot.into().map(IntoElement::into_any_element);
        self
    }

    /// 设置鼠标悬浮时显示的右侧插槽
    pub fn end_hover_slot<E: IntoElement>(mut self, end_hover_slot: impl Into<Option<E>>) -> Self {
        self.end_hover_slot = end_hover_slot.into().map(IntoElement::into_any_element);
        self
    }

    /// 设置是否启用内边距缩进
    pub fn inset(mut self, inset: bool) -> Self {
        self.inset = inset;
        self
    }
}

/// 实现可选中状态接口
impl Toggleable for ListHeader {
    fn toggle_state(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
}

impl RenderOnce for ListHeader {
    /// 渲染列表头部
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let ui_density = theme::theme_settings(cx).ui_density(cx);

        h_flex()
            .id(self.label.clone())
            .w_full()
            .relative()
            .group("list_header")
            .child(
                div()
                    .map(|this| match ui_density {
                        UiDensity::Comfortable => this.h_5(),
                        _ => this.h_7(),
                    })
                    .when(self.inset, |this| this.px_2())
                    .when(self.selected, |this| {
                        this.bg(cx.theme().colors().ghost_element_selected)
                    })
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_between()
                    .w_full()
                    .gap(DynamicSpacing::Base04.rems(cx))
                    .child(
                        h_flex()
                            .gap(DynamicSpacing::Base04.rems(cx))
                            .children(self.toggle.map(|is_open| {
                                Disclosure::new("toggle", is_open)
                                    .on_toggle_expanded(self.on_toggle.clone())
                            }))
                            .child(
                                div()
                                    .id("label_container")
                                    .flex()
                                    .gap(DynamicSpacing::Base04.rems(cx))
                                    .items_center()
                                    .children(self.start_slot)
                                    .child(Label::new(self.label.clone()).color(Color::Muted))
                                    .when_some(self.on_toggle, |this, on_toggle| {
                                        this.on_click(move |event, window, cx| {
                                            on_toggle(event, window, cx)
                                        })
                                    }),
                            ),
                    )
                    .child(h_flex().children(self.end_slot))
                    .when_some(self.end_hover_slot, |this, end_hover_slot| {
                        this.child(
                            div()
                                .absolute()
                                .right_0()
                                .visible_on_hover("list_header")
                                .child(end_hover_slot),
                        )
                    }),
            )
    }
}

impl Component for ListHeader {
    /// 组件分类：数据展示
    fn scope() -> ComponentScope {
        ComponentScope::DataDisplay
    }

    /// 组件描述
    fn description() -> Option<&'static str> {
        Some("列表头部组件，支持图标、操作按钮、可折叠区域等功能。")
    }

    /// 组件预览示例
    fn preview(_window: &mut Window, _cx: &mut App) -> Option<AnyElement> {
        Some(
            v_flex()
                .gap_6()
                .children(vec![
                    example_group_with_title(
                        "基础头部",
                        vec![
                            single_example(
                                "基础样式",
                                ListHeader::new("分区标题").into_any_element(),
                            ),
                            single_example(
                                "带图标",
                                ListHeader::new("文件")
                                    .start_slot(Icon::new(IconName::File))
                                    .into_any_element(),
                            ),
                            single_example(
                                "带右侧内容",
                                ListHeader::new("最近使用")
                                    .end_slot(Label::new("5").color(Color::Muted))
                                    .into_any_element(),
                            ),
                        ],
                    ),
                    example_group_with_title(
                        "可折叠头部",
                        vec![
                            single_example(
                                "展开状态",
                                ListHeader::new("展开分区")
                                    .toggle(Some(true))
                                    .into_any_element(),
                            ),
                            single_example(
                                "收起状态",
                                ListHeader::new("收起分区")
                                    .toggle(Some(false))
                                    .into_any_element(),
                            ),
                        ],
                    ),
                    example_group_with_title(
                        "状态样式",
                        vec![
                            single_example(
                                "选中状态",
                                ListHeader::new("选中头部")
                                    .toggle_state(true)
                                    .into_any_element(),
                            ),
                            single_example(
                                "缩进样式",
                                ListHeader::new("缩进头部")
                                    .inset(true)
                                    .into_any_element(),
                            ),
                        ],
                    ),
                ])
                .into_any_element(),
        )
    }
}