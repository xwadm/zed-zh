#[cfg(test)]
mod tab_switcher_tests;

use collections::{HashMap, HashSet};
use editor::items::{
    entry_diagnostic_aware_icon_decoration_and_color, entry_git_aware_label_color,
};
use fuzzy_nucleo::StringMatchCandidate;
use gpui::{
    Action, AnyElement, App, Context, DismissEvent, Entity, EntityId, EventEmitter, FocusHandle,
    Focusable, Modifiers, ModifiersChangedEvent, MouseButton, MouseUpEvent, ParentElement, Point,
    Render, Styled, Task, WeakEntity, Window, actions, rems,
};
use picker::{Picker, PickerDelegate};
use project::Project;
use schemars::JsonSchema;
use serde::Deserialize;
use settings::Settings;
use std::{cmp::Reverse, sync::Arc};
use ui::{
    DecoratedIcon, IconDecoration, IconDecorationKind, ListItem, ListItemSpacing, Tooltip,
    prelude::*,
};
use util::ResultExt;
use workspace::{
    Event as WorkspaceEvent, ModalView, Pane, SaveIntent, Workspace,
    item::{ItemHandle, ItemSettings, ShowDiagnostics, TabContentParams},
    pane::{render_item_indicator, tab_details},
};

/// 面板宽度（单位：rem）
const PANEL_WIDTH_REMS: f32 = 28.;

/// 切换标签页切换器界面
#[derive(PartialEq, Clone, Deserialize, JsonSchema, Default, Action)]
#[action(namespace = tab_switcher)]
#[serde(deny_unknown_fields)]
pub struct Toggle {
    #[serde(default)]
    pub select_last: bool,
}

actions!(
    tab_switcher,
    [
        /// 关闭标签页切换器中选中的项目
        CloseSelectedItem,
        /// 切换显示所有标签页或仅当前面板的标签页
        ToggleAll,
        /// 切换显示所有面板中去重的标签页，在活动面板中打开选中项目
        OpenInActivePane,
    ]
);

/// 标签页切换器
pub struct TabSwitcher {
    picker: Entity<Picker<TabSwitcherDelegate>>,
    init_modifiers: Option<Modifiers>,
}

impl ModalView for TabSwitcher {}

/// 初始化标签页切换器
pub fn init(cx: &mut App) {
    cx.observe_new(TabSwitcher::register).detach();
}

impl TabSwitcher {
    /// 注册标签页切换器相关操作
    fn register(
        workspace: &mut Workspace,
        _window: Option<&mut Window>,
        _: &mut Context<Workspace>,
    ) {
        workspace.register_action(|workspace, action: &Toggle, window, cx| {
            let Some(tab_switcher) = workspace.active_modal::<Self>(cx) else {
                Self::open(workspace, action.select_last, false, false, window, cx);
                return;
            };

            tab_switcher.update(cx, |tab_switcher, cx| {
                tab_switcher
                    .picker
                    .update(cx, |picker, cx| picker.cycle_selection(window, cx))
            });
        });

        workspace.register_action(|workspace, _action: &ToggleAll, window, cx| {
            let Some(tab_switcher) = workspace.active_modal::<Self>(cx) else {
                Self::open(workspace, false, true, false, window, cx);
                return;
            };

            tab_switcher.update(cx, |tab_switcher, cx| {
                tab_switcher
                    .picker
                    .update(cx, |picker, cx| picker.cycle_selection(window, cx))
            });
        });

        workspace.register_action(|workspace, _action: &OpenInActivePane, window, cx| {
            let Some(tab_switcher) = workspace.active_modal::<Self>(cx) else {
                Self::open(workspace, false, true, true, window, cx);
                return;
            };

            tab_switcher.update(cx, |tab_switcher, cx| {
                tab_switcher
                    .picker
                    .update(cx, |picker, cx| picker.cycle_selection(window, cx))
            });
        });
    }

