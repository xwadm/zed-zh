// 引入配置相关模块
use settings::{Settings, SettingsStore};

// 引入GPUI UI框架核心依赖
use gpui::{
    AnyWindowHandle, Context, Hsla, InteractiveElement, MouseButton, ParentElement, ScrollHandle,
    Styled, SystemWindowTab, SystemWindowTabController, Window, WindowId, actions, canvas, div,
};

// 引入主题配置
use theme_settings::ThemeSettings;
// 引入UI组件库
use ui::{
    Color, ContextMenu, DynamicSpacing, IconButton, IconButtonShape, IconName, IconSize, Label,
    LabelSize, Tab, h_flex, prelude::*, right_click_menu,
};
// 引入工作区相关功能
use workspace::{
    CloseWindow, ItemSettings, Workspace, WorkspaceSettings,
    item::{ClosePosition, ShowCloseButton},
};

// 定义窗口相关操作命令
actions!(
    window,
    [
        ShowNextWindowTab,    // 显示下一个窗口标签
        ShowPreviousWindowTab,// 显示上一个窗口标签
        MergeAllWindows,      // 合并所有窗口
        MoveTabToNewWindow    // 将标签移动到新窗口
    ]
);

/// 被拖拽的窗口标签数据结构
#[derive(Clone)]
pub struct DraggedWindowTab {
    pub id: WindowId,                // 窗口ID
    pub ix: usize,                   // 标签索引
    pub handle: AnyWindowHandle,     // 窗口句柄
    pub title: String,               // 标签标题
    pub width: Pixels,               // 标签宽度
    pub is_active: bool,             // 是否为激活状态
    pub active_background_color: Hsla,    // 激活状态背景色
    pub inactive_background_color: Hsla,  // 未激活状态背景色
}

/// 系统窗口标签栏组件
pub struct SystemWindowTabs {
    tab_bar_scroll_handle: ScrollHandle,  // 标签栏滚动控制器
    measured_tab_width: Pixels,           // 计算后的标签宽度
    last_dragged_tab: Option<DraggedWindowTab>,  // 最后一次拖拽的标签
}

impl SystemWindowTabs {
    /// 创建新的窗口标签栏实例
    pub fn new() -> Self {
        Self {
            tab_bar_scroll_handle: ScrollHandle::new(),
            measured_tab_width: px(0.),
            last_dragged_tab: None,
        }
    }

    /// 初始化组件（全局配置监听）
    pub fn init(cx: &mut App) {
        // 记录上一次的系统窗口标签配置状态
        let mut was_use_system_window_tabs =
            WorkspaceSettings::get_global(cx).use_system_window_tabs;

        // 监听配置存储的全局变化
        cx.observe_global::<SettingsStore>(move |cx| {
            let use_system_window_tabs = WorkspaceSettings::get_global(cx).use_system_window_tabs;
            // 配置未变化时直接返回
            if use_system_window_tabs == was_use_system_window_tabs {
                return;
            }
            was_use_system_window_tabs = use_system_window_tabs;

            // 设置窗口分组标识（用于系统标签合并）
            let tabbing_identifier = if use_system_window_tabs {
                Some(String::from("zed"))
            } else {
                None
            };

            // 启用系统窗口标签时初始化控制器
            if use_system_window_tabs {
                SystemWindowTabController::init(cx);
            }

            // 遍历所有窗口，更新标签配置
            cx.windows().iter().for_each(|handle| {
                let _ = handle.update(cx, |_, window, cx| {
                    window.set_tabbing_identifier(tabbing_identifier.clone());
                    if use_system_window_tabs {
                        // 获取窗口标签组，不存在则创建当前窗口标签
                        let tabs = if let Some(tabs) = window.tabbed_windows() {
                            tabs
                        } else {
                            vec![SystemWindowTab::new(
                                SharedString::from(window.window_title()),
                                window.window_handle(),
                            )]
                        };

                        // 将标签添加到系统窗口标签控制器
                        SystemWindowTabController::add_tab(cx, handle.window_id(), tabs);
                    }
                });
            });
        })
        .detach();

        // 监听新工作区创建，注册命令渲染器
        cx.observe_new(|workspace: &mut Workspace, _, _| {
            workspace.register_action_renderer(|div, _, window, cx| {
                let window_id = window.window_handle().window_id();
                let controller = cx.global::<SystemWindowTabController>();

                let tab_groups = controller.tab_groups();
                let tabs = controller.tabs(window_id);
                // 无标签时直接返回
                let Some(tabs) = tabs else {
                    return div;
                };

                // 多个标签时注册切换命令
                div.when(tabs.len() > 1, |div| {
                    div.on_action(move |_: &ShowNextWindowTab, window, cx| {
                        SystemWindowTabController::select_next_tab(
                            cx,
                            window.window_handle().window_id(),
                        );
                    })
                    .on_action(move |_: &ShowPreviousWindowTab, window, cx| {
                        SystemWindowTabController::select_previous_tab(
                            cx,
                            window.window_handle().window_id(),
                        );
                    })
                    .on_action(move |_: &MoveTabToNewWindow, window, cx| {
                        SystemWindowTabController::move_tab_to_new_window(
                            cx,
                            window.window_handle().window_id(),
                        );
                        window.move_tab_to_new_window();
                    })
                })
                // 多个标签组时注册合并命令
                .when(tab_groups.len() > 1, |div| {
                    div.on_action(move |_: &MergeAllWindows, window, cx| {
                        SystemWindowTabController::merge_all_windows(
                            cx,
                            window.window_handle().window_id(),
                        );
                        window.merge_all_windows();
                    })
                })
            });
        })
        .detach();
    }

