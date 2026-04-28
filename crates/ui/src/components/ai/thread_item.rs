use crate::{CommonAnimationExt, DiffStat, GradientFade, HighlightedLabel, Tooltip, prelude::*};

use gpui::{
    Animation, AnimationExt, ClickEvent, Hsla, MouseButton, SharedString, pulsating_between,
};
use itertools::Itertools as _;
use std::{path::PathBuf, sync::Arc, time::Duration};

/// 智能体线程状态枚举
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AgentThreadStatus {
    #[default]
    Completed,       // 已完成
    Running,         // 运行中
    WaitingForConfirmation, // 等待确认
    Error,           // 错误
}

/// 工作树类型枚举
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WorktreeKind {
    #[default]
    Main,    // 主工作树
    Linked,  // 关联工作树
}

/// 线程项的工作树信息
#[derive(Clone, Default)]
pub struct ThreadItemWorktreeInfo {
    pub worktree_name: Option<SharedString>,   // 工作树名称
    pub branch_name: Option<SharedString>,     // 分支名称
    pub full_path: SharedString,               // 完整路径
    pub highlight_positions: Vec<usize>,       // 高亮位置
    pub kind: WorktreeKind,                    // 工作树类型
}

/// 线程项组件
#[derive(IntoElement, RegisterComponent)]
pub struct ThreadItem {
    id: ElementId,
    icon: IconName,
    icon_color: Option<Color>,
    icon_visible: bool,
    custom_icon_from_external_svg: Option<SharedString>,
    title: SharedString,
    title_label_color: Option<Color>,
    title_generating: bool,
    highlight_positions: Vec<usize>,
    timestamp: SharedString,
    notified: bool,
    status: AgentThreadStatus,
    selected: bool,
    focused: bool,
    hovered: bool,
    rounded: bool,
    added: Option<usize>,
    removed: Option<usize>,
    project_paths: Option<Arc<[PathBuf]>>,
    project_name: Option<SharedString>,
    worktrees: Vec<ThreadItemWorktreeInfo>,
    is_remote: bool,
    archived: bool,
    on_click: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
    on_hover: Box<dyn Fn(&bool, &mut Window, &mut App) + 'static>,
    action_slot: Option<AnyElement>,
    base_bg: Option<Hsla>,
}

