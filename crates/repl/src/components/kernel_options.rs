use crate::KERNEL_DOCS_URL;
use crate::kernels::KernelSpecification;
use crate::repl_store::ReplStore;

use gpui::{AnyView, DismissEvent, FontWeight, SharedString, Task};
use picker::{Picker, PickerDelegate};
use project::WorktreeId;
use std::sync::Arc;
use ui::{ListItem, ListItemSpacing, PopoverMenu, PopoverMenuHandle, PopoverTrigger, prelude::*};

/// 选择内核后的回调类型
type OnSelect = Box<dyn Fn(KernelSpecification, &mut Window, &mut App)>;

/// 内核选择器列表项：分组标题 / 内核项
#[derive(Clone)]
pub enum KernelPickerEntry {
    SectionHeader(SharedString),
    Kernel {
        spec: KernelSpecification,
        is_recommended: bool,
    },
}

/// 构建分组后的内核列表（推荐 → Python环境 → Jupyter → WSL → 远程）
fn build_grouped_entries(store: &ReplStore, worktree_id: WorktreeId) -> Vec<KernelPickerEntry> {
    let mut entries = Vec::new();
    let mut recommended_entry: Option<KernelPickerEntry> = None;
    let mut found_selected = false;
    let selected_kernel = store.selected_kernel(worktree_id);

    let mut python_envs = Vec::new();
    let mut jupyter_kernels = Vec::new();
    let mut wsl_kernels = Vec::new();
    let mut remote_kernels = Vec::new();

    for spec in store.kernel_specifications_for_worktree(worktree_id) {
        let is_recommended = store.is_recommended_kernel(worktree_id, spec);
        let is_selected = selected_kernel.map_or(false, |s| s == spec);

        // 优先记录当前选中项 / 推荐项
        if is_selected {
            recommended_entry = Some(KernelPickerEntry::Kernel {
                spec: spec.clone(),
                is_recommended: true,
            });
            found_selected = true;
        } else if is_recommended && !found_selected {
            recommended_entry = Some(KernelPickerEntry::Kernel {
                spec: spec.clone(),
                is_recommended: true,
            });
        }

        // 按类型分组
        match spec {
            KernelSpecification::PythonEnv(_) => {
                python_envs.push(KernelPickerEntry::Kernel {
                    spec: spec.clone(),
                    is_recommended,
                });
            }
            KernelSpecification::Jupyter(_) => {
                jupyter_kernels.push(KernelPickerEntry::Kernel {
                    spec: spec.clone(),
                    is_recommended,
                });
            }
            KernelSpecification::JupyterServer(_) | KernelSpecification::SshRemote(_) => {
                remote_kernels.push(KernelPickerEntry::Kernel {
                    spec: spec.clone(),
                    is_recommended,
                });
            }
            KernelSpecification::WslRemote(_) => {
                wsl_kernels.push(KernelPickerEntry::Kernel {
                    spec: spec.clone(),
                    is_recommended,
                });
            }
        }
    }

    // Python 环境排序：安装 ipykernel 的优先 → 再按名称
    python_envs.sort_by(|a, b| {
        let (spec_a, spec_b) = match (a, b) {
            (
                KernelPickerEntry::Kernel { spec: sa, .. },
                KernelPickerEntry::Kernel { spec: sb, .. },
            ) => (sa, sb),
            _ => return std::cmp::Ordering::Equal,
        };
        spec_b
            .has_ipykernel()
            .cmp(&spec_a.has_ipykernel())
            .then_with(|| spec_a.name().cmp(&spec_b.name()))
    });

    // 推荐分组
    if let Some(rec) = recommended_entry {
        entries.push(KernelPickerEntry::SectionHeader("推荐".into()));
        entries.push(rec);
    }

    // Python 环境
    if !python_envs.is_empty() {
        entries.push(KernelPickerEntry::SectionHeader("Python 环境".into()));
        entries.extend(python_envs);
    }

    // Jupyter 内核
    if !jupyter_kernels.is_empty() {
        entries.push(KernelPickerEntry::SectionHeader("Jupyter 内核".into()));
        entries.extend(jupyter_kernels);
    }

    // WSL 内核
    if !wsl_kernels.is_empty() {
        entries.push(KernelPickerEntry::SectionHeader("WSL 内核".into()));
        entries.extend(wsl_kernels);
    }

    // 远程服务器
    if !remote_kernels.is_empty() {
        entries.push(KernelPickerEntry::SectionHeader("远程服务器".into()));
        entries.extend(remote_kernels);
    }

    entries
}