    /// 打开标签页切换器
    fn open(
        workspace: &mut Workspace,
        select_last: bool,
        is_global: bool,
        open_in_active_pane: bool,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let mut weak_pane = workspace.active_pane().downgrade();
        // 检查各个停靠面板，获取焦点所在面板
        for dock in [
            workspace.left_dock(),
            workspace.bottom_dock(),
            workspace.right_dock(),
        ] {
            dock.update(cx, |this, cx| {
                let Some(panel) = this
                    .active_panel()
                    .filter(|panel| panel.panel_focus_handle(cx).contains_focused(window, cx))
                else {
                    return;
                };
                if let Some(pane) = panel.pane(cx) {
                    weak_pane = pane.downgrade();
                }
            })
        }

        let weak_workspace = workspace.weak_handle();
        let project = workspace.project().clone();
        // 保存原始激活项状态，用于关闭时恢复
        let original_items: Vec<_> = workspace
            .panes()
            .iter()
            .map(|p| (p.clone(), p.read(cx).active_item_index()))
            .collect();

        // 打开模态窗口
        workspace.toggle_modal(window, cx, |window, cx| {
            let delegate = TabSwitcherDelegate::new(
                project,
                select_last,
                cx.entity().downgrade(),
                weak_pane,
                weak_workspace,
                is_global,
                open_in_active_pane,
                window,
                cx,
                original_items,
            );
            TabSwitcher::new(delegate, window, is_global, cx)
        });
    }

    /// 创建标签页切换器实例
    fn new(
        delegate: TabSwitcherDelegate,
        window: &mut Window,
        is_global: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let init_modifiers = if is_global {
            None
        } else {
            window.modifiers().modified().then_some(window.modifiers())
        };

        Self {
            picker: cx.new(|cx| {
                if is_global {
                    Picker::list(delegate, window, cx)
                } else {
                    Picker::nonsearchable_list(delegate, window, cx)
                }
            }),
            init_modifiers,
        }
    }

    /// 处理修饰键变化事件
    fn handle_modifiers_changed(
        &mut self,
        event: &ModifiersChangedEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(init_modifiers) = self.init_modifiers else {
            return;
        };
        // 修饰键释放时确认选择或关闭切换器
        if !event.modified() || !init_modifiers.is_subset_of(event) {
            self.init_modifiers = None;
            if self.picker.read(cx).delegate.matches.is_empty() {
                cx.emit(DismissEvent)
            } else {
                window.dispatch_action(menu::Confirm.boxed_clone(), cx);
            }
        }
    }

    /// 关闭选中的项目
    fn handle_close_selected_item(
        &mut self,
        _: &CloseSelectedItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.picker.update(cx, |picker, cx| {
            picker
                .delegate
                .close_item_at(picker.delegate.selected_index(), window, cx)
        });
    }
}

impl EventEmitter<DismissEvent> for TabSwitcher {}

impl Focusable for TabSwitcher {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.picker.focus_handle(cx)
    }
}

impl Render for TabSwitcher {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context("TabSwitcher")
            .w(rems(PANEL_WIDTH_REMS))
            .on_modifiers_changed(cx.listener(Self::handle_modifiers_changed))
            .on_action(cx.listener(Self::handle_close_selected_item))
            .child(self.picker.clone())
    }
}

/// 标签页匹配项
#[derive(Clone)]
struct TabMatch {
    pane: WeakEntity<Pane>,
    item_index: usize,
    item: Box<dyn ItemHandle>,
    detail: usize,
    preview: bool,
}