    /// 渲染单个窗口标签
    fn render_tab(
        &self,
        ix: usize,
        item: SystemWindowTab,
        tabs: Vec<SystemWindowTab>,
        active_background_color: Hsla,
        inactive_background_color: Hsla,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let entity = cx.entity();
        let settings = ItemSettings::get_global(cx);
        let close_side = &settings.close_position;       // 关闭按钮位置
        let show_close_button = &settings.show_close_button; // 关闭按钮显示规则

        let rem_size = window.rem_size();
        let width = self.measured_tab_width.max(rem_size * 10);
        let is_active = window.window_handle().window_id() == item.id;
        let title = item.title.to_string();

        // 创建标签文字
        let label = Label::new(&title)
            .size(LabelSize::Small)
            .truncate()
            .color(if is_active {
                Color::Default
            } else {
                Color::Muted
            });

        // 标签主体布局与交互
        let tab = h_flex()
            .id(ix)
            .group("tab")
            .w_full()
            .overflow_hidden()
            .h(Tab::content_height(cx))
            .relative()
            .px(DynamicSpacing::Base16.px(cx))
            .justify_center()
            .border_l_1()
            .border_color(cx.theme().colors().border)
            .cursor_pointer()
            // 拖拽事件：记录拖拽的标签
            .on_drag(
                DraggedWindowTab {
                    id: item.id,
                    ix,
                    handle: item.handle,
                    title: item.title.to_string(),
                    width,
                    is_active,
                    active_background_color,
                    inactive_background_color,
                },
                move |tab, _, _, cx| {
                    entity.update(cx, |this, _cx| {
                        this.last_dragged_tab = Some(tab.clone());
                    });
                    cx.new(|_| tab.clone())
                },
            )
            // 拖拽悬停效果：显示放置位置边框
            .drag_over::<DraggedWindowTab>({
                let tab_ix = ix;
                move |element, dragged_tab: &DraggedWindowTab, _, cx| {
                    let mut styled_tab = element
                        .bg(cx.theme().colors().drop_target_background)
                        .border_color(cx.theme().colors().drop_target_border)
                        .border_0();

                    if tab_ix < dragged_tab.ix {
                        styled_tab = styled_tab.border_l_2();
                    } else if tab_ix > dragged_tab.ix {
                        styled_tab = styled_tab.border_r_2();
                    }

                    styled_tab
                }
            })
            // 放置事件：调整标签位置
            .on_drop({
                let tab_ix = ix;
                cx.listener(move |this, dragged_tab: &DraggedWindowTab, _window, cx| {
                    this.last_dragged_tab = None;
                    Self::handle_tab_drop(dragged_tab, tab_ix, cx);
                })
            })
            // 点击：激活对应窗口
            .on_click(move |_, _, cx| {
                let _ = item.handle.update(cx, |_, window, _| {
                    window.activate_window();
                });
            })
            // 鼠标中键点击：关闭标签
            .on_mouse_up(MouseButton::Middle, move |_, window, cx| {
                if item.handle.window_id() == window.window_handle().window_id() {
                    window.dispatch_action(Box::new(CloseWindow), cx);
                } else {
                    let _ = item.handle.update(cx, |_, window, cx| {
                        window.dispatch_action(Box::new(CloseWindow), cx);
                    });
                }
            })
            .child(label)
            // 根据配置添加关闭按钮
            .map(|this| match show_close_button {
                ShowCloseButton::Hidden => this,
                _ => this.child(
                    div()
                        .absolute()
                        .top_2()
                        .w_4()
                        .h_4()
                        // 设置关闭按钮左右位置
                        .map(|this| match close_side {
                            ClosePosition::Left => this.left_1(),
                            ClosePosition::Right => this.right_1(),
                        })
                        .child(
                            IconButton::new("close", IconName::Close)
                                .shape(IconButtonShape::Square)
                                .icon_color(Color::Muted)
                                .icon_size(IconSize::XSmall)
                                // 关闭按钮点击事件
                                .on_click({
                                    move |_, window, cx| {
                                        if item.handle.window_id()
                                            == window.window_handle().window_id()
                                        {
                                            window.dispatch_action(Box::new(CloseWindow), cx);
                                        } else {
                                            let _ = item.handle.update(cx, |_, window, cx| {
                                                window.dispatch_action(Box::new(CloseWindow), cx);
                                            });
                                        }
                                    }
                                })
                                // 悬停显示/常驻显示控制
                                .map(|this| match show_close_button {
                                    ShowCloseButton::Hover => this.visible_on_hover("tab"),
                                    _ => this,
                                }),
                        ),
                ),
            })
            .into_any();

        // 右键菜单
        let menu = right_click_menu(ix)
            .trigger(|_, _, _| tab)
            .menu(move |window, cx| {
                let focus_handle = cx.focus_handle();
                let tabs = tabs.clone();
                let other_tabs = tabs.clone();
                let move_tabs = tabs.clone();
                let merge_tabs = tabs.clone();

                ContextMenu::build(window, cx, move |mut menu, _window_, _cx| {
                    // 关闭当前标签
                    menu = menu.entry("关闭标签", None, move |window, cx| {
                        Self::handle_right_click_action(
                            cx,
                            window,
                            &tabs,
                            |tab| tab.id == item.id,
                            |window, cx| {
                                window.dispatch_action(Box::new(CloseWindow), cx);
                            },
                        );
                    });

                    // 关闭其他标签
                    menu = menu.entry("关闭其他标签", None, move |window, cx| {
                        Self::handle_right_click_action(
                            cx,
                            window,
                            &other_tabs,
                            |tab| tab.id != item.id,
                            |window, cx| {
                                window.dispatch_action(Box::new(CloseWindow), cx);
                            },
                        );
                    });

                    // 将标签移动到新窗口
                    menu = menu.entry("移动标签到新窗口", None, move |window, cx| {
                        Self::handle_right_click_action(
                            cx,
                            window,
                            &move_tabs,
                            |tab| tab.id == item.id,
                            |window, cx| {
                                SystemWindowTabController::move_tab_to_new_window(
                                    cx,
                                    window.window_handle().window_id(),
                                );
                                window.move_tab_to_new_window();
                            },
                        );
                    });

                    // 显示所有标签概览
                    menu = menu.entry("显示所有标签", None, move |window, cx| {
                        Self::handle_right_click_action(
                            cx,
                            window,
                            &merge_tabs,
                            |tab| tab.id == item.id,
                            |window, _cx| {
                                window.toggle_window_tab_overview();
                            },
                        );
                    });

                    menu.context(focus_handle)
                })
            });

        // 标签容器：设置激活/未激活样式
        div()
            .flex_1()
            .min_w(rem_size * 10)
            .when(is_active, |this| this.bg(active_background_color))
            .border_t_1()
            .border_color(if is_active {
                active_background_color
            } else {
                cx.theme().colors().border
            })
            .child(menu)
    }

