use std::sync::Arc;

use crate::TaskContexts;
use editor::Editor;
use fuzzy::{StringMatch, StringMatchCandidate};
use gpui::{
    Action, AnyElement, App, AppContext as _, Context, DismissEvent, Entity, EventEmitter,
    Focusable, InteractiveElement, ParentElement, Render, Styled, Subscription, Task, WeakEntity,
    Window, rems,
};
use itertools::Itertools;
use picker::{Picker, PickerDelegate, highlighted_match_with_paths::HighlightedMatch};
use project::{TaskSourceKind, task_store::TaskStore};
use task::{DebugScenario, ResolvedTask, RevealTarget, TaskContext, TaskTemplate};
use ui::{
    ActiveTheme, Clickable, FluentBuilder as _, IconButtonShape, IconWithIndicator, Indicator,
    IntoElement, KeyBinding, ListItem, ListItemSpacing, RenderOnce, Toggleable, Tooltip, div,
    prelude::*,
};

use util::{ResultExt, truncate_and_trailoff};
use workspace::{ModalView, Workspace};
pub use zed_actions::{Rerun, Spawn};

/// 用于创建新任务的模态框
pub struct TasksModalDelegate {
    task_store: Entity<TaskStore>,
    candidates: Option<Vec<(TaskSourceKind, ResolvedTask)>>,
    task_overrides: Option<TaskOverrides>,
    last_used_candidate_index: Option<usize>,
    divider_index: Option<usize>,
    matches: Vec<StringMatch>,
    selected_index: usize,
    workspace: WeakEntity<Workspace>,
    prompt: String,
    task_contexts: Arc<TaskContexts>,
    placeholder_text: Arc<str>,
}

/// 解析上下文前对任务模板的修改配置
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskOverrides {
    /// 参见 [`RevealTarget`]
    pub reveal_target: Option<RevealTarget>,
}

impl TasksModalDelegate {
    fn new(
        task_store: Entity<TaskStore>,
        task_contexts: Arc<TaskContexts>,
        task_overrides: Option<TaskOverrides>,
        workspace: WeakEntity<Workspace>,
    ) -> Self {
        let placeholder_text = if let Some(TaskOverrides {
            reveal_target: Some(RevealTarget::Center),
        }) = &task_overrides
        {
            Arc::from("查找任务，或在中央面板运行命令")
        } else {
            Arc::from("查找任务，或运行命令")
        };
        Self {
            task_store,
            workspace,
            candidates: None,
            matches: Vec::new(),
            last_used_candidate_index: None,
            divider_index: None,
            selected_index: 0,
            prompt: String::default(),
            task_contexts,
            task_overrides,
            placeholder_text,
        }
    }

    /// 创建一次性命令任务
    fn spawn_oneshot(&mut self) -> Option<(TaskSourceKind, ResolvedTask)> {
        if self.prompt.trim().is_empty() {
            return None;
        }

        let default_context = TaskContext::default();
        let active_context = self
            .task_contexts
            .active_context()
            .unwrap_or(&default_context);
        let source_kind = TaskSourceKind::UserInput;
        let id_base = source_kind.to_id_base();
        let mut new_oneshot = TaskTemplate {
            label: self.prompt.clone(),
            command: self.prompt.clone(),
            ..TaskTemplate::default()
        };
        if let Some(TaskOverrides {
            reveal_target: Some(reveal_target),
        }) = &self.task_overrides
        {
            new_oneshot.reveal_target = *reveal_target;
        }
        Some((
            source_kind,
            new_oneshot.resolve_task(&id_base, active_context)?,
        ))
    }

    /// 删除最近使用的任务
    fn delete_previously_used(&mut self, ix: usize, cx: &mut App) {
        let Some(candidates) = self.candidates.as_mut() else {
            return;
        };
        let Some(task) = candidates.get(ix).map(|(_, task)| task.clone()) else {
            return;
        };
        // 手动移除候选项而非重新查询，避免性能损耗
        candidates.remove(ix);
        if let Some(inventory) = self.task_store.read(cx).task_inventory().cloned() {
            inventory.update(cx, |inventory, _| {
                inventory.delete_previously_used(&task.id);
            })
        };
    }
}

/// 任务选择模态框
pub struct TasksModal {
    pub picker: Entity<Picker<TasksModalDelegate>>,
    _subscriptions: [Subscription; 2],
}

impl TasksModal {
    pub fn new(
        task_store: Entity<TaskStore>,
        task_contexts: Arc<TaskContexts>,
        task_overrides: Option<TaskOverrides>,
        is_modal: bool,
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let picker = cx.new(|cx| {
            Picker::uniform_list(
                TasksModalDelegate::new(
                    task_store.clone(),
                    task_contexts,
                    task_overrides,
                    workspace.clone(),
                ),
                window,
                cx,
            )
            .modal(is_modal)
        });
        let mut _subscriptions = [
            cx.subscribe(&picker, |_, _, _: &DismissEvent, cx| {
                cx.emit(DismissEvent);
            }),
            cx.subscribe(&picker, |_, _, event: &ShowAttachModal, cx| {
                cx.emit(ShowAttachModal {
                    debug_config: event.debug_config.clone(),
                });
            }),
        ];

        Self {
            picker,
            _subscriptions,
        }
    }