impl TabMatch {
    /// 获取标签页图标（包含诊断、Git状态装饰）
    fn icon(
        &self,
        project: &Entity<Project>,
        selected: bool,
        window: &Window,
        cx: &App,
    ) -> Option<DecoratedIcon> {
        let icon = self.item.tab_icon(window, cx)?;
        let item_settings = ItemSettings::get_global(cx);
        let show_diagnostics = item_settings.show_diagnostics;

        // 设置Git状态颜色
        let git_status_color = item_settings
            .git_status
            .then(|| {
                let path = self.item.project_path(cx)?;
                let project = project.read(cx);
                let entry = project.entry_for_path(&path, cx)?;
                let git_status = project
                    .project_path_git_status(&path, cx)
                    .map(|status| status.summary())
                    .unwrap_or_default();
                Some(entry_git_aware_label_color(
                    git_status,
                    entry.is_ignored,
                    selected,
                ))
            })
            .flatten();
        let colored_icon = icon.color(git_status_color.unwrap_or_default());

        // 获取诊断信息级别
        let most_severe_diagnostic_level = if show_diagnostics == ShowDiagnostics::Off {
            None
        } else {
            let buffer_store = project.read(cx).buffer_store().read(cx);
            let buffer = self
                .item
                .project_path(cx)
                .and_then(|path| buffer_store.get_by_path(&path))
                .map(|buffer| buffer.read(cx));
            buffer.and_then(|buffer| {
                buffer
                    .buffer_diagnostics(None)
                    .iter()
                    .map(|diagnostic_entry| diagnostic_entry.diagnostic.severity)
                    .min()
            })
        };

        // 生成诊断装饰图标
        let decorations =
            entry_diagnostic_aware_icon_decoration_and_color(most_severe_diagnostic_level)
                .filter(|(d, _)| {
                    *d != IconDecorationKind::Triangle
                        || show_diagnostics != ShowDiagnostics::Errors
                })
                .map(|(icon, color)| {
                    let knockout_item_color = if selected {
                        cx.theme().colors().element_selected
                    } else {
                        cx.theme().colors().element_background
                    };
                    IconDecoration::new(icon, knockout_item_color, cx)
                        .color(color.color(cx))
                        .position(Point {
                            x: px(-2.),
                            y: px(-2.),
                        })
                });

        Some(DecoratedIcon::new(colored_icon, decorations))
    }
}

/// 标签页切换器代理
pub struct TabSwitcherDelegate {
    select_last: bool,
    tab_switcher: WeakEntity<TabSwitcher>,
    selected_index: usize,
    pane: WeakEntity<Pane>,
    workspace: WeakEntity<Workspace>,
    project: Entity<Project>,
    matches: Vec<TabMatch>,
    original_items: Vec<(Entity<Pane>, usize)>,
    is_all_panes: bool,
    open_in_active_pane: bool,
    restored_items: bool,
}

impl TabSwitcherDelegate {
    /// 创建代理实例
    #[allow(clippy::complexity)]
    fn new(
        project: Entity<Project>,
        select_last: bool,
        tab_switcher: WeakEntity<TabSwitcher>,
        pane: WeakEntity<Pane>,
        workspace: WeakEntity<Workspace>,
        is_all_panes: bool,
        open_in_active_pane: bool,
        window: &mut Window,
        cx: &mut Context<TabSwitcher>,
        original_items: Vec<(Entity<Pane>, usize)>,
    ) -> Self {
        Self::subscribe_to_updates(&workspace, window, cx);
        Self {
            select_last,
            tab_switcher,
            selected_index: 0,
            pane,
            workspace,
            project,
            matches: Vec::new(),
            is_all_panes,
            open_in_active_pane,
            original_items,
            restored_items: false,
        }
    }

    /// 订阅工作区更新事件
    fn subscribe_to_updates(
        workspace: &WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<TabSwitcher>,
    ) {
        let Some(workspace) = workspace.upgrade() else {
            return;
        };
        cx.subscribe_in(&workspace, window, |tab_switcher, _, event, window, cx| {
            match event {
                // 项目添加/面板移除时刷新匹配列表
                WorkspaceEvent::ItemAdded { .. } | WorkspaceEvent::PaneRemoved => {
                    tab_switcher.picker.update(cx, |picker, cx| {
                        let query = picker.query(cx);
                        picker.delegate.update_matches(query, window, cx);
                        cx.notify();
                    })
                }
                // 项目移除时刷新并同步选中索引
                WorkspaceEvent::ItemRemoved { .. } => {
                    tab_switcher.picker.update(cx, |picker, cx| {
                        let query = picker.query(cx);
                        picker.delegate.update_matches(query, window, cx);
                        picker.delegate.sync_selected_index(cx);
                        cx.notify();
                    })
                }
                _ => {}
            };
        })
        .detach();
    }

    /// 更新所有面板的标签页匹配项
    fn update_all_pane_matches(
        &mut self,
        query: String,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };

        let mut all_items = Vec::new();
        let mut item_index = 0;
        // 收集所有面板的标签页
        for pane_handle in workspace.read(cx).panes() {
            let pane = pane_handle.read(cx);
            let items: Vec<Box<dyn ItemHandle>> =
                pane.items().map(|item| item.boxed_clone()).collect();

            for ((_detail, item), detail) in items
                .iter()
                .enumerate()
                .zip(tab_details(&items, window, cx))
            {
                all_items.push(TabMatch {
                    pane: pane_handle.downgrade(),
                    item_index,
                    item: item.clone(),
                    detail,
                    preview: pane.is_active_preview_item(item.item_id()),
                });
                item_index += 1;
            }
        }

        // 模糊搜索匹配
        let mut matches = if query.is_empty() {
            // 空查询按激活历史排序
            let history = workspace.read(cx).recently_activated_items(cx);
            all_items
                .sort_by_key(|tab| (Reverse(history.get(&tab.item.item_id())), tab.item_index));
            all_items
        } else {
            // 执行模糊匹配
            let candidates = all_items
                .iter()
                .enumerate()
                .flat_map(|(ix, tab_match)| {
                    Some(StringMatchCandidate::new(
                        ix,
                        &tab_match.item.tab_content_text(0, cx),
                    ))
                })
                .collect::<Vec<_>>();

            fuzzy_nucleo::match_strings(
                &candidates,
                &query,
                fuzzy_nucleo::Case::Smart,
                fuzzy_nucleo::LengthPenalty::On,
                10000,
            )
            .into_iter()
            .map(|m| all_items[m.candidate_id].clone())
            .collect()
        };

        // 活动面板模式下去重
        if self.open_in_active_pane {
            let mut seen_paths: HashSet<project::ProjectPath> = HashSet::default();
            matches.retain(|tab| {
                if let Some(path) = tab.item.project_path(cx) {
                    seen_paths.insert(path)
                } else {
                    true
                }
            });
        }