/// 内核选择器组件（带弹出菜单）
#[derive(IntoElement)]
pub struct KernelSelector<T, TT>
where
    T: PopoverTrigger + ButtonCommon,
    TT: Fn(&mut Window, &mut App) -> AnyView + 'static,
{
    handle: Option<PopoverMenuHandle<Picker<KernelPickerDelegate>>>,
    on_select: OnSelect,
    trigger: T,
    tooltip: TT,
    info_text: Option<SharedString>,
    worktree_id: WorktreeId,
}

/// 内核选择器代理（实现 Picker 列表逻辑）
pub struct KernelPickerDelegate {
    all_entries: Vec<KernelPickerEntry>,
    filtered_entries: Vec<KernelPickerEntry>,
    selected_kernelspec: Option<KernelSpecification>,
    selected_index: usize,
    on_select: OnSelect,
}

impl<T, TT> KernelSelector<T, TT>
where
    T: PopoverTrigger + ButtonCommon,
    TT: Fn(&mut Window, &mut App) -> AnyView + 'static,
{
    /// 创建内核选择器
    pub fn new(on_select: OnSelect, worktree_id: WorktreeId, trigger: T, tooltip: TT) -> Self {
        KernelSelector {
            on_select,
            handle: None,
            trigger,
            tooltip,
            info_text: None,
            worktree_id,
        }
    }

    /// 设置弹出菜单句柄
    pub fn with_handle(mut self, handle: PopoverMenuHandle<Picker<KernelPickerDelegate>>) -> Self {
        self.handle = Some(handle);
        self
    }

    /// 设置提示文本
    pub fn with_info_text(mut self, text: impl Into<SharedString>) -> Self {
        self.info_text = Some(text.into());
        self
    }
}

impl KernelPickerDelegate {
    /// 获取第一个可选择的内核项索引（跳过标题）
    fn first_selectable_index(entries: &[KernelPickerEntry]) -> usize {
        entries
            .iter()
            .position(|e| matches!(e, KernelPickerEntry::Kernel { .. }))
            .unwrap_or(0)
    }

    /// 获取下一个可选择项索引（上下方向）
    fn next_selectable_index(&self, from: usize, direction: i32) -> usize {
        let len = self.filtered_entries.len();
        if len == 0 {
            return 0;
        }

        let mut index = from as i32 + direction;
        while index >= 0 && (index as usize) < len {
            if matches!(
                self.filtered_entries.get(index as usize),
                Some(KernelPickerEntry::Kernel { .. })
            ) {
                return index as usize;
            }
            index += direction;
        }

        from
    }
}

impl PickerDelegate for KernelPickerDelegate {
    type ListItem = ListItem;

    fn match_count(&self) -> usize {
        self.filtered_entries.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    /// 设置选中项（自动跳过分组标题）
    fn set_selected_index(&mut self, ix: usize, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        if matches!(
            self.filtered_entries.get(ix),
            Some(KernelPickerEntry::SectionHeader(_))
        ) {
            let forward = self.next_selectable_index(ix, 1);
            if forward != ix {
                self.selected_index = forward;
            } else {
                self.selected_index = self.next_selectable_index(ix, -1);
            }
        } else {
            self.selected_index = ix;
        }

        if let Some(KernelPickerEntry::Kernel { spec, .. }) =
            self.filtered_entries.get(self.selected_index)
        {
            self.selected_kernelspec = Some(spec.clone());
        }
        cx.notify();
    }

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        "选择内核...".into()
    }