impl ThreadItem {
    /// 创建新的线程项
    pub fn new(id: impl Into<ElementId>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            icon: IconName::ZedAgent,
            icon_color: None,
            icon_visible: true,
            custom_icon_from_external_svg: None,
            title: title.into(),
            title_label_color: None,
            title_generating: false,
            highlight_positions: Vec::new(),
            timestamp: "".into(),
            notified: false,
            status: AgentThreadStatus::default(),
            selected: false,
            focused: false,
            hovered: false,
            rounded: false,
            added: None,
            removed: None,
            project_paths: None,
            project_name: None,
            worktrees: Vec::new(),
            is_remote: false,
            archived: false,
            on_click: None,
            on_hover: Box::new(|_, _, _| {}),
            action_slot: None,
            base_bg: None,
        }
    }

    /// 设置时间戳
    pub fn timestamp(mut self, timestamp: impl Into<SharedString>) -> Self {
        self.timestamp = timestamp.into();
        self
    }

    /// 设置图标
    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = icon;
        self
    }

    /// 设置图标颜色
    pub fn icon_color(mut self, color: Color) -> Self {
        self.icon_color = Some(color);
        self
    }

    /// 设置图标可见性
    pub fn icon_visible(mut self, visible: bool) -> Self {
        self.icon_visible = visible;
        self
    }

    /// 设置外部SVG自定义图标
    pub fn custom_icon_from_external_svg(mut self, svg: impl Into<SharedString>) -> Self {
        self.custom_icon_from_external_svg = Some(svg.into());
        self
    }

    /// 设置通知状态
    pub fn notified(mut self, notified: bool) -> Self {
        self.notified = notified;
        self
    }

    /// 设置线程状态
    pub fn status(mut self, status: AgentThreadStatus) -> Self {
        self.status = status;
        self
    }

    /// 设置标题生成状态
    pub fn title_generating(mut self, generating: bool) -> Self {
        self.title_generating = generating;
        self
    }

    /// 设置标题标签颜色
    pub fn title_label_color(mut self, color: Color) -> Self {
        self.title_label_color = Some(color);
        self
    }

    /// 设置高亮位置
    pub fn highlight_positions(mut self, positions: Vec<usize>) -> Self {
        self.highlight_positions = positions;
        self
    }

    /// 设置选中状态
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// 设置焦点状态
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// 设置新增代码行数
    pub fn added(mut self, added: usize) -> Self {
        self.added = Some(added);
        self
    }

    /// 设置删除代码行数
    pub fn removed(mut self, removed: usize) -> Self {
        self.removed = Some(removed);
        self
    }

    /// 设置项目路径
    pub fn project_paths(mut self, paths: Arc<[PathBuf]>) -> Self {
        self.project_paths = Some(paths);
        self
    }

    /// 设置项目名称
    pub fn project_name(mut self, name: impl Into<SharedString>) -> Self {
        self.project_name = Some(name.into());
        self
    }

    /// 设置工作树列表
    pub fn worktrees(mut self, worktrees: Vec<ThreadItemWorktreeInfo>) -> Self {
        self.worktrees = worktrees;
        self
    }

    /// 设置是否为远程项目
    pub fn is_remote(mut self, is_remote: bool) -> Self {
        self.is_remote = is_remote;
        self
    }

    /// 设置归档状态
    pub fn archived(mut self, archived: bool) -> Self {
        self.archived = archived;
        self
    }

    /// 设置悬停状态
    pub fn hovered(mut self, hovered: bool) -> Self {
        self.hovered = hovered;
        self
    }

    /// 设置圆角样式
    pub fn rounded(mut self, rounded: bool) -> Self {
        self.rounded = rounded;
        self
    }

    /// 设置点击事件
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    /// 设置悬停事件
    pub fn on_hover(mut self, on_hover: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_hover = Box::new(on_hover);
        self
    }

    /// 设置操作槽位
    pub fn action_slot(mut self, element: impl IntoElement) -> Self {
        self.action_slot = Some(element.into_any_element());
        self
    }

    /// 设置基础背景色
    pub fn base_bg(mut self, color: Hsla) -> Self {
        self.base_bg = Some(color);
        self
    }
}

impl RenderOnce for ThreadItem {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let color = cx.theme().colors();
        let sidebar_base_bg = color
            .title_bar_background
            .blend(color.panel_background.opacity(0.25));

        let raw_bg = self.base_bg.unwrap_or(sidebar_base_bg);
        let apparent_bg = color.background.blend(raw_bg);

        let base_bg = if self.selected {
            apparent_bg.blend(color.element_active)
        } else {
            apparent_bg
        };

        let hover_color = color
            .element_active
            .blend(color.element_background.opacity(0.2));
        let hover_bg = apparent_bg.blend(hover_color);

        let gradient_overlay = GradientFade::new(base_bg, hover_bg, hover_bg)
            .width(px(64.0))
            .right(px(-10.0))
            .gradient_stop(0.75)
            .group_name("thread-item");

        let separator_color = Color::Custom(color.text_muted.opacity(0.4));
        // 点分隔符
        let dot_separator = || {
            Label::new("•")
                .size(LabelSize::Small)
                .color(separator_color)
        };

        let icon_id = format!("icon-{}", self.id);
        let icon_visible = self.icon_visible;
        // 图标容器
        let icon_container = || {
            h_flex()
                .id(icon_id.clone())
                .size_4()
                .flex_none()
                .justify_center()
                .when(!icon_visible, |this| this.invisible())
        };
        let icon_color = self.icon_color.unwrap_or(Color::Muted);
        let agent_icon = if let Some(custom_svg) = self.custom_icon_from_external_svg {
            Icon::from_external_svg(custom_svg)
                .color(icon_color)
                .size(IconSize::Small)
        } else {
            Icon::new(self.icon).color(icon_color).size(IconSize::Small)
        };