    /// 任务加载完成，更新候选列表
    pub fn tasks_loaded(
        &mut self,
        task_contexts: Arc<TaskContexts>,
        lsp_tasks: Vec<(TaskSourceKind, task::ResolvedTask)>,
        used_tasks: Vec<(TaskSourceKind, task::ResolvedTask)>,
        current_resolved_tasks: Vec<(TaskSourceKind, task::ResolvedTask)>,
        add_current_language_tasks: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let last_used_candidate_index = if used_tasks.is_empty() {
            None
        } else {
            Some(used_tasks.len() - 1)
        };
        let mut new_candidates = used_tasks;
        new_candidates.extend(lsp_tasks);
        // 待办：无论prefer_lsp是否为false，此处始终添加LSP任务
        // 应将过滤逻辑移至new_candidates并补充测试
        new_candidates.extend(current_resolved_tasks.into_iter().filter(|(task_kind, _)| {
            match task_kind {
                TaskSourceKind::Language { .. } => add_current_language_tasks,
                _ => true,
            }
        }));
        self.picker.update(cx, |picker, cx| {
            picker.delegate.task_contexts = task_contexts;
            picker.delegate.last_used_candidate_index = last_used_candidate_index;
            picker.delegate.candidates = Some(new_candidates);
            picker.refresh(window, cx);
            cx.notify();
        })
    }
}

impl Render for TasksModal {
    fn render(
        &mut self,
        _window: &mut Window,
        _: &mut Context<Self>,
    ) -> impl gpui::prelude::IntoElement {
        v_flex()
            .key_context("TasksModal")
            .w(rems(34.))
            .child(self.picker.clone())
    }
}

/// 显示附加调试模态框事件
pub struct ShowAttachModal {
    pub debug_config: DebugScenario,
}

impl EventEmitter<DismissEvent> for TasksModal {}
impl EventEmitter<ShowAttachModal> for TasksModal {}
impl EventEmitter<ShowAttachModal> for Picker<TasksModalDelegate> {}

impl Focusable for TasksModal {
    fn focus_handle(&self, cx: &gpui::App) -> gpui::FocusHandle {
        self.picker.read(cx).focus_handle(cx)
    }
}

impl ModalView for TasksModal {}

/// 标签行最大长度
const MAX_TAGS_LINE_LEN: usize = 30;

impl PickerDelegate for TasksModalDelegate {
    type ListItem = ListItem;