    /// 搜索过滤内核列表
    fn update_matches(
        &mut self,
        query: String,
        _window: &mut Window,
        _cx: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        if query.is_empty() {
            self.filtered_entries = self.all_entries.clone();
        } else {
            let query_lower = query.to_lowercase();
            let mut filtered = Vec::new();
            let mut pending_header: Option<KernelPickerEntry> = None;

            for entry in &self.all_entries {
                match entry {
                    KernelPickerEntry::SectionHeader(_) => {
                        pending_header = Some(entry.clone());
                    }
                    KernelPickerEntry::Kernel { spec, .. } => {
                        if spec.name().to_lowercase().contains(&query_lower) {
                            if let Some(header) = pending_header.take() {
                                filtered.push(header);
                            }
                            filtered.push(entry.clone());
                        }
                    }
                }
            }

            self.filtered_entries = filtered;
        }

        self.selected_index = Self::first_selectable_index(&self.filtered_entries);
        if let Some(KernelPickerEntry::Kernel { spec, .. }) =
            self.filtered_entries.get(self.selected_index)
        {
            self.selected_kernelspec = Some(spec.clone());
        }

        Task::ready(())
    }

    /// 分组标题前显示分隔线
    fn separators_after_indices(&self) -> Vec<usize> {
        let mut separators = Vec::new();
        for (index, entry) in self.filtered_entries.iter().enumerate() {
            if matches!(entry, KernelPickerEntry::SectionHeader(_)) && index > 0 {
                separators.push(index - 1);
            }
        }
        separators
    }

    /// 确认选择内核
    fn confirm(&mut self, _secondary: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        if let Some(KernelPickerEntry::Kernel { spec, .. }) =
            self.filtered_entries.get(self.selected_index)
        {
            (self.on_select)(spec.clone(), window, cx);
            cx.emit(DismissEvent);
        }
    }

    fn dismissed(&mut self, _window: &mut Window, _cx: &mut Context<Picker<Self>>) {}

    /// 渲染列表项：标题 / 内核
    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let entry = self.filtered_entries.get(ix)?;