        // 状态图标
        let status_icon = if self.status == AgentThreadStatus::Error {
            Some(
                Icon::new(IconName::Close)
                    .size(IconSize::Small)
                    .color(Color::Error),
            )
        } else if self.status == AgentThreadStatus::WaitingForConfirmation {
            Some(
                Icon::new(IconName::Warning)
                    .size(IconSize::XSmall)
                    .color(Color::Warning),
            )
        } else if self.notified {
            Some(
                Icon::new(IconName::Circle)
                    .size(IconSize::Small)
                    .color(Color::Accent),
            )
        } else {
            None
        };

        let icon = if self.status == AgentThreadStatus::Running {
            icon_container()
                .child(
                    Icon::new(IconName::LoadCircle)
                        .size(IconSize::Small)
                        .color(Color::Muted)
                        .with_rotate_animation(2),
                )
                .into_any_element()
        } else if let Some(status_icon) = status_icon {
            icon_container().child(status_icon).into_any_element()
        } else {
            icon_container().child(agent_icon).into_any_element()
        };

        let title = self.title;
        let highlight_positions = self.highlight_positions;

        // 标题标签渲染
        let title_label = if self.title_generating {
            Label::new(title)
                .color(Color::Muted)
                .with_animation(
                    "generating-title",
                    Animation::new(Duration::from_secs(2))
                        .repeat()
                        .with_easing(pulsating_between(0.4, 0.8)),
                    |label, delta| label.alpha(delta),
                )
                .into_any_element()
        } else if highlight_positions.is_empty() {
            Label::new(title)
                .when_some(self.title_label_color, |label, color| label.color(color))
                .into_any_element()
        } else {
            HighlightedLabel::new(title, highlight_positions)
                .when_some(self.title_label_color, |label, color| label.color(color))
                .into_any_element()
        };

        let has_diff_stats = self.added.is_some() || self.removed.is_some();
        let diff_stat_id = self.id.clone();
        let added_count = self.added.unwrap_or(0);
        let removed_count = self.removed.unwrap_or(0);

        // 处理项目路径显示
        let project_paths = self.project_paths.as_ref().and_then(|paths| {
            let paths_str = paths
                .as_ref()
                .iter()
                .filter_map(|p| p.file_name())
                .filter_map(|name| name.to_str())
                .join(", ");
            if paths_str.is_empty() {
                None
            } else {
                Some(paths_str)
            }
        });

        let has_project_name = self.project_name.is_some();
        let has_project_paths = project_paths.is_some();
        let has_timestamp = !self.timestamp.is_empty();
        let timestamp = self.timestamp;

        // 是否显示提示框
        let show_tooltip = matches!(
            self.status,
            AgentThreadStatus::Error | AgentThreadStatus::WaitingForConfirmation
        );

        // 筛选关联工作树
        let linked_worktrees: Vec<ThreadItemWorktreeInfo> = self
            .worktrees
            .into_iter()
            .filter(|wt| wt.kind == WorktreeKind::Linked)
            .filter(|wt| wt.worktree_name.is_some() || wt.branch_name.is_some())
            .collect();

        let has_worktree = !linked_worktrees.is_empty();

        // 是否包含元数据
        let has_metadata = has_project_name
            || has_project_paths
            || has_worktree
            || has_diff_stats
            || has_timestamp;