    fn match_count(&self) -> usize {
        self.matches.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(
        &mut self,
        ix: usize,
        _window: &mut Window,
        _cx: &mut Context<picker::Picker<Self>>,
    ) {
        self.selected_index = ix;
    }

    fn placeholder_text(&self, _window: &mut Window, _: &mut App) -> Arc<str> {
        self.placeholder_text.clone()
    }

    /// 根据搜索词更新匹配结果
    fn update_matches(
        &mut self,
        query: String,
        window: &mut Window,
        cx: &mut Context<picker::Picker<Self>>,
    ) -> Task<()> {
        let candidates = match &self.candidates {
            Some(candidates) => Task::ready(string_match_candidates(candidates)),
            None => {
                if let Some(task_inventory) = self.task_store.read(cx).task_inventory().cloned() {
                    let task_list = task_inventory.update(cx, |this, cx| {
                        this.used_and_current_resolved_tasks(self.task_contexts.clone(), cx)
                    });
                    let workspace = self.workspace.clone();
                    let lsp_task_sources = self.task_contexts.lsp_task_sources.clone();
                    let task_position = self.task_contexts.latest_selection;
                    cx.spawn(async move |picker, cx| {
                        let (used, current) = task_list.await;
                        let Ok((lsp_tasks, prefer_lsp)) = workspace.update(cx, |workspace, cx| {
                            let lsp_tasks = editor::lsp_tasks(
                                workspace.project().clone(),
                                &lsp_task_sources,
                                task_position,
                                cx,
                            );
                            let prefer_lsp = workspace
                                .active_item(cx)
                                .and_then(|item| item.downcast::<Editor>())
                                .map(|editor| {
                                    editor
                                        .read(cx)
                                        .buffer()
                                        .read(cx)
                                        .language_settings(cx)
                                        .tasks
                                        .prefer_lsp
                                })
                                .unwrap_or(false);
                            (lsp_tasks, prefer_lsp)
                        }) else {
                            return Vec::new();
                        };

                        let lsp_tasks = lsp_tasks.await;
                        picker
                            .update(cx, |picker, _| {
                                picker.delegate.last_used_candidate_index = if used.is_empty() {
                                    None
                                } else {
                                    Some(used.len() - 1)
                                };

                                let mut new_candidates = used;
                                let add_current_language_tasks =
                                    !prefer_lsp || lsp_tasks.is_empty();
                                new_candidates.extend(lsp_tasks.into_iter().flat_map(
                                    |(kind, tasks_with_locations)| {
                                        tasks_with_locations
                                            .into_iter()
                                            .sorted_by_key(|(location, task)| {
                                                (location.is_none(), task.resolved_label.clone())
                                            })
                                            .map(move |(_, task)| (kind.clone(), task))
                                    },
                                ));
                                // 待办：无论prefer_lsp是否为false，此处始终添加LSP任务
                                // 应将过滤逻辑移至new_candidates并补充测试
                                new_candidates.extend(current.into_iter().filter(
                                    |(task_kind, _)| {
                                        add_current_language_tasks
                                            || !matches!(task_kind, TaskSourceKind::Language { .. })
                                    },
                                ));
                                let match_candidates = string_match_candidates(&new_candidates);
                                let _ = picker.delegate.candidates.insert(new_candidates);
                                match_candidates
                            })
                            .ok()
                            .unwrap_or_default()
                    })
                } else {
                    Task::ready(Vec::new())
                }
            }
        };

        cx.spawn_in(window, async move |picker, cx| {
            let candidates = candidates.await;
            let matches = fuzzy::match_strings(
                &candidates,
                &query,
                true,
                true,
                1000,
                &Default::default(),
                cx.background_executor().clone(),
            )
            .await;
            picker
                .update(cx, |picker, _| {
                    let delegate = &mut picker.delegate;
                    delegate.matches = matches;
                    if let Some(index) = delegate.last_used_candidate_index {
                        delegate.matches.sort_by_key(|m| m.candidate_id > index);
                    }

                    delegate.prompt = query;
                    delegate.divider_index = delegate.last_used_candidate_index.and_then(|index| {
                        let index = delegate
                            .matches
                            .partition_point(|matching_task| matching_task.candidate_id <= index);
                        Some(index).and_then(|index| (index != 0).then(|| index - 1))
                    });

                    if delegate.matches.is_empty() {
                        delegate.selected_index = 0;
                    } else {
                        delegate.selected_index =
                            delegate.selected_index.min(delegate.matches.len() - 1);
                    }
                })
                .log_err();
        })
    }

    /// 确认选择并执行任务
    fn confirm(
        &mut self,
        omit_history_entry: bool,
        window: &mut Window,
        cx: &mut Context<picker::Picker<Self>>,
    ) {
        let current_match_index = self.selected_index();
        let task = self
            .matches
            .get(current_match_index)
            .and_then(|current_match| {
                let ix = current_match.candidate_id;
                self.candidates
                    .as_ref()
                    .map(|candidates| candidates[ix].clone())
            });
        let Some((task_source_kind, mut task)) = task else {
            return;
        };
        if let Some(TaskOverrides {
            reveal_target: Some(reveal_target),
        }) = &self.task_overrides
        {
            task.resolved.reveal_target = *reveal_target;
        }

        self.workspace
            .update(cx, |workspace, cx| {
                workspace.schedule_resolved_task(
                    task_source_kind,
                    task,
                    omit_history_entry,
                    window,
                    cx,
                );
            })
            .ok();

        cx.emit(DismissEvent);
    }

    fn dismissed(&mut self, _window: &mut Window, cx: &mut Context<picker::Picker<Self>>) {
        cx.emit(DismissEvent);
    }

    /// 渲染单个匹配项
    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        window: &mut Window,
        cx: &mut Context<picker::Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let candidates = self.candidates.as_ref()?;
        let hit = &self.matches.get(ix)?;
        let (source_kind, resolved_task) = &candidates.get(hit.candidate_id)?;
        let template = resolved_task.original_task();
        let display_label = resolved_task.display_label();

        let mut tooltip_label_text =
            if display_label != &template.label || source_kind == &TaskSourceKind::UserInput {
                resolved_task.resolved_label.clone()
            } else {
                String::new()
            };

        if resolved_task.resolved.command_label != resolved_task.resolved_label {
            if !tooltip_label_text.trim().is_empty() {
                tooltip_label_text.push('\n');
            }
            tooltip_label_text.push_str(&resolved_task.resolved.command_label);
        }

        if !template.tags.is_empty() {
            tooltip_label_text.push('\n');
            tooltip_label_text.push_str(
                template
                    .tags
                    .iter()
                    .map(|tag| format!("\n#{}", tag))
                    .collect::<Vec<_>>()
                    .join("")
                    .as_str(),
            );
        }
        let tooltip_label = if tooltip_label_text.trim().is_empty() {
            None
        } else {
            Some(Tooltip::simple(tooltip_label_text, cx))
        };

        let highlighted_location = HighlightedMatch {
            text: hit.string.clone(),
            highlight_positions: hit.positions.clone(),
            color: Color::Default,
        };
        let icon = match source_kind {
            TaskSourceKind::UserInput => Some(Icon::new(IconName::Terminal)),
            TaskSourceKind::AbsPath { .. } => Some(Icon::new(IconName::Settings)),
            TaskSourceKind::Worktree { .. } => Some(Icon::new(IconName::FileTree)),
            TaskSourceKind::Lsp {
                language_name: name,
                ..
            }
            | TaskSourceKind::Language { name, .. } => file_icons::FileIcons::get(cx)
                .get_icon_for_type(&name.to_lowercase(), cx)
                .map(Icon::from_path),
        }
        .map(|icon| icon.color(Color::Muted).size(IconSize::Small));
        let indicator = if matches!(source_kind, TaskSourceKind::Lsp { .. }) {
            Some(Indicator::icon(
                Icon::new(IconName::BoltOutlined).size(IconSize::Small),
            ))
        } else {
            None
        };
        let icon = icon.map(|icon| {
            IconWithIndicator::new(icon, indicator)
                .indicator_border_color(Some(cx.theme().colors().border_transparent))
        });
        let history_run_icon = if Some(ix) <= self.divider_index {
            Some(
                Icon::new(IconName::HistoryRerun)
                    .color(Color::Muted)
                    .size(IconSize::Small)
                    .into_any_element(),
            )
        } else {
            Some(
                v_flex()
                    .flex_none()
                    .size(IconSize::Small.rems())
                    .into_any_element(),
            )
        };

        Some(
            ListItem::new(format!("tasks-modal-{ix}"))
                .inset(true)
                .start_slot::<IconWithIndicator>(icon)
                .end_slot::<AnyElement>(
                    h_flex()
                        .gap_1()
                        .child(Label::new(truncate_and_trailoff(
                            &template
                                .tags
                                .iter()
                                .map(|tag| format!("#{}", tag))
                                .collect::<Vec<_>>()
                                .join(" "),
                            MAX_TAGS_LINE_LEN,
                        )))
                        .flex_none()
                        .child(history_run_icon.unwrap())
                        .into_any_element(),
                )
                .spacing(ListItemSpacing::Sparse)
                .when_some(tooltip_label, |list_item, item_label| {
                    list_item.tooltip(move |_, _| item_label.clone())
                })
                .map(|item| {
                    if matches!(source_kind, TaskSourceKind::UserInput)
                        || Some(ix) <= self.divider_index
                    {
                        let task_index = hit.candidate_id;
                        let delete_button = div().child(
                            IconButton::new("delete", IconName::Close)
                                .shape(IconButtonShape::Square)
                                .icon_color(Color::Muted)
                                .size(ButtonSize::None)
                                .icon_size(IconSize::XSmall)
                                .on_click(cx.listener(move |picker, _event, window, cx| {
                                    cx.stop_propagation();
                                    window.prevent_default();

                                    picker.delegate.delete_previously_used(task_index, cx);
                                    picker.delegate.last_used_candidate_index = picker
                                        .delegate
                                        .last_used_candidate_index
                                        .unwrap_or(0)
                                        .checked_sub(1);
                                    picker.refresh(window, cx);
                                }))
                                .tooltip(|_, cx| Tooltip::simple("从最近任务删除", cx)),
                        );
                        item.end_slot_on_hover(delete_button)
                    } else {
                        item
                    }
                })
                .toggle_state(selected)
                .child(highlighted_location.render(window, cx)),
        )
    }