        match entry {
            // 分组标题
            KernelPickerEntry::SectionHeader(title) => Some(
                ListItem::new(ix)
                    .inset(true)
                    .spacing(ListItemSpacing::Dense)
                    .selectable(false)
                    .child(
                        Label::new(title.clone())
                            .size(LabelSize::Small)
                            .weight(FontWeight::SEMIBOLD)
                            .color(Color::Muted),
                    ),
            ),
            // 内核项
            KernelPickerEntry::Kernel {
                spec,
                is_recommended,
            } => {
                let is_currently_selected = self.selected_kernelspec.as_ref() == Some(spec);
                let icon = spec.icon(cx);
                let has_ipykernel = spec.has_ipykernel();

                let subtitle = match spec {
                    KernelSpecification::Jupyter(_) => None,
                    KernelSpecification::WslRemote(_) => Some(spec.path().to_string()),
                    KernelSpecification::PythonEnv(_)
                    | KernelSpecification::JupyterServer(_)
                    | KernelSpecification::SshRemote(_) => {
                        let env_kind = spec.environment_kind_label();
                        let path = spec.path();
                        match env_kind {
                            Some(kind) => Some(format!("{} \u{2013} {}", kind, path)),
                            None => Some(path.to_string()),
                        }
                    }
                };

                Some(
                    ListItem::new(ix)
                        .inset(true)
                        .spacing(ListItemSpacing::Sparse)
                        .toggle_state(selected)
                        .child(
                            h_flex()
                                .w_full()
                                .gap_3()
                                // 未安装 ipykernel 则半透明
                                .when(!has_ipykernel, |flex| flex.opacity(0.5))
                                .child(icon.color(Color::Default).size(IconSize::Medium))
                                .child(
                                    v_flex()
                                        .flex_grow()
                                        .overflow_x_hidden()
                                        .gap_0p5()
                                        .child(
                                            h_flex()
                                                .gap_1()
                                                .child(
                                                    div()
                                                        .overflow_x_hidden()
                                                        .flex_shrink()
                                                        .text_ellipsis()
                                                        .child(
                                                            Label::new(spec.name())
                                                                .weight(FontWeight::MEDIUM)
                                                                .size(LabelSize::Default),
                                                        ),
                                                )
                                                // 推荐标签
                                                .when(*is_recommended, |flex| {
                                                    flex.child(
                                                        Label::new("推荐")
                                                            .size(LabelSize::XSmall)
                                                            .color(Color::Accent),
                                                    )
                                                })
                                                // 未安装 ipykernel 警告
                                                .when(!has_ipykernel, |flex| {
                                                    flex.child(
                                                        Label::new("未安装 ipykernel")
                                                            .size(LabelSize::XSmall)
                                                            .color(Color::Warning),
                                                    )
                                                }),
                                        )
                                        .when_some(subtitle, |flex, subtitle| {
                                            flex.child(
                                                div().overflow_x_hidden().text_ellipsis().child(
                                                    Label::new(subtitle)
                                                        .size(LabelSize::Small)
                                                        .color(Color::Muted),
                                                ),
                                            )
                                        }),
                                ),
                        )
                        // 当前选中项显示对勾
                        .when(is_currently_selected, |item| {
                            item.end_slot(
                                Icon::new(IconName::Check)
                                    .color(Color::Accent)
                                    .size(IconSize::Small),
                            )
                        }),
                )
            }
        }
    }

    /// 渲染底部：内核文档链接
    fn render_footer(
        &self,
        _: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<gpui::AnyElement> {
        Some(
            h_flex()
                .w_full()
                .border_t_1()
                .border_color(cx.theme().colors().border_variant)
                .p_1()
                .gap_4()
                .child(
                    Button::new("kernel-docs", "内核文档")
                        .end_icon(
                            Icon::new(IconName::ArrowUpRight)
                                .size(IconSize::Small)
                                .color(Color::Muted),
                        )
                        .on_click(move |_, _, cx| cx.open_url(KERNEL_DOCS_URL)),
                )
                .into_any(),
        )
    }
}

impl<T, TT> RenderOnce for KernelSelector<T, TT>
where
    T: PopoverTrigger + ButtonCommon,
    TT: Fn(&mut Window, &mut App) -> AnyView + 'static,
{
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let store = ReplStore::global(cx);
        store.update(cx, |store, cx| store.ensure_kernelspecs(cx));
        let store = store.read(cx);

        let all_entries = build_grouped_entries(store, self.worktree_id);
        let selected_kernelspec = store.active_kernelspec(self.worktree_id, None, cx);
        let selected_index = all_entries
            .iter()
            .position(|entry| {
                if let KernelPickerEntry::Kernel { spec, .. } = entry {
                    selected_kernelspec.as_ref() == Some(spec)
                } else {
                    false
                }
            })
            .unwrap_or_else(|| KernelPickerDelegate::first_selectable_index(&all_entries));

        let delegate = KernelPickerDelegate {
            on_select: self.on_select,
            all_entries: all_entries.clone(),
            filtered_entries: all_entries,
            selected_kernelspec,
            selected_index,
        };

        let picker_view = cx.new(|cx| {
            Picker::list(delegate, window, cx)
                .list_measure_all()
                .width(rems(34.))
                .max_height(Some(rems(24.).into()))
        });

        PopoverMenu::new("kernel-switcher")
            .menu(move |_window, _cx| Some(picker_view.clone()))
            .trigger_with_tooltip(self.trigger, self.tooltip)
            .attach(gpui::Anchor::BottomLeft)
            .when_some(self.handle, |menu, handle| menu.with_handle(handle))
    }
}