        let selected_item_id = self.selected_item_id();
        self.matches = matches;
        self.selected_index = self.compute_selected_index(selected_item_id, window, cx);
    }

    /// 更新匹配项列表
    fn update_matches(
        &mut self,
        query: String,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) {
        if self.is_all_panes {
            // 全局模式延迟执行避免借用冲突
            let this = cx.entity();
            window.defer(cx, move |window, cx| {
                this.update(cx, |this, cx| {
                    this.delegate.update_all_pane_matches(query, window, cx);
                })
            });
            return;
        }

        let selected_item_id = self.selected_item_id();
        self.matches.clear();
        let Some(pane) = self.pane.upgrade() else {
            return;
        };

        let pane = pane.read(cx);
        // 构建激活历史索引
        let mut history_indices = HashMap::default();
        pane.activation_history().iter().rev().enumerate().for_each(
            |(history_index, history_entry)| {
                history_indices.insert(history_entry.entity_id, history_index);
            },
        );

        // 收集当前面板标签页
        let items: Vec<Box<dyn ItemHandle>> = pane.items().map(|item| item.boxed_clone()).collect();
        items
            .iter()
            .enumerate()
            .zip(tab_details(&items, window, cx))
            .map(|((item_index, item), detail)| TabMatch {
                pane: self.pane.clone(),
                item_index,
                item: item.boxed_clone(),
                detail,
                preview: pane.is_active_preview_item(item.item_id()),
            })
            .for_each(|tab_match| self.matches.push(tab_match));

        // 按激活历史排序
        let non_history_base = history_indices.len();
        self.matches.sort_by(move |a, b| {
            let a_score = *history_indices
                .get(&a.item.item_id())
                .unwrap_or(&(a.item_index + non_history_base));
            let b_score = *history_indices
                .get(&b.item.item_id())
                .unwrap_or(&(b.item_index + non_history_base));
            a_score.cmp(&b_score)
        });

        self.selected_index = self.compute_selected_index(selected_item_id, window, cx);
    }

    /// 获取选中项目ID
    fn selected_item_id(&self) -> Option<EntityId> {
        self.matches
            .get(self.selected_index())
            .map(|tab_match| tab_match.item.item_id())
    }

    /// 计算选中索引
    fn compute_selected_index(
        &mut self,
        prev_selected_item_id: Option<EntityId>,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> usize {
        if self.matches.is_empty() {
            return 0;
        }

        // 优先保持原有选中项
        if let Some(selected_item_id) = prev_selected_item_id {
            if let Some(item_index) = self
                .matches
                .iter()
                .position(|tab_match| tab_match.item.item_id() == selected_item_id)
            {
                return item_index;
            }
            return self.selected_index.min(self.matches.len() - 1);
        }

        // 选择最后一项
        if self.select_last {
            let item_index = self.matches.len() - 1;
            self.set_selected_index(item_index, window, cx);
            return item_index;
        }

        // 默认选中第二项（第一项为当前激活项）
        if self.matches.len() > 1 {
            self.set_selected_index(1, window, cx);
            return 1;
        }

        0
    }

    /// 关闭指定索引的项目
    fn close_item_at(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Picker<TabSwitcherDelegate>>,
    ) {
        let Some(tab_match) = self.matches.get(ix) else {
            return;
        };

        // 活动面板模式：关闭所有同路径项目
        if self.open_in_active_pane
            && let Some(project_path) = tab_match.item.project_path(cx)
        {
            let Some(workspace) = self.workspace.upgrade() else {
                return;
            };
            workspace.update(cx, |workspace, cx| {
                workspace.close_items_with_project_path(
                    &project_path,
                    SaveIntent::Close,
                    true,
                    window,
                    cx,
                );
            });
        } else {
            // 普通模式：关闭对应面板项目
            let Some(pane) = tab_match.pane.upgrade() else {
                return;
            };
            pane.update(cx, |pane, cx| {
                pane.close_item_by_id(tab_match.item.item_id(), SaveIntent::Close, window, cx)
                    .detach_and_log_err(cx);
            });
        }
    }

    /// 同步选中索引与面板激活项
    fn sync_selected_index(&mut self, cx: &mut Context<Picker<TabSwitcherDelegate>>) {
        let item = if self.is_all_panes {
            self.workspace
                .read_with(cx, |workspace, cx| workspace.active_item(cx))
        } else {
            self.pane.read_with(cx, |pane, _cx| pane.active_item())
        };

        let Ok(Some(item)) = item else {
            return;
        };

        let item_id = item.item_id();
        let Some((index, _tab_match)) = self
            .matches
            .iter()
            .enumerate()
            .find(|(_index, tab_match)| tab_match.item.item_id() == item_id)
        else {
            return;
        };

        self.selected_index = index;
    }

    /// 在活动面板中打开选中项
    fn confirm_open_in_active_pane(
        &mut self,
        selected_match: TabMatch,
        window: &mut Window,
        cx: &mut Context<Picker<TabSwitcherDelegate>>,
    ) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };

        // 获取当前活动面板
        let current_pane = self
            .pane
            .upgrade()
            .filter(|pane| {
                workspace
                    .read(cx)
                    .panes()
                    .iter()
                    .any(|p| p.entity_id() == pane.entity_id())
            })
            .or_else(|| selected_match.pane.upgrade());

        let Some(current_pane) = current_pane else {
            return;
        };

        // 项目已存在则激活
        if let Some(index) = current_pane
            .read(cx)
            .index_for_item(selected_match.item.as_ref())
        {
            current_pane.update(cx, |pane, cx| {
                pane.activate_item(index, true, true, window, cx);
            });
        } else if selected_match.item.project_path(cx).is_some()
            && selected_match.item.can_split(cx)
        {
            // 克隆项目并添加到当前面板
            let database_id = workspace.read(cx).database_id();
            let task = selected_match.item.clone_on_split(database_id, window, cx);
            let current_pane = current_pane.downgrade();

            cx.spawn_in(window, async move |_, cx| {
                if let Some(clone) = task.await {
                    current_pane
                        .update_in(cx, |pane, window, cx| {
                            pane.add_item(clone, true, true, None, window, cx);
                        })
                        .log_err();
                }
            })
            .detach();
        } else {
            // 移动项目到当前面板
            let Some(source_pane) = selected_match.pane.upgrade() else {
                return;
            };
            workspace::move_item(
                &source_pane,
                &current_pane,
                selected_match.item.item_id(),
                current_pane.read(cx).items_len(),
                true,
                window,
                cx,
            );
        }
    }
}

