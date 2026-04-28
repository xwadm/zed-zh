use gpui::{Anchor, AnyView, Entity, Pixels, Point};

use crate::{ButtonLike, ContextMenu, PopoverMenu, prelude::*};

use super::PopoverMenuHandle;

/// 下拉菜单样式
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DropdownStyle {
    #[default]
    /// 实心样式
    Solid,
    /// 描边样式
    Outlined,
    /// 淡色样式
    Subtle,
    /// 幽灵透明样式
    Ghost,
}

/// 标签类型：文本或自定义元素
enum LabelKind {
    Text(SharedString),
    Element(AnyElement),
}

/// 下拉菜单组件
#[derive(IntoElement, RegisterComponent)]
pub struct DropdownMenu {
    id: ElementId,
    label: LabelKind,
    trigger_size: ButtonSize,
    trigger_tooltip: Option<Box<dyn Fn(&mut Window, &mut App) -> AnyView + 'static>>,
    trigger_icon: Option<IconName>,
    style: DropdownStyle,
    menu: Entity<ContextMenu>,
    full_width: bool,
    disabled: bool,
    handle: Option<PopoverMenuHandle<ContextMenu>>,
    attach: Option<Anchor>,
    offset: Option<Point<Pixels>>,
    tab_index: Option<isize>,
    chevron: bool,
}

impl DropdownMenu {
    /// 创建文本标签的下拉菜单
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        menu: Entity<ContextMenu>,
    ) -> Self {
        Self {
            id: id.into(),
            label: LabelKind::Text(label.into()),
            trigger_size: ButtonSize::Default,
            trigger_tooltip: None,
            trigger_icon: Some(IconName::ChevronUpDown),
            style: DropdownStyle::default(),
            menu,
            full_width: false,
            disabled: false,
            handle: None,
            attach: None,
            offset: None,
            tab_index: None,
            chevron: true,
        }
    }

    /// 创建自定义元素标签的下拉菜单
    pub fn new_with_element(
        id: impl Into<ElementId>,
        label: AnyElement,
        menu: Entity<ContextMenu>,
    ) -> Self {
        Self {
            id: id.into(),
            label: LabelKind::Element(label),
            trigger_size: ButtonSize::Default,
            trigger_tooltip: None,
            trigger_icon: Some(IconName::ChevronUpDown),
            style: DropdownStyle::default(),
            menu,
            full_width: false,
            disabled: false,
            handle: None,
            attach: None,
            offset: None,
            tab_index: None,
            chevron: true,
        }
    }

    /// 设置菜单样式
    pub fn style(mut self, style: DropdownStyle) -> Self {
        self.style = style;
        self
    }

    /// 设置触发按钮大小
    pub fn trigger_size(mut self, size: ButtonSize) -> Self {
        self.trigger_size = size;
        self
    }

    /// 设置触发按钮的悬浮提示
    pub fn trigger_tooltip(
        mut self,
        tooltip: impl Fn(&mut Window, &mut App) -> AnyView + 'static,
    ) -> Self {
        self.trigger_tooltip = Some(Box::new(tooltip));
        self
    }

    /// 设置触发按钮图标
    pub fn trigger_icon(mut self, icon: IconName) -> Self {
        self.trigger_icon = Some(icon);
        self
    }

    /// 是否占满父容器宽度
    pub fn full_width(mut self, full_width: bool) -> Self {
        self.full_width = full_width;
        self
    }

    /// 设置弹出菜单句柄
    pub fn handle(mut self, handle: PopoverMenuHandle<ContextMenu>) -> Self {
        self.handle = Some(handle);
        self
    }

    /// 设置菜单依附于触发按钮的哪个角
    pub fn attach(mut self, attach: Anchor) -> Self {
        self.attach = Some(attach);
        self
    }

    /// 设置菜单偏移量（像素）
    pub fn offset(mut self, offset: Point<Pixels>) -> Self {
        self.offset = Some(offset);
        self
    }

    /// 设置 Tab 索引
    pub fn tab_index(mut self, arg: isize) -> Self {
        self.tab_index = Some(arg);
        self
    }

    /// 不显示下拉箭头
    pub fn no_chevron(mut self) -> Self {
        self.chevron = false;
        self
    }
}

