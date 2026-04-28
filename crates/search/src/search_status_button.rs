use editor::EditorSettings;
use gpui::FocusHandle;
use settings::Settings as _;
use ui::{ButtonCommon, Clickable, Context, Render, Tooltip, Window, prelude::*};
use workspace::{ItemHandle, StatusItemView};

/// 搜索图标常量
pub const SEARCH_ICON: IconName = IconName::MagnifyingGlass;

/// 搜索按钮组件
pub struct SearchButton {
    /// 面板项焦点句柄
    pane_item_focus_handle: Option<FocusHandle>,
}

impl SearchButton {
    /// 创建新的搜索按钮实例
    pub fn new() -> Self {
        Self {
            pane_item_focus_handle: None,
        }
    }
}

impl Render for SearchButton {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl ui::IntoElement {
        let button = div();

        // 根据编辑器设置判断是否显示搜索按钮
        if !EditorSettings::get_global(cx).search.button {
            return button.hidden();
        }

        let focus_handle = self.pane_item_focus_handle.clone();
        button.child(
            IconButton::new("project-search-indicator", SEARCH_ICON)
                .icon_size(IconSize::Small)
                // 设置按钮悬停提示
                .tooltip(move |_window, cx| {
                    if let Some(focus_handle) = &focus_handle {
                        Tooltip::for_action_in(
                            "项目搜索",
                            &workspace::DeploySearch::default(),
                            focus_handle,
                            cx,
                        )
                    } else {
                        Tooltip::for_action(
                            "项目搜索",
                            &workspace::DeploySearch::default(),
                            cx,
                        )
                    }
                })
                // 点击按钮触发项目搜索操作
                .on_click(cx.listener(|_this, _, window, cx| {
                    window.dispatch_action(Box::new(workspace::DeploySearch::default()), cx);
                })),
        )
    }
}

impl StatusItemView for SearchButton {
    /// 设置激活的面板项，更新焦点句柄
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pane_item_focus_handle = active_pane_item.map(|item| item.item_focus_handle(cx));
    }
}