    /// 处理标签拖拽放置：更新标签位置
    fn handle_tab_drop(dragged_tab: &DraggedWindowTab, ix: usize, cx: &mut Context<Self>) {
        SystemWindowTabController::update_tab_position(cx, dragged_tab.id, ix);
    }

    /// 处理右键菜单通用逻辑
    fn handle_right_click_action<F, P>(
        cx: &mut App,
        window: &mut Window,
        tabs: &Vec<SystemWindowTab>,
        predicate: P,
        mut action: F,
    ) where
        P: Fn(&SystemWindowTab) -> bool,
        F: FnMut(&mut Window, &mut App),
    {
        for tab in tabs {
            if predicate(tab) {
                // 匹配标签时执行对应操作
                if tab.id == window.window_handle().window_id() {
                    action(window, cx);
                } else {
                    let _ = tab.handle.update(cx, |_view, window, cx| {
                        action(window, cx);
                    });
                }
            }
        }
    }
}

/// 实现窗口标签栏渲染逻辑
impl Render for SystemWindowTabs {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let use_system_window_tabs = WorkspaceSettings::get_global(cx).use_system_window_tabs;
        let active_background_color = cx.theme().colors().title_bar_background;
        let inactive_background_color = cx.theme().colors().tab_bar_background;
        let entity = cx.entity();