impl Disableable for DropdownMenu {
    fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl RenderOnce for DropdownMenu {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let button_style = match self.style {
            DropdownStyle::Solid => ButtonStyle::Filled,
            DropdownStyle::Subtle => ButtonStyle::Subtle,
            DropdownStyle::Outlined => ButtonStyle::Outlined,
            DropdownStyle::Ghost => ButtonStyle::Transparent,
        };

        let full_width = self.full_width;
        let trigger_size = self.trigger_size;

        let (text_button, element_button) = match self.label {
            LabelKind::Text(text) => (
                Some(
                    Button::new(self.id.clone(), text)
                        .style(button_style)
                        .when_some(self.trigger_icon.filter(|_| self.chevron), |this, icon| {
                            this.end_icon(
                                Icon::new(icon).size(IconSize::XSmall).color(Color::Muted),
                            )
                        })
                        .when(full_width, |this| this.full_width())
                        .size(trigger_size)
                        .disabled(self.disabled)
                        .when_some(self.tab_index, |this, tab_index| this.tab_index(tab_index)),
                ),
                None,
            ),
            LabelKind::Element(element) => (
                None,
                Some(
                    ButtonLike::new(self.id.clone())
                        .child(element)
                        .style(button_style)
                        .when(self.chevron, |this| {
                            this.child(
                                Icon::new(IconName::ChevronUpDown)
                                    .size(IconSize::XSmall)
                                    .color(Color::Muted),
                            )
                        })
                        .when(full_width, |this| this.full_width())
                        .size(trigger_size)
                        .disabled(self.disabled)
                        .when_some(self.tab_index, |this, tab_index| this.tab_index(tab_index)),
                ),
            ),
        };

        let mut popover = PopoverMenu::new((self.id.clone(), "popover"))
            .full_width(self.full_width)
            .menu(move |_window, _cx| Some(self.menu.clone()));

        popover = match (text_button, element_button, self.trigger_tooltip) {
            (Some(text_button), None, Some(tooltip)) => {
                popover.trigger_with_tooltip(text_button, tooltip)
            }
            (Some(text_button), None, None) => popover.trigger(text_button),
            (None, Some(element_button), Some(tooltip)) => {
                popover.trigger_with_tooltip(element_button, tooltip)
            }
            (None, Some(element_button), None) => popover.trigger(element_button),
            _ => popover,
        };

        popover
            .attach(match self.attach {
                Some(attach) => attach,
                None => Anchor::BottomRight,
            })
            .when_some(self.offset, |this, offset| this.offset(offset))
            .when_some(self.handle, |this, handle| this.with_handle(handle))
    }
}

impl Component for DropdownMenu {
    fn scope() -> ComponentScope {
        ComponentScope::Input
    }

    fn name() -> &'static str {
        "DropdownMenu"
    }

    fn description() -> Option<&'static str> {
        Some(
            "下拉菜单用于展示一组操作或选项。点击触发按钮（或通过快捷键）可激活下拉菜单。",
        )
    }

    fn preview(window: &mut Window, cx: &mut App) -> Option<AnyElement> {
        let menu = ContextMenu::build(window, cx, |this, _, _| {
            this.entry("选项 1", None, |_, _| {})
                .entry("选项 2", None, |_, _| {})
                .entry("选项 3", None, |_, _| {})
                .separator()
                .entry("选项 4", None, |_, _| {})
        });

        let menu_with_submenu = ContextMenu::build(window, cx, |this, _, _| {
            this.entry("全部切换面板", None, |_, _| {})
                .submenu("编辑器布局", |menu, _, _| {
                    menu.entry("向上分割", None, |_, _| {})
                        .entry("向下分割", None, |_, _| {})
                        .separator()
                        .entry("侧边分割", None, |_, _| {})
                })
                .separator()
                .entry("项目面板", None, |_, _| {})
                .entry("大纲面板", None, |_, _| {})
                .separator()
                .submenu("自动填充", |menu, _, _| {
                    menu.entry("联系人…", None, |_, _| {})
                        .entry("密码…", None, |_, _| {})
                })
                .submenu_with_icon("预测补全", IconName::ZedPredict, |menu, _, _| {
                    menu.entry("全部位置", None, |_, _| {})
                        .entry("光标处", None, |_, _| {})
                        .entry("这里", None, |_, _| {})
                        .entry("那里", None, |_, _| {})
                })
        });

        Some(
            v_flex()
                .gap_6()
                .children(vec![
                    example_group_with_title(
                        "基础用法",
                        vec![
                            single_example(
                                "默认",
                                DropdownMenu::new("default", "选择一个选项", menu.clone())
                                    .into_any_element(),
                            ),
                            single_example(
                                "占满宽度",
                                DropdownMenu::new(
                                    "full-width",
                                    "占满宽度下拉菜单",
                                    menu.clone(),
                                )
                                .full_width(true)
                                .into_any_element(),
                            ),
                        ],
                    ),
                    example_group_with_title(
                        "子菜单",
                        vec![single_example(
                            "带子菜单",
                            DropdownMenu::new("submenu", "子菜单", menu_with_submenu)
                                .into_any_element(),
                        )],
                    ),
                    example_group_with_title(
                        "样式",
                        vec![
                            single_example(
                                "描边样式",
                                DropdownMenu::new("outlined", "描边下拉菜单", menu.clone())
                                    .style(DropdownStyle::Outlined)
                                    .into_any_element(),
                            ),
                            single_example(
                                "幽灵样式",
                                DropdownMenu::new("ghost", "幽灵下拉菜单", menu.clone())
                                    .style(DropdownStyle::Ghost)
                                    .into_any_element(),
                            ),
                        ],
                    ),
                    example_group_with_title(
                        "状态",
                        vec![single_example(
                            "禁用状态",
                            DropdownMenu::new("disabled", "禁用下拉菜单", menu)
                                .disabled(true)
                                .into_any_element(),
                        )],
                    ),
                ])
                .into_any_element(),
        )
    }
}