        // 主布局渲染
        v_flex()
            .id(self.id.clone())
            .cursor_pointer()
            .group("thread-item")
            .relative()
            .flex_shrink_0()
            .overflow_hidden()
            .w_full()
            .py_1()
            .px_1p5()
            .when(self.selected, |s| s.bg(color.element_active))
            .border_1()
            .border_color(gpui::transparent_black())
            .when(self.focused, |s| s.border_color(color.border_focused))
            .when(self.rounded, |s| s.rounded_sm())
            .hover(|s| s.bg(hover_color))
            .on_hover(self.on_hover)
            .child(
                h_flex()
                    .min_w_0()
                    .w_full()
                    .gap_2()
                    .justify_between()
                    .child(
                        h_flex()
                            .id("content")
                            .min_w_0()
                            .flex_1()
                            .gap_1p5()
                            .child(icon)
                            .child(title_label),
                    )
                    .child(gradient_overlay)
                    .when(self.hovered, |this| {
                        this.when_some(self.action_slot, |this, slot| {
                            let overlay = GradientFade::new(base_bg, hover_bg, hover_bg)
                                .width(px(80.0))
                                .right(px(8.))
                                .gradient_stop(0.80)
                                .group_name("thread-item");

                            this.child(
                                h_flex()
                                    .relative()
                                    .pr_1p5()
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .child(overlay)
                                    .child(slot),
                            )
                        })
                    }),
            )
            .when(has_metadata, |this| {
                this.child(
                    h_flex()
                        .gap_1p5()
                        .child(icon_container()) // 图标占位
                        .when(self.archived, |this| {
                            this.child(
                                Icon::new(IconName::Archive).size(IconSize::XSmall).color(
                                    Color::Custom(cx.theme().colors().icon_muted.opacity(0.5)),
                                ),
                            )
                        })
                        .when(
                            has_project_name || has_project_paths || has_worktree,
                            |this| {
                                this.when_some(self.project_name, |this, name| {
                                    this.child(
                                        Label::new(name).size(LabelSize::Small).color(Color::Muted),
                                    )
                                })
                                .when(
                                    has_project_name && (has_project_paths || has_worktree),
                                    |this| this.child(dot_separator()),
                                )
                                .when_some(project_paths, |this, paths| {
                                    this.child(
                                        Label::new(paths)
                                            .size(LabelSize::Small)
                                            .color(Color::Muted),
                                    )
                                })
                                .when(has_project_paths && has_worktree, |this| {
                                    this.child(dot_separator())
                                })
                                .children(
                                    linked_worktrees.into_iter().map(|wt| {
                                        let worktree_label = wt.worktree_name.clone().map(|name| {
                                            if wt.highlight_positions.is_empty() {
                                                Label::new(name)
                                                    .size(LabelSize::Small)
                                                    .color(Color::Muted)
                                                    .truncate()
                                                    .into_any_element()
                                            } else {
                                                HighlightedLabel::new(
                                                    name,
                                                    wt.highlight_positions.clone(),
                                                )
                                                .size(LabelSize::Small)
                                                .color(Color::Muted)
                                                .truncate()
                                                .into_any_element()
                                            }
                                        });

                                        // 仅显示分支时使用分支图标，否则使用工作树图标
                                        let chip_icon = if wt.worktree_name.is_none()
                                            && wt.branch_name.is_some()
                                        {
                                            IconName::GitBranch
                                        } else {
                                            IconName::GitWorktree
                                        };

                                        let branch_label = wt.branch_name.map(|branch| {
                                            Label::new(branch)
                                                .size(LabelSize::Small)
                                                .color(Color::Muted)
                                                .truncate()
                                                .into_any_element()
                                        });

                                        let show_separator =
                                            worktree_label.is_some() && branch_label.is_some();

                                        h_flex()
                                            .min_w_0()
                                            .gap_0p5()
                                            .child(
                                                Icon::new(chip_icon)
                                                    .size(IconSize::XSmall)
                                                    .color(Color::Muted),
                                            )
                                            .when_some(worktree_label, |this, label| {
                                                this.child(label)
                                            })
                                            .when(show_separator, |this| {
                                                this.child(
                                                    Label::new("/")
                                                        .size(LabelSize::Small)
                                                        .color(separator_color)
                                                        .flex_shrink_0(),
                                                )
                                            })
                                            .when_some(branch_label, |this, label| {
                                                this.child(label)
                                            })
                                    }),
                                )
                            },
                        )
                        .when(
                            (has_project_name || has_project_paths || has_worktree)
                                && (has_diff_stats || has_timestamp),
                            |this| this.child(dot_separator()),
                        )
                        .when(has_diff_stats, |this| {
                            this.child(DiffStat::new(diff_stat_id, added_count, removed_count))
                        })
                        .when(has_diff_stats && has_timestamp, |this| {
                            this.child(dot_separator())
                        })
                        .when(has_timestamp, |this| {
                            this.child(
                                Label::new(timestamp.clone())
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            )
                        }),
                )
            })
            // 状态提示框
            .when(show_tooltip, |this| {
                let status = self.status;
                this.tooltip(Tooltip::element(move |_, _| match status {
                    AgentThreadStatus::Error => h_flex()
                        .gap_1()
                        .child(
                            Icon::new(IconName::Close)
                                .size(IconSize::Small)
                                .color(Color::Error),
                        )
                        .child(Label::new("线程发生错误"))
                        .into_any_element(),
                    AgentThreadStatus::WaitingForConfirmation => h_flex()
                        .gap_1()
                        .child(
                            Icon::new(IconName::Warning)
                                .size(IconSize::Small)
                                .color(Color::Warning),
                        )
                        .child(Label::new("等待确认"))
                        .into_any_element(),
                    _ => gpui::Empty.into_any_element(),
                }))
            })
            .when_some(self.on_click, |this, on_click| this.on_click(on_click))
    }
}