        let controller = cx.global::<SystemWindowTabController>();
        let visible = controller.is_visible();
        // 默认标签：当前窗口
        let current_window_tab = vec![SystemWindowTab::new(
            SharedString::from(window.window_title()),
            window.window_handle(),
        )];
        // 获取当前窗口的所有标签
        let tabs = controller
            .tabs(window.window_handle().window_id())
            .unwrap_or(&current_window_tab)
            .clone();

        // 渲染所有标签项
        let tab_items = tabs
            .iter()
            .enumerate()
            .map(|(ix, item)| {
                self.render_tab(
                    ix,
                    item.clone(),
                    tabs.clone(),
                    active_background_color,
                    inactive_background_color,
                    window,
                    cx,
                )
            })
            .collect::<Vec<_>>();

        let number_of_tabs = tab_items.len().max(1);
        // 不满足显示条件时返回空布局
        if (!window.tab_bar_visible() && !visible)
            || (!use_system_window_tabs && number_of_tabs == 1)
        {
            return h_flex().into_any_element();
        }

        // 标签栏整体布局
        h_flex()
            .w_full()
            .h(Tab::container_height(cx))
            .bg(inactive_background_color)
            // 拖拽到标签栏外：新建窗口
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _event, window, cx| {
                    if let Some(tab) = this.last_dragged_tab.take() {
                        SystemWindowTabController::move_tab_to_new_window(cx, tab.id);
                        if tab.id == window.window_handle().window_id() {
                            window.move_tab_to_new_window();
                        } else {
                            let _ = tab.handle.update(cx, |_, window, _cx| {
                                window.move_tab_to_new_window();
                            });
                        }
                    }
                }),
            )
            // 标签滚动容器
            .child(
                h_flex()
                    .id("window tabs")
                    .w_full()
                    .h(Tab::container_height(cx))
                    .bg(inactive_background_color)
                    .overflow_x_scroll()
                    .track_scroll(&self.tab_bar_scroll_handle)
                    .children(tab_items)
                    // 画布：动态计算标签宽度
                    .child(
                        canvas(
                            |_, _, _| (),
                            move |bounds, _, _, cx| {
                                let entity = entity.clone();
                                entity.update(cx, |this, cx| {
                                    let width = bounds.size.width / number_of_tabs as f32;
                                    if width != this.measured_tab_width {
                                        this.measured_tab_width = width;
                                        cx.notify();
                                    }
                                });
                            },
                        )
                        .absolute()
                        .size_full(),
                    ),
            )
            // 新建窗口按钮
            .child(
                h_flex()
                    .h_full()
                    .px(DynamicSpacing::Base06.rems(cx))
                    .border_t_1()
                    .border_l_1()
                    .border_color(cx.theme().colors().border)
                    .child(
                        IconButton::new("plus", IconName::Plus)
                            .icon_size(IconSize::Small)
                            .icon_color(Color::Muted)
                            .on_click(|_event, window, cx| {
                                window.dispatch_action(
                                    Box::new(zed_actions::OpenRecent {
                                        create_new_window: true,
                                    }),
                                    cx,
                                );
                            }),
                    ),
            )
            .into_any_element()
    }
}

/// 实现拖拽标签的渲染样式
impl Render for DraggedWindowTab {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        let ui_font = ThemeSettings::get_global(cx).ui_font.clone();
        let label = Label::new(self.title.clone())
            .size(LabelSize::Small)
            .truncate()
            .color(if self.is_active {
                Color::Default
            } else {
                Color::Muted
            });

        // 拖拽时的标签预览样式
        h_flex()
            .h(Tab::container_height(cx))
            .w(self.width)
            .px(DynamicSpacing::Base16.px(cx))
            .justify_center()
            .bg(if self.is_active {
                self.active_background_color
            } else {
                self.inactive_background_color
            })
            .border_1()
            .border_color(cx.theme().colors().border)
            .font(ui_font)
            .child(label)
    }
}