    /// 确认补全
    fn confirm_completion(
        &mut self,
        _: String,
        _window: &mut Window,
        _: &mut Context<Picker<Self>>,
    ) -> Option<String> {
        let task_index = self.matches.get(self.selected_index())?.candidate_id;
        let tasks = self.candidates.as_ref()?;
        let (_, task) = tasks.get(task_index)?;
        Some(task.resolved.command_label.clone())
    }

    /// 确认输入的自定义命令
    fn confirm_input(
        &mut self,
        omit_history_entry: bool,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) {
        let Some((task_source_kind, mut task)) = self.spawn_oneshot() else {
            return;
        };

        if let Some(TaskOverrides {
            reveal_target: Some(reveal_target),
        }) = &self.task_overrides
        {
            task.resolved.reveal_target = *reveal_target;
        }
        self.workspace
            .update(cx, |workspace, cx| {
                workspace.schedule_resolved_task(
                    task_source_kind,
                    task,
                    omit_history_entry,
                    window,
                    cx,
                )
            })
            .ok();
        cx.emit(DismissEvent);
    }

    /// 分隔线位置
    fn separators_after_indices(&self) -> Vec<usize> {
        if let Some(i) = self.divider_index {
            vec![i]
        } else {
            Vec::new()
        }
    }

    /// 渲染底部操作栏
    fn render_footer(
        &self,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<gpui::AnyElement> {
        let is_recent_selected = self.divider_index >= Some(self.selected_index);
        let current_modifiers = window.modifiers();
        let left_button = if self
            .task_store
            .read(cx)
            .task_inventory()?
            .read(cx)
            .last_scheduled_task(None)
            .is_some()
        {
            Some(("重新运行上一个任务", Rerun::default().boxed_clone()))
        } else {
            None
        };
        Some(
            h_flex()
                .w_full()
                .p_1p5()
                .justify_between()
                .border_t_1()
                .border_color(cx.theme().colors().border_variant)
                .child(
                    left_button
                        .map(|(label, action)| {
                            let keybind = KeyBinding::for_action(&*action, cx);

                            Button::new("edit-current-task", label)
                                .key_binding(keybind)
                                .on_click(move |_, window, cx| {
                                    window.dispatch_action(action.boxed_clone(), cx);
                                })
                                .into_any_element()
                        })
                        .unwrap_or_else(|| h_flex().into_any_element()),
                )
                .map(|this| {
                    if (current_modifiers.alt || self.matches.is_empty()) && !self.prompt.is_empty()
                    {
                        let action = picker::ConfirmInput {
                            secondary: current_modifiers.secondary(),
                        }
                        .boxed_clone();
                        this.child({
                            let spawn_oneshot_label = if current_modifiers.secondary() {
                                "运行一次性命令（不记录）"
                            } else {
                                "运行一次性命令"
                            };

                            Button::new("spawn-onehshot", spawn_oneshot_label)
                                .key_binding(KeyBinding::for_action(&*action, cx))
                                .on_click(move |_, window, cx| {
                                    window.dispatch_action(action.boxed_clone(), cx)
                                })
                        })
                    } else if current_modifiers.secondary() {
                        this.child({
                            let label = if is_recent_selected {
                                "重新运行（不记录）"
                            } else {
                                "运行（不记录）"
                            };
                            Button::new("spawn", label)
                                .key_binding(KeyBinding::for_action(&menu::SecondaryConfirm, cx))
                                .on_click(move |_, window, cx| {
                                    window.dispatch_action(menu::SecondaryConfirm.boxed_clone(), cx)
                                })
                        })
                    } else {
                        this.child({
                            let run_entry_label =
                                if is_recent_selected { "重新运行" } else { "运行" };

                            Button::new("spawn", run_entry_label)
                                .key_binding(KeyBinding::for_action(&menu::Confirm, cx))
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(menu::Confirm.boxed_clone(), cx);
                                })
                        })
                    }
                })
                .into_any_element(),
        )
    }
}