impl Component for ThreadItem {
    fn scope() -> ComponentScope {
        ComponentScope::Agent
    }

    /// 组件预览
    fn preview(_window: &mut Window, cx: &mut App) -> Option<AnyElement> {
        let color = cx.theme().colors();
        let bg = color
            .title_bar_background
            .blend(color.panel_background.opacity(0.25));

        let container = || {
            v_flex()
                .w_72()
                .border_1()
                .border_color(color.border_variant)
                .bg(bg)
        };

        let thread_item_examples = vec![
            single_example(
                "默认样式",
                container()
                    .child(
                        ThreadItem::new("ti-1", "根据设置链接到智能体面板")
                            .icon(IconName::AiOpenAi)
                            .timestamp("15分钟前"),
                    )
                    .into_any_element(),
            ),
            single_example(
                "等待确认",
                container()
                    .child(
                        ThreadItem::new("ti-2b", "在终端执行shell命令")
                            .timestamp("2小时前")
                            .status(AgentThreadStatus::WaitingForConfirmation),
                    )
                    .into_any_element(),
            ),
            single_example(
                "错误状态",
                container()
                    .child(
                        ThreadItem::new("ti-2c", "连接语言服务失败")
                            .timestamp("5小时前")
                            .status(AgentThreadStatus::Error),
                    )
                    .into_any_element(),
            ),
            single_example(
                "智能体运行中",
                container()
                    .child(
                        ThreadItem::new("ti-3", "为文件编辑块添加行号选项")
                            .icon(IconName::AiClaude)
                            .timestamp("23小时前")
                            .status(AgentThreadStatus::Running),
                    )
                    .into_any_element(),
            ),
            single_example(
                "工作树中",
                container()
                    .child(
                        ThreadItem::new("ti-4", "为文件编辑块添加行号选项")
                            .icon(IconName::AiClaude)
                            .timestamp("2周前")
                            .worktrees(vec![ThreadItemWorktreeInfo {
                                worktree_name: Some("link-agent-panel".into()),
                                full_path: "link-agent-panel".into(),
                                highlight_positions: Vec::new(),
                                kind: WorktreeKind::Linked,
                                branch_name: None,
                            }]),
                    )
                    .into_any_element(),
            ),
            single_example(
                "包含代码变更",
                container()
                    .child(
                        ThreadItem::new("ti-5", "管理用户和项目设置交互")
                            .icon(IconName::AiClaude)
                            .timestamp("1个月前")
                            .added(10)
                            .removed(3),
                    )
                    .into_any_element(),
            ),
            single_example(
                "工作树+变更+时间戳",
                container()
                    .child(
                        ThreadItem::new("ti-5b", "完整元数据示例")
                            .icon(IconName::AiClaude)
                            .worktrees(vec![ThreadItemWorktreeInfo {
                                worktree_name: Some("my-project".into()),
                                full_path: "my-project".into(),
                                highlight_positions: Vec::new(),
                                kind: WorktreeKind::Linked,
                                branch_name: None,
                            }])
                            .added(42)
                            .removed(17)
                            .timestamp("3周前"),
                    )
                    .into_any_element(),
            ),
            single_example(
                "工作树+分支+变更+时间戳",
                container()
                    .child(
                        ThreadItem::new("ti-5c", "包含分支的完整元数据")
                            .icon(IconName::AiClaude)
                            .worktrees(vec![ThreadItemWorktreeInfo {
                                worktree_name: Some("my-project".into()),
                                full_path: "/worktrees/my-project/zed".into(),
                                highlight_positions: Vec::new(),
                                kind: WorktreeKind::Linked,
                                branch_name: Some("feature-branch".into()),
                            }])
                            .added(42)
                            .removed(17)
                            .timestamp("3周前"),
                    )
                    .into_any_element(),
            ),
            single_example(
                "长分支名+变更（自动截断）",
                container()
                    .child(
                        ThreadItem::new("ti-5d", "长分支名元数据溢出示例")
                            .icon(IconName::AiClaude)
                            .worktrees(vec![ThreadItemWorktreeInfo {
                                worktree_name: Some("my-project".into()),
                                full_path: "/worktrees/my-project/zed".into(),
                                highlight_positions: Vec::new(),
                                kind: WorktreeKind::Linked,
                                branch_name: Some("fix-very-long-branch-name-here".into()),
                            }])
                            .added(108)
                            .removed(53)
                            .timestamp("2天前"),
                    )
                    .into_any_element(),
            ),
            single_example(
                "主工作树（隐藏）+变更+时间戳",
                container()
                    .child(
                        ThreadItem::new("ti-5e", "带差异统计的主工作树分支")
                            .icon(IconName::ZedAgent)
                            .worktrees(vec![ThreadItemWorktreeInfo {
                                worktree_name: Some("zed".into()),
                                full_path: "/projects/zed".into(),
                                highlight_positions: Vec::new(),
                                kind: WorktreeKind::Main,
                                branch_name: Some("sidebar-show-branch-name".into()),
                            }])
                            .added(23)
                            .removed(8)
                            .timestamp("5分钟前"),
                    )
                    .into_any_element(),
            ),
            single_example(
                "长工作树名（自动截断）",
                container()
                    .child(
                        ThreadItem::new("ti-5f", "超长工作树名线程")
                            .icon(IconName::AiClaude)
                            .worktrees(vec![ThreadItemWorktreeInfo {
                                worktree_name: Some(
                                    "very-long-worktree-name-that-should-truncate".into(),
                                ),
                                full_path: "/worktrees/very-long-worktree-name/zed".into(),
                                highlight_positions: Vec::new(),
                                kind: WorktreeKind::Linked,
                                branch_name: None,
                            }])
                            .timestamp("1小时前"),
                    )
                    .into_any_element(),
            ),
            single_example(
                "带搜索高亮的工作树",
                container()
                    .child(
                        ThreadItem::new("ti-5g", "带高亮工作树的筛选线程")
                            .icon(IconName::AiClaude)
                            .worktrees(vec![ThreadItemWorktreeInfo {
                                worktree_name: Some("jade-glen".into()),
                                full_path: "/worktrees/jade-glen/zed".into(),
                                highlight_positions: vec![0, 1, 2, 3],
                                kind: WorktreeKind::Linked,
                                branch_name: Some("fix-scrolling".into()),
                            }])
                            .timestamp("3天前"),
                    )
                    .into_any_element(),
            ),
            single_example(
                "多工作树（无分支）",
                container()
                    .child(
                        ThreadItem::new("ti-5h", "跨多工作树的线程")
                            .icon(IconName::AiClaude)
                            .worktrees(vec![
                                ThreadItemWorktreeInfo {
                                    worktree_name: Some("jade-glen".into()),
                                    full_path: "/worktrees/jade-glen/zed".into(),
                                    highlight_positions: Vec::new(),
                                    kind: WorktreeKind::Linked,
                                    branch_name: None,
                                },
                                ThreadItemWorktreeInfo {
                                    worktree_name: Some("fawn-otter".into()),
                                    full_path: "/worktrees/fawn-otter/zed-slides".into(),
                                    highlight_positions: Vec::new(),
                                    kind: WorktreeKind::Linked,
                                    branch_name: None,
                                },
                            ])
                            .timestamp("2小时前"),
                    )
                    .into_any_element(),
            ),
            single_example(
                "带分支的多工作树",
                container()
                    .child(
                        ThreadItem::new("ti-5i", "带独立分支的多根目录")
                            .icon(IconName::ZedAgent)
                            .worktrees(vec![
                                ThreadItemWorktreeInfo {
                                    worktree_name: Some("jade-glen".into()),
                                    full_path: "/worktrees/jade-glen/zed".into(),
                                    highlight_positions: Vec::new(),
                                    kind: WorktreeKind::Linked,
                                    branch_name: Some("fix".into()),
                                },
                                ThreadItemWorktreeInfo {
                                    worktree_name: Some("fawn-otter".into()),
                                    full_path: "/worktrees/fawn-otter/zed-slides".into(),
                                    highlight_positions: Vec::new(),
                                    kind: WorktreeKind::Linked,
                                    branch_name: Some("main".into()),
                                },
                            ])
                            .timestamp("15分钟前"),
                    )
                    .into_any_element(),
            ),
            single_example(
                "项目名+工作树+分支",
                container()
                    .child(
                        ThreadItem::new("ti-5j", "带项目上下文的线程")
                            .icon(IconName::AiClaude)
                            .project_name("my-remote-server")
                            .worktrees(vec![ThreadItemWorktreeInfo {
                                worktree_name: Some("jade-glen".into()),
                                full_path: "/worktrees/jade-glen/zed".into(),
                                highlight_positions: Vec::new(),
                                kind: WorktreeKind::Linked,
                                branch_name: Some("feature-branch".into()),
                            }])
                            .timestamp("1天前"),
                    )
                    .into_any_element(),
            ),
            single_example(
                "项目路径+工作树（归档视图）",
                container()
                    .child(
                        ThreadItem::new("ti-5k", "带文件夹路径的归档线程")
                            .icon(IconName::AiClaude)
                            .project_paths(Arc::from(vec![
                                PathBuf::from("/projects/zed"),
                                PathBuf::from("/projects/zed-slides"),
                            ]))
                            .worktrees(vec![ThreadItemWorktreeInfo {
                                worktree_name: Some("jade-glen".into()),
                                full_path: "/worktrees/jade-glen/zed".into(),
                                highlight_positions: Vec::new(),
                                kind: WorktreeKind::Linked,
                                branch_name: Some("feature".into()),
                            }])
                            .timestamp("2个月前"),
                    )
                    .into_any_element(),
            ),
            single_example(
                "完整元数据",
                container()
                    .child(
                        ThreadItem::new("ti-5l", "所有元数据字段均已填充")
                            .icon(IconName::ZedAgent)
                            .project_name("remote-dev")
                            .worktrees(vec![ThreadItemWorktreeInfo {
                                worktree_name: Some("my-worktree".into()),
                                full_path: "/worktrees/my-worktree/zed".into(),
                                highlight_positions: Vec::new(),
                                kind: WorktreeKind::Linked,
                                branch_name: Some("main".into()),
                            }])
                            .added(15)
                            .removed(4)
                            .timestamp("8小时前"),
                    )
                    .into_any_element(),
            ),
            single_example(
                "焦点项（键盘选择）",
                container()
                    .child(
                        ThreadItem::new("ti-7", "实现键盘导航")
                            .icon(IconName::AiClaude)
                            .timestamp("12小时前")
                            .focused(true),
                    )
                    .into_any_element(),
            ),
            single_example(
                "操作槽位",
                container()
                    .child(
                        ThreadItem::new("ti-9", "悬停查看操作按钮")
                            .icon(IconName::AiClaude)
                            .timestamp("6小时前")
                            .hovered(true)
                            .action_slot(
                                IconButton::new("delete", IconName::Trash)
                                    .icon_size(IconSize::Small)
                                    .icon_color(Color::Muted),
                            ),
                    )
                    .into_any_element(),
            ),
        ];

        Some(
            example_group(thread_item_examples)
                .vertical()
                .into_any_element(),
        )
    }
}