impl PickerDelegate for TabSwitcherDelegate {
    type ListItem = ListItem;

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        "搜索所有标签页…".into()
    }

    fn no_matches_text(&self, _window: &mut Window, _cx: &mut App) -> Option<SharedString> {
        Some("无匹配标签页".into())
    }

    fn match_count(&self) -> usize {
        self.matches.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) {
        self.selected_index = ix;

        // 非活动面板模式：实时预览选中项
        if !self.open_in_active_pane {
            let Some(selected_match) = self.matches.get(self.selected_index()) else {
                return;
            };
            selected_match
                .pane
                .update(cx, |pane, cx| {
                    if let Some(index) = pane.index_for_item(selected_match.item.as_ref()) {
                        pane.activate_item(index, false, false, window, cx);
                    }
                })
                .ok();
        }
        cx.notify();
    }

    fn separators_after_indices(&self) -> Vec<usize> {
        Vec::new()
    }

    fn update_matches(
        &mut self,
        raw_query: String,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        self.update_matches(raw_query, window, cx);
        Task::ready(())
    }

    fn confirm(
        &mut self,
        _secondary: bool,
        window: &mut Window,
        cx: &mut Context<Picker<TabSwitcherDelegate>>,
    ) {
        let Some(selected_match) = self.matches.get(self.selected_index()).cloned() else {
            return;
        };

        // 恢复原始激活项状态
        self.restored_items = true;
        for (pane, index) in self.original_items.iter() {
            pane.update(cx, |this, cx| {
                this.activate_item(*index, false, false, window, cx);
            })
        }

        // 打开选中项
        if self.open_in_active_pane {
            self.confirm_open_in_active_pane(selected_match, window, cx);
        } else {
            selected_match
                .pane
                .update(cx, |pane, cx| {
                    if let Some(index) = pane.index_for_item(selected_match.item.as_ref()) {
                        pane.activate_item(index, true, true, window, cx);
                    }
                })
                .ok();
        }
    }

    fn dismissed(&mut self, window: &mut Window, cx: &mut Context<Picker<TabSwitcherDelegate>>) {
        // 取消时恢复原始激活项
        if !self.restored_items {
            for (pane, index) in self.original_items.iter() {
                pane.update(cx, |this, cx| {
                    this.activate_item(*index, false, false, window, cx);
                })
            }
        }

        // 发送关闭事件
        self.tab_switcher
            .update(cx, |_, cx| cx.emit(DismissEvent))
            .log_err();
    }

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let tab_match = self.matches.get(ix)?;

        // 渲染标签内容
        let params = TabContentParams {
            detail: Some(tab_match.detail),
            selected: true,
            preview: tab_match.preview,
            deemphasized: false,
        };
        let label = tab_match.item.tab_content(params, window, cx);

        let icon = tab_match.icon(&self.project, selected, window, cx);

        // 渲染状态指示器
        let indicator = render_item_indicator(tab_match.item.boxed_clone(), cx);
        let indicator_color = if let Some(ref indicator) = indicator {
            indicator.color
        } else {
            Color::default()
        };
        let indicator = h_flex()
            .flex_shrink_0()
            .children(indicator)
            .child(div().w_2())
            .into_any_element();

        // 关闭按钮
        let close_button = div()
            .id("close-button")
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(move |picker, _: &MouseUpEvent, window, cx| {
                    cx.stop_propagation();
                    picker.delegate.close_item_at(ix, window, cx);
                }),
            )
            .child(
                IconButton::new("close_tab", IconName::Close)
                    .icon_size(IconSize::Small)
                    .icon_color(indicator_color)
                    .tooltip(Tooltip::for_action_title("关闭", &CloseSelectedItem))
                    .on_click(cx.listener(move |picker, _, window, cx| {
                        cx.stop_propagation();
                        picker.delegate.close_item_at(ix, window, cx);
                    })),
            )
            .into_any_element();

        Some(
            ListItem::new(ix)
                .spacing(ListItemSpacing::Sparse)
                .inset(true)
                .toggle_state(selected)
                .child(h_flex().w_full().child(label))
                .start_slot::<DecoratedIcon>(icon)
                // 选中项直接显示关闭按钮，未选中项hover显示
                .map(|el| {
                    if self.selected_index == ix {
                        el.end_slot::<AnyElement>(close_button)
                    } else {
                        el.end_slot::<AnyElement>(indicator)
                            .end_slot_on_hover::<AnyElement>(close_button)
                    }
                }),
        )
    }
}