/// 生成字符串匹配候选项
fn string_match_candidates<'a>(
    candidates: impl IntoIterator<Item = &'a (TaskSourceKind, ResolvedTask)> + 'a,
) -> Vec<StringMatchCandidate> {
    candidates
        .into_iter()
        .enumerate()
        .map(|(index, (_, candidate))| StringMatchCandidate::new(index, candidate.display_label()))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc};

    use editor::{Editor, SelectionEffects};
    use gpui::{TestAppContext, VisualTestContext};
    use language::{Language, LanguageConfig, LanguageMatcher, Point};
    use project::{ContextProviderWithTasks, FakeFs, Project};
    use serde_json::json;
    use task::TaskTemplates;
    use util::path;
    use workspace::{CloseInactiveTabsAndPanes, MultiWorkspace, OpenOptions, OpenVisible};

    use crate::{modal::Spawn, tests::init_test};

    use super::*;

    #[gpui::test]
    async fn test_spawn_tasks_modal_query_reuse(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        fs.insert_tree(
            path!("/dir"),
            json!({
                ".zed": {
                    "tasks.json": r#"[
                        {
                            "label": "example task",
                            "command": "echo",
                            "args": ["4"]
                        },
                        {
                            "label": "another one",
                            "command": "echo",
                            "args": ["55"]
                        },
                    ]"#,
                },
                "a.ts": "a"
            }),
        )
        .await;

        let project = Project::test(fs, [path!("/dir").as_ref()], cx).await;
        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
        let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());

        let tasks_picker = open_spawn_tasks(&workspace, cx);
        assert_eq!(
            query(&tasks_picker, cx),
            "",
            "初始搜索词应为空"
        );
        assert_eq!(
            task_names(&tasks_picker, cx),
            vec!["another one", "example task"],
            "无全局任务且未打开文件时，应使用单个工作树并列出其任务"
        );
        drop(tasks_picker);

        let _ = workspace
            .update_in(cx, |workspace, window, cx| {
                workspace.open_abs_path(
                    PathBuf::from(path!("/dir/a.ts")),
                    OpenOptions {
                        visible: Some(OpenVisible::All),
                        ..Default::default()
                    },
                    window,
                    cx,
                )
            })
            .await
            .unwrap();
        let tasks_picker = open_spawn_tasks(&workspace, cx);
        assert_eq!(
            task_names(&tasks_picker, cx),
            vec!["another one", "example task"],
            "初始任务应按字母顺序排列"
        );

        let query_str = "tas";
        cx.simulate_input(query_str);
        assert_eq!(query(&tasks_picker, cx), query_str);
        assert_eq!(
            task_names(&tasks_picker, cx),
            vec!["example task"],
            "仅一个任务应匹配搜索词 {query_str}"
        );

        cx.dispatch_action(picker::ConfirmCompletion);
        assert_eq!(
            query(&tasks_picker, cx),
            "echo 4",
            "搜索词应设置为选中任务的命令"
        );
        assert_eq!(
            task_names(&tasks_picker, cx),
            Vec::<String>::new(),
            "不应列出任何任务"
        );
        cx.dispatch_action(picker::ConfirmInput { secondary: false });

        let tasks_picker = open_spawn_tasks(&workspace, cx);
        assert_eq!(
            query(&tasks_picker, cx),
            "",
            "确认后搜索词应重置"
        );
        assert_eq!(
            task_names(&tasks_picker, cx),
            vec!["echo 4", "another one", "example task"],
            "新的一次性任务应优先显示"
        );

        let query_str = "echo 4";
        cx.simulate_input(query_str);
        assert_eq!(query(&tasks_picker, cx), query_str);
        assert_eq!(
            task_names(&tasks_picker, cx),
            vec!["echo 4"],
            "新的一次性任务应匹配自定义命令搜索"
        );

        cx.dispatch_action(picker::ConfirmInput { secondary: false });
        let tasks_picker = open_spawn_tasks(&workspace, cx);
        assert_eq!(
            query(&tasks_picker, cx),
            "",
            "确认后搜索词应重置"
        );
        assert_eq!(
            task_names(&tasks_picker, cx),
            vec![query_str, "another one", "example task"],
            "最近使用的任务应优先显示"
        );

        cx.dispatch_action(picker::ConfirmCompletion);
        assert_eq!(
            query(&tasks_picker, cx),
            query_str,
            "搜索词应设置为自定义任务名称"
        );
        assert_eq!(
            task_names(&tasks_picker, cx),
            vec![query_str],
            "仅应显示自定义任务"
        );

        let query_str = "0";
        cx.simulate_input(query_str);
        assert_eq!(query(&tasks_picker, cx), "echo 40");
        assert_eq!(
            task_names(&tasks_picker, cx),
            Vec::<String>::new(),
            "新的一次性任务不应匹配任何命令搜索"
        );

        cx.dispatch_action(picker::ConfirmInput { secondary: true });
        let tasks_picker = open_spawn_tasks(&workspace, cx);
        assert_eq!(
            query(&tasks_picker, cx),
            "",
            "确认后搜索词应重置"
        );
        assert_eq!(
            task_names(&tasks_picker, cx),
            vec!["echo 4", "another one", "example task"],
            "不应添加搜索词到列表，因为使用了不记录历史的操作"
        );

        cx.dispatch_action(Spawn::ByName {
            task_name: "example task".to_string(),
            reveal_target: None,
        });
        let tasks_picker = workspace.update(cx, |workspace, cx| {
            workspace
                .active_modal::<TasksModal>(cx)
                .unwrap()
                .read(cx)
                .picker
                .clone()
        });
        assert_eq!(
            task_names(&tasks_picker, cx),
            vec!["echo 4", "another one", "example task"],
        );
    }

    #[gpui::test]
    async fn test_basic_context_for_simple_files(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        fs.insert_tree(
            path!("/dir"),
            json!({
                ".zed": {
                    "tasks.json": r#"[
                        {
                            "label": "hello from $ZED_FILE:$ZED_ROW:$ZED_COLUMN",
                            "command": "echo",
                            "args": ["hello", "from", "$ZED_FILE", ":", "$ZED_ROW", ":", "$ZED_COLUMN"]
                        },
                        {
                            "label": "opened now: $ZED_WORKTREE_ROOT",
                            "command": "echo",
                            "args": ["opened", "now:", "$ZED_WORKTREE_ROOT"]
                        }
                    ]"#,
                },
                "file_without_extension": "aaaaaaaaaaaaaaaaaaaa\naaaaaaaaaaaaaaaaaa",
                "file_with.odd_extension": "b",
            }),
        )
        .await;

        let project = Project::test(fs, [path!("/dir").as_ref()], cx).await;
        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
        let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());

        let tasks_picker = open_spawn_tasks(&workspace, cx);
        assert_eq!(
            task_names(&tasks_picker, cx),
            vec![concat!("opened now: ", path!("/dir")).to_string()],
            "未打开文件时，单个工作树应自动检测所有相关任务"
        );
        tasks_picker.update(cx, |_, cx| {
            cx.emit(DismissEvent);
        });
        drop(tasks_picker);
        cx.executor().run_until_parked();

        let _ = workspace
            .update_in(cx, |workspace, window, cx| {
                workspace.open_abs_path(
                    PathBuf::from(path!("/dir/file_with.odd_extension")),
                    OpenOptions {
                        visible: Some(OpenVisible::All),
                        ..Default::default()
                    },
                    window,
                    cx,
                )
            })
            .await
            .unwrap();
        cx.executor().run_until_parked();
        let tasks_picker = open_spawn_tasks(&workspace, cx);
        assert_eq!(
            task_names(&tasks_picker, cx),
            vec![
                concat!("hello from ", path!("/dir/file_with.odd_extension:1:1")).to_string(),
                concat!("opened now: ", path!("/dir")).to_string(),
            ],
            "第二个打开的缓冲区应填充上下文，标签过长时应截断"
        );
        tasks_picker.update(cx, |_, cx| {
            cx.emit(DismissEvent);
        });
        drop(tasks_picker);
        cx.executor().run_until_parked();

        let second_item = workspace
            .update_in(cx, |workspace, window, cx| {
                workspace.open_abs_path(
                    PathBuf::from(path!("/dir/file_without_extension")),
                    OpenOptions {
                        visible: Some(OpenVisible::All),
                        ..Default::default()
                    },
                    window,
                    cx,
                )
            })
            .await
            .unwrap();

        let editor = cx
            .update(|_window, cx| second_item.act_as::<Editor>(cx))
            .unwrap();
        editor.update_in(cx, |editor, window, cx| {
            editor.change_selections(SelectionEffects::no_scroll(), window, cx, |s| {
                s.select_ranges(Some(Point::new(1, 2)..Point::new(1, 5)))
            })
        });
        cx.executor().run_until_parked();
        let tasks_picker = open_spawn_tasks(&workspace, cx);
        assert_eq!(
            task_names(&tasks_picker, cx),
            vec![
                concat!("hello from ", path!("/dir/file_without_extension:2:3")).to_string(),
                concat!("opened now: ", path!("/dir")).to_string(),
            ],
            "打开的缓冲区应填充上下文，标签过长时应截断"
        );
        tasks_picker.update(cx, |_, cx| {
            cx.emit(DismissEvent);
        });
        drop(tasks_picker);
        cx.executor().run_until_parked();
    }

    #[gpui::test]
    async fn test_language_task_filtering(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        fs.insert_tree(
            path!("/dir"),
            json!({
                "a1.ts": "// a1",
                "a2.ts": "// a2",
                "b.rs": "// b",
            }),
        )
        .await;

        let project = Project::test(fs, [path!("/dir").as_ref()], cx).await;
        project.read_with(cx, |project, _| {
            let language_registry = project.languages();
            language_registry.add(Arc::new(
                Language::new(
                    LanguageConfig {
                        name: "TypeScript".into(),
                        matcher: LanguageMatcher {
                            path_suffixes: vec!["ts".to_string()],
                            ..LanguageMatcher::default()
                        },
                        ..LanguageConfig::default()
                    },
                    None,
                )
                .with_context_provider(Some(Arc::new(
                    ContextProviderWithTasks::new(TaskTemplates(vec![
                        TaskTemplate {
                            label: "Task without variables".to_string(),
                            command: "npm run clean".to_string(),
                            ..TaskTemplate::default()
                        },
                        TaskTemplate {
                            label: "TypeScript task from file $ZED_FILE".to_string(),
                            command: "npm run build".to_string(),
                            ..TaskTemplate::default()
                        },
                        TaskTemplate {
                            label: "Another task from file $ZED_FILE".to_string(),
                            command: "npm run lint".to_string(),
                            ..TaskTemplate::default()
                        },
                    ])),
                ))),
            ));
            language_registry.add(Arc::new(
                Language::new(
                    LanguageConfig {
                        name: "Rust".into(),
                        matcher: LanguageMatcher {
                            path_suffixes: vec!["rs".to_string()],
                            ..LanguageMatcher::default()
                        },
                        ..LanguageConfig::default()
                    },
                    None,
                )
                .with_context_provider(Some(Arc::new(
                    ContextProviderWithTasks::new(TaskTemplates(vec![TaskTemplate {
                        label: "Rust task".to_string(),
                        command: "cargo check".into(),
                        ..TaskTemplate::default()
                    }])),
                ))),
            ));
        });
        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
        let workspace = multi_workspace.read_with(cx, |mw, _| mw.workspace().clone());

        let _ts_file_1 = workspace
            .update_in(cx, |workspace, window, cx| {
                workspace.open_abs_path(
                    PathBuf::from(path!("/dir/a1.ts")),
                    OpenOptions {
                        visible: Some(OpenVisible::All),
                        ..Default::default()
                    },
                    window,
                    cx,
                )
            })
            .await
            .unwrap();
        let tasks_picker = open_spawn_tasks(&workspace, cx);
        assert_eq!(
            task_names(&tasks_picker, cx),
            vec![
                concat!("Another task from file ", path!("/dir/a1.ts")),
                concat!("TypeScript task from file ", path!("/dir/a1.ts")),
                "Task without variables",
            ],
            "应为打开的文件显示TypeScript任务，带变量的任务优先，所有组按字母数字排序"
        );

        emulate_task_schedule(
            tasks_picker,
            &project,
            concat!("TypeScript task from file ", path!("/dir/a1.ts")),
            cx,
        );

        let tasks_picker = open_spawn_tasks(&workspace, cx);
        assert_eq!(
            task_names(&tasks_picker, cx),
            vec![
                concat!("TypeScript task from file ", path!("/dir/a1.ts")),
                concat!("Another task from file ", path!("/dir/a1.ts")),
                "Task without variables",
            ],
            "运行任务并加入历史后，应作为最近使用项置顶。相同标签和上下文的任务会去重。"
        );
        tasks_picker.update(cx, |_, cx| {
            cx.emit(DismissEvent);
        });
        drop(tasks_picker);
        cx.executor().run_until_parked();

        let _ts_file_2 = workspace
            .update_in(cx, |workspace, window, cx| {
                workspace.open_abs_path(
                    PathBuf::from(path!("/dir/a2.ts")),
                    OpenOptions {
                        visible: Some(OpenVisible::All),
                        ..Default::default()
                    },
                    window,
                    cx,
                )
            })
            .await
            .unwrap();
        let tasks_picker = open_spawn_tasks(&workspace, cx);
        assert_eq!(
            task_names(&tasks_picker, cx),
            vec![
                concat!("TypeScript task from file ", path!("/dir/a1.ts")),
                concat!("Another task from file ", path!("/dir/a2.ts")),
                concat!("TypeScript task from file ", path!("/dir/a2.ts")),
                "Task without variables",
            ],
            "即使两个TS文件都打开，也只应显示历史（置顶）和当前文件解析的任务"
        );
        tasks_picker.update(cx, |_, cx| {
            cx.emit(DismissEvent);
        });
        drop(tasks_picker);
        cx.executor().run_until_parked();

        let _rs_file = workspace
            .update_in(cx, |workspace, window, cx| {
                workspace.open_abs_path(
                    PathBuf::from(path!("/dir/b.rs")),
                    OpenOptions {
                        visible: Some(OpenVisible::All),
                        ..Default::default()
                    },
                    window,
                    cx,
                )
            })
            .await
            .unwrap();
        let tasks_picker = open_spawn_tasks(&workspace, cx);
        assert_eq!(
            task_names(&tasks_picker, cx),
            vec!["Rust task"],
            "即使打开了TS文件并运行过TS任务，也只应显示当前打开文件的语言任务"
        );

        cx.dispatch_action(CloseInactiveTabsAndPanes::default());
        emulate_task_schedule(tasks_picker, &project, "Rust task", cx);
        let _ts_file_2 = workspace
            .update_in(cx, |workspace, window, cx| {
                workspace.open_abs_path(
                    PathBuf::from(path!("/dir/a2.ts")),
                    OpenOptions {
                        visible: Some(OpenVisible::All),
                        ..Default::default()
                    },
                    window,
                    cx,
                )
            })
            .await
            .unwrap();
        let tasks_picker = open_spawn_tasks(&workspace, cx);
        assert_eq!(
            task_names(&tasks_picker, cx),
            vec![
                concat!("TypeScript task from file ", path!("/dir/a1.ts")),
                concat!("Another task from file ", path!("/dir/a2.ts")),
                concat!("TypeScript task from file ", path!("/dir/a2.ts")),
                "Task without variables",
            ],
            "关闭除.rs外的所有标签、运行Rust任务并切回TS后，应恢复之前的TS运行历史"
        );
    }

    /// 模拟任务调度
    fn emulate_task_schedule(
        tasks_picker: Entity<Picker<TasksModalDelegate>>,
        project: &Entity<Project>,
        scheduled_task_label: &str,
        cx: &mut VisualTestContext,
    ) {
        let scheduled_task = tasks_picker.read_with(cx, |tasks_picker, _| {
            tasks_picker
                .delegate
                .candidates
                .iter()
                .flatten()
                .find(|(_, task)| task.resolved_label == scheduled_task_label)
                .cloned()
                .unwrap()
        });
        project.update(cx, |project, cx| {
            if let Some(task_inventory) = project.task_store().read(cx).task_inventory().cloned() {
                task_inventory.update(cx, |inventory, _| {
                    let (kind, task) = scheduled_task;
                    inventory.task_scheduled(kind, task);
                });
            }
        });
        tasks_picker.update(cx, |_, cx| {
            cx.emit(DismissEvent);
        });
        drop(tasks_picker);
        cx.executor().run_until_parked()
    }

    /// 打开任务创建模态框
    fn open_spawn_tasks(
        workspace: &Entity<Workspace>,
        cx: &mut VisualTestContext,
    ) -> Entity<Picker<TasksModalDelegate>> {
        cx.dispatch_action(Spawn::modal());
        workspace.update(cx, |workspace, cx| {
            workspace
                .active_modal::<TasksModal>(cx)
                .expect("执行Spawn操作后未找到任务模态框")
                .read(cx)
                .picker
                .clone()
        })
    }

    /// 获取当前搜索词
    fn query(
        spawn_tasks: &Entity<Picker<TasksModalDelegate>>,
        cx: &mut VisualTestContext,
    ) -> String {
        spawn_tasks.read_with(cx, |spawn_tasks, cx| spawn_tasks.query(cx))
    }

    /// 获取任务名称列表
    fn task_names(
        spawn_tasks: &Entity<Picker<TasksModalDelegate>>,
        cx: &mut VisualTestContext,
    ) -> Vec<String> {
        spawn_tasks.read_with(cx, |spawn_tasks, _| {
            spawn_tasks
                .delegate
                .matches
                .iter()
                .map(|hit| hit.string.clone())
                .collect::<Vec<_>>()
        })
    }
}