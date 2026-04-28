use command_palette_hooks::CommandPaletteFilter;
use editor::{
    Anchor, Editor, HighlightKey, MultiBufferOffset, SelectionEffects, scroll::Autoscroll,
};
use gpui::{
    App, AppContext as _, Context, Div, Entity, EntityId, EventEmitter, FocusHandle, Focusable,
    Hsla, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    ParentElement, Render, ScrollStrategy, SharedString, Styled, Task, UniformListScrollHandle,
    WeakEntity, Window, actions, div, rems, uniform_list,
};
use language::{Buffer, OwnedSyntaxLayer};
use std::{any::TypeId, mem, ops::Range};
use theme::ActiveTheme;
use tree_sitter::{Node, TreeCursor};
use ui::{
    ButtonCommon, ButtonLike, Clickable, Color, ContextMenu, FluentBuilder as _, IconButton,
    IconName, Label, LabelCommon, LabelSize, PopoverMenu, StyledExt, Tooltip, WithScrollbar,
    h_flex, v_flex,
};
use workspace::{
    Event as WorkspaceEvent, SplitDirection, ToolbarItemEvent, ToolbarItemLocation,
    ToolbarItemView, Workspace,
    item::{Item, ItemHandle},
};

// 定义开发者操作：打开当前文件的语法树视图
actions!(
    dev,
    [
        /// 为当前文件打开语法树视图
        OpenSyntaxTreeView,
    ]
);

// 定义语法树视图操作
actions!(
    syntax_tree_view,
    [
        /// 更新语法树视图，显示最后聚焦的文件
        UseActiveEditor
    ]
);

/// 初始化模块：注册语法树视图相关功能
pub fn init(cx: &mut App) {
    let syntax_tree_actions = [TypeId::of::<UseActiveEditor>()];

    // 全局隐藏内部操作
    CommandPaletteFilter::update_global(cx, |this, _| {
        this.hide_action_types(&syntax_tree_actions);
    });

    cx.observe_new(move |workspace: &mut Workspace, _, _| {
        // 注册打开语法树视图操作
        workspace.register_action(move |workspace, _: &OpenSyntaxTreeView, window, cx| {
            CommandPaletteFilter::update_global(cx, |this, _| {
                this.show_action_types(&syntax_tree_actions);
            });

            let active_item = workspace.active_item(cx);
            let workspace_handle = workspace.weak_handle();
            let syntax_tree_view = cx.new(|cx| {
                // 视图释放时隐藏操作
                cx.on_release(move |view: &mut SyntaxTreeView, cx| {
                    if view
                        .workspace_handle
                        .read_with(cx, |workspace, cx| {
                            workspace.item_of_type::<SyntaxTreeView>(cx).is_none()
                        })
                        .unwrap_or_default()
                    {
                        CommandPaletteFilter::update_global(cx, |this, _| {
                            this.hide_action_types(&syntax_tree_actions);
                        });
                    }
                })
                .detach();

                SyntaxTreeView::new(workspace_handle, active_item, window, cx)
            });
            // 在右侧分割打开语法树视图
            workspace.split_item(
                SplitDirection::Right,
                Box::new(syntax_tree_view),
                window,
                cx,
            )
        });
        // 注册切换到当前编辑器的操作
        workspace.register_action(|workspace, _: &UseActiveEditor, window, cx| {
            if let Some(tree_view) = workspace.item_of_type::<SyntaxTreeView>(cx) {
                tree_view.update(cx, |view, cx| {
                    view.update_active_editor(&Default::default(), window, cx)
                })
            }
        });
    })
    .detach();
}

/// 语法树视图（核心组件）
/// 实时展示当前编辑器文件的 Tree-sitter 语法树结构，支持点击定位、悬停高亮
pub struct SyntaxTreeView {
    workspace_handle: WeakEntity<Workspace>,
    editor: Option<EditorState>,
    list_scroll_handle: UniformListScrollHandle,
    /// 工作区最后激活的编辑器（非当前显示的编辑器）
    last_active_editor: Option<Entity<Editor>>,
    selected_descendant_ix: Option<usize>,
    hovered_descendant_ix: Option<usize>,
    focus_handle: FocusHandle,
}

/// 语法树工具栏视图
pub struct SyntaxTreeToolbarItemView {
    tree_view: Option<Entity<SyntaxTreeView>>,
    subscription: Option<gpui::Subscription>,
}

/// 编辑器状态：绑定的编辑器及缓冲区信息
struct EditorState {
    editor: Entity<Editor>,
    active_buffer: Option<BufferState>,
    _subscription: gpui::Subscription,
}

impl EditorState {
    /// 判断是否关联了语言（语法解析）
    fn has_language(&self) -> bool {
        self.active_buffer
            .as_ref()
            .is_some_and(|buffer| buffer.active_layer.is_some())
    }
}

/// 缓冲区状态：当前解析的语法层
#[derive(Clone)]
struct BufferState {
    buffer: Entity<Buffer>,
    active_layer: Option<OwnedSyntaxLayer>,
}

impl SyntaxTreeView {
    /// 创建新的语法树视图
    pub fn new(
        workspace_handle: WeakEntity<Workspace>,
        active_item: Option<Box<dyn ItemHandle>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            workspace_handle: workspace_handle.clone(),
            list_scroll_handle: UniformListScrollHandle::new(),
            editor: None,
            last_active_editor: None,
            hovered_descendant_ix: None,
            selected_descendant_ix: None,
            focus_handle: cx.focus_handle(),
        };

        this.handle_item_updated(active_item, window, cx);

        // 监听工作区事件：激活项变更、项目移除
        cx.subscribe_in(
            &workspace_handle.upgrade().unwrap(),
            window,
            move |this, workspace, event, window, cx| match event {
                WorkspaceEvent::ItemAdded { .. } | WorkspaceEvent::ActiveItemChanged => {
                    this.handle_item_updated(workspace.read(cx).active_item(cx), window, cx)
                }
                WorkspaceEvent::ItemRemoved { item_id } => {
                    this.handle_item_removed(item_id, window, cx);
                }
                _ => {}
            },
        )
        .detach();

        this
    }

    /// 处理激活项变更
    fn handle_item_updated(
        &mut self,
        active_item: Option<Box<dyn ItemHandle>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = active_item
            .filter(|item| item.item_id() != cx.entity_id())
            .and_then(|item| item.act_as::<Editor>(cx))
        else {
            return;
        };

        if let Some(editor_state) = self.editor.as_ref().filter(|state| state.has_language()) {
            self.last_active_editor = (editor_state.editor != editor).then_some(editor);
        } else {
            self.set_editor(editor, window, cx);
        }
    }

    /// 处理项目关闭
    fn handle_item_removed(
        &mut self,
        item_id: &EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .editor
            .as_ref()
            .is_some_and(|state| state.editor.entity_id() == *item_id)
        {
            self.editor = None;
            // 尝试激活最后使用的编辑器
            self.update_active_editor(&Default::default(), window, cx);
            cx.notify();
        }
    }

    /// 切换到当前激活的编辑器
    fn update_active_editor(
        &mut self,
        _: &UseActiveEditor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.last_active_editor.take() else {
            return;
        };
        self.set_editor(editor, window, cx);
    }

    /// 绑定目标编辑器
    fn set_editor(&mut self, editor: Entity<Editor>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(state) = &self.editor {
            if state.editor == editor {
                return;
            }
            let key = HighlightKey::SyntaxTreeView(cx.entity_id().as_u64() as usize);
            editor.update(cx, |editor, cx| editor.clear_background_highlights(key, cx));
        }

        // 订阅编辑器事件：重解析、选区变化
        let subscription = cx.subscribe_in(&editor, window, |this, _, event, window, cx| {
            let did_reparse = match event {
                editor::EditorEvent::Reparsed(_) => true,
                editor::EditorEvent::SelectionsChanged { .. } => false,
                _ => return,
            };
            this.editor_updated(did_reparse, window, cx);
        });

        self.editor = Some(EditorState {
            editor,
            _subscription: subscription,
            active_buffer: None,
        });
        self.editor_updated(true, window, cx);
    }

    /// 编辑器更新（重解析/光标移动）
    fn editor_updated(
        &mut self,
        did_reparse: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<()> {
        let editor_state = self.editor.as_mut()?;
        let snapshot = editor_state
            .editor
            .update(cx, |editor, cx| editor.snapshot(window, cx));
        let (buffer, range) = editor_state.editor.update(cx, |editor, cx| {
            let selection_range = editor
                .selections
                .last::<MultiBufferOffset>(&editor.display_snapshot(cx))
                .range();
            let multi_buffer = editor.buffer().read(cx);
            let (buffer, range, _) = snapshot
                .buffer_snapshot()
                .range_to_buffer_ranges(selection_range.start..selection_range.end)
                .pop()?;
            let buffer = multi_buffer.buffer(buffer.remote_id()).unwrap();
            Some((buffer, range))
        })?;

        // 光标切换片段时，获取新的语法层
        let buffer_state = editor_state
            .active_buffer
            .get_or_insert_with(|| BufferState {
                buffer: buffer.clone(),
                active_layer: None,
            });
        let mut prev_layer = None;
        if did_reparse {
            prev_layer = buffer_state.active_layer.take();
        }
        if buffer_state.buffer != buffer {
            buffer_state.buffer = buffer.clone();
            buffer_state.active_layer = None;
        }

        let layer = match &mut buffer_state.active_layer {
            Some(layer) => layer,
            None => {
                let snapshot = buffer.read(cx).snapshot();
                let layer = if let Some(prev_layer) = prev_layer {
                    let prev_range = prev_layer.node().byte_range();
                    snapshot
                        .syntax_layers()
                        .filter(|layer| layer.language == &prev_layer.language)
                        .min_by_key(|layer| {
                            let range = layer.node().byte_range();
                            ((range.start as i64) - (prev_range.start as i64)).abs()
                                + ((range.end as i64) - (prev_range.end as i64)).abs()
                        })?
                } else {
                    snapshot.syntax_layers().next()?
                };
                buffer_state.active_layer.insert(layer.to_owned())
            }
        };

        // 在激活层中找到光标下的语法节点，并滚动定位
        let mut cursor = layer.node().walk();
        while cursor.goto_first_child_for_byte(range.start.0).is_some() {
            if !range.is_empty() && cursor.node().end_byte() == range.start.0 {
                cursor.goto_next_sibling();
            }
        }

        // 向上找到包含选区的最小祖先
        loop {
            let node_range = cursor.node().byte_range();
            if node_range.start <= range.start.0 && node_range.end >= range.end.0 {
                break;
            }
            if !cursor.goto_parent() {
                break;
            }
        }

        let descendant_ix = cursor.descendant_index();
        self.selected_descendant_ix = Some(descendant_ix);
        self.list_scroll_handle
            .scroll_to_item(descendant_ix, ScrollStrategy::Center);

        cx.notify();
        Some(())
    }

    /// 根据节点索引更新编辑器选区
    fn update_editor_with_range_for_descendant_ix(
        &self,
        descendant_ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: &mut dyn FnMut(&mut Editor, Range<Anchor>, usize, &mut Window, &mut Context<Editor>),
    ) -> Option<()> {
        let editor_state = self.editor.as_ref()?;
        let buffer_state = editor_state.active_buffer.as_ref()?;
        let layer = buffer_state.active_layer.as_ref()?;

        // 查找节点
        let mut cursor = layer.node().walk();
        cursor.goto_descendant(descendant_ix);
        let node = cursor.node();
        let range = node.byte_range();

        // 构建锚点范围
        let buffer = buffer_state.buffer.read(cx);
        let range = buffer.anchor_before(range.start)..buffer.anchor_after(range.end);

        // 转换为多缓冲区范围
        let multibuffer = editor_state.editor.read(cx).buffer();
        let multibuffer = multibuffer.read(cx).snapshot(cx);
        let range = multibuffer.buffer_anchor_range_to_anchor_range(range)?;
        let key = cx.entity_id().as_u64() as usize;

        // 更新编辑器
        editor_state.editor.update(cx, |editor, cx| {
            f(editor, range, key, window, cx);
        });
        Some(())
    }

    /// 渲染单个语法节点
    fn render_node(cursor: &TreeCursor, depth: u32, selected: bool, cx: &App) -> Div {
        let colors = cx.theme().colors();
        let mut row = h_flex();
        if let Some(field_name) = cursor.field_name() {
            row = row.children([Label::new(field_name).color(Color::Info), Label::new(": ")]);
        }

        let node = cursor.node();
        row.child(if node.is_named() {
            Label::new(node.kind()).color(Color::Default)
        } else {
            Label::new(format!("\"{}\"", node.kind())).color(Color::Created)
        })
        .child(
            div()
                .child(Label::new(format_node_range(node)).color(Color::Muted))
                .pl_1(),
        )
        .text_bg(if selected {
            colors.element_selected
        } else {
            Hsla::default()
        })
        .pl(rems(depth as f32))
        .hover(|style| style.bg(colors.element_hover))
    }

    /// 计算可视区域内的节点项
    fn compute_items(
        &mut self,
        layer: &OwnedSyntaxLayer,
        range: Range<usize>,
        cx: &Context<Self>,
    ) -> Vec<Div> {
        let mut items = Vec::new();
        let mut cursor = layer.node().walk();
        let mut descendant_ix = range.start;
        cursor.goto_descendant(descendant_ix);
        let mut depth = cursor.depth();
        let mut visited_children = false;
        while descendant_ix < range.end {
            if visited_children {
                if cursor.goto_next_sibling() {
                    visited_children = false;
                } else if cursor.goto_parent() {
                    depth -= 1;
                } else {
                    break;
                }
            } else {
                items.push(
                    Self::render_node(
                        &cursor,
                        depth,
                        Some(descendant_ix) == self.selected_descendant_ix,
                        cx,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |tree_view, _: &MouseDownEvent, window, cx| {
                            tree_view.update_editor_with_range_for_descendant_ix(
                                descendant_ix,
                                window,
                                cx,
                                &mut |editor, mut range, _, window, cx| {
                                    // 将光标放在节点开头
                                    mem::swap(&mut range.start, &mut range.end);

                                    editor.change_selections(
                                        SelectionEffects::scroll(Autoscroll::newest()),
                                        window,
                                        cx,
                                        |selections| {
                                            selections.select_ranges([range]);
                                        },
                                    );
                                },
                            );
                        }),
                    )
                    .on_mouse_move(cx.listener(
                        move |tree_view, _: &MouseMoveEvent, window, cx| {
                            if tree_view.hovered_descendant_ix != Some(descendant_ix) {
                                tree_view.hovered_descendant_ix = Some(descendant_ix);
                                tree_view.update_editor_with_range_for_descendant_ix(
                                    descendant_ix,
                                    window,
                                    cx,
                                    &mut |editor, range, key, _, cx| {
                                        Self::set_editor_highlights(editor, key, &[range], cx);
                                    },
                                );
                                cx.notify();
                            }
                        },
                    )),
                );
                descendant_ix += 1;
                if cursor.goto_first_child() {
                    depth += 1;
                } else {
                    visited_children = true;
                }
            }
        }
        items
    }

    /// 设置编辑器背景高亮
    fn set_editor_highlights(
        editor: &mut Editor,
        key: usize,
        ranges: &[Range<Anchor>],
        cx: &mut Context<Editor>,
    ) {
        editor.highlight_background(
            HighlightKey::SyntaxTreeView(key),
            ranges,
            |_, theme| theme.colors().editor_document_highlight_write_background,
            cx,
        );
    }

    /// 清除编辑器高亮
    fn clear_editor_highlights(editor: &Entity<Editor>, cx: &mut Context<Self>) {
        let highlight_key = HighlightKey::SyntaxTreeView(cx.entity_id().as_u64() as usize);
        editor.update(cx, |editor, cx| {
            editor.clear_background_highlights(highlight_key, cx);
        });
    }
}

/// 渲染语法树视图
impl Render for SyntaxTreeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_1()
            .bg(cx.theme().colors().editor_background)
            .map(|this| {
                let editor_state = self.editor.as_ref();

                if let Some(layer) = editor_state
                    .and_then(|editor| editor.active_buffer.as_ref())
                    .and_then(|buffer| buffer.active_layer.as_ref())
                {
                    let layer = layer.clone();
                    this.child(
                        uniform_list(
                            "SyntaxTreeView",
                            layer.node().descendant_count(),
                            cx.processor(move |this, range: Range<usize>, _, cx| {
                                this.compute_items(&layer, range, cx)
                            }),
                        )
                        .size_full()
                        .track_scroll(&self.list_scroll_handle)
                        .text_bg(cx.theme().colors().background)
                        .into_any_element(),
                    )
                    .vertical_scrollbar_for(&self.list_scroll_handle, window, cx)
                    .into_any_element()
                } else {
                    let inner_content = v_flex()
                        .items_center()
                        .text_center()
                        .gap_2()
                        .max_w_3_5()
                        .map(|this| {
                            if editor_state.is_some_and(|state| !state.has_language()) {
                                this.child(Label::new("当前编辑器未关联语言"))
                                    .child(
                                        Label::new(concat!(
                                            "请分配语言或",
                                            "切换到其他缓冲区"
                                        ))
                                        .size(LabelSize::Small),
                                    )
                            } else {
                                this.child(Label::new("未绑定编辑器")).child(
                                    Label::new("聚焦编辑器以显示语法树")
                                        .size(LabelSize::Small),
                                )
                            }
                        });

                    this.h_flex()
                        .size_full()
                        .justify_center()
                        .child(inner_content)
                        .into_any_element()
                }
            })
    }
}

impl EventEmitter<()> for SyntaxTreeView {}

/// 实现可聚焦接口
impl Focusable for SyntaxTreeView {
    fn focus_handle(&self, _: &App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

/// 实现工作区项目接口
impl Item for SyntaxTreeView {
    type Event = ();

    fn to_item_events(_: &Self::Event, _: &mut dyn FnMut(workspace::item::ItemEvent)) {}

    /// 标签栏名称
    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        "Syntax Tree".into()
    }

    fn telemetry_event_text(&self) -> Option<&'static str> {
        None
    }

    /// 允许分割
    fn can_split(&self) -> bool {
        true
    }

    /// 分割时克隆视图
    fn clone_on_split(
        &self,
        _: Option<workspace::WorkspaceId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Option<Entity<Self>>>
    where
        Self: Sized,
    {
        Task::ready(Some(cx.new(|cx| {
            let mut clone = Self::new(self.workspace_handle.clone(), None, window, cx);
            if let Some(editor) = &self.editor {
                clone.set_editor(editor.editor.clone(), window, cx)
            }
            clone
        })))
    }

    /// 移除时清除高亮
    fn on_removed(&self, cx: &mut Context<Self>) {
        if let Some(state) = self.editor.as_ref() {
            Self::clear_editor_highlights(&state.editor, cx);
        }
    }
}

impl Default for SyntaxTreeToolbarItemView {
    fn default() -> Self {
        Self::new()
    }
}

impl SyntaxTreeToolbarItemView {
    pub fn new() -> Self {
        Self {
            tree_view: None,
            subscription: None,
        }
    }

    /// 渲染语法层下拉菜单
    fn render_menu(&mut self, cx: &mut Context<Self>) -> Option<PopoverMenu<ContextMenu>> {
        let tree_view = self.tree_view.as_ref()?;
        let tree_view = tree_view.read(cx);

        let editor_state = tree_view.editor.as_ref()?;
        let buffer_state = editor_state.active_buffer.as_ref()?;
        let active_layer = buffer_state.active_layer.clone()?;
        let active_buffer = buffer_state.buffer.read(cx).snapshot();

        let view = cx.weak_entity();
        Some(
            PopoverMenu::new("Syntax Tree")
                .trigger(Self::render_header(&active_layer))
                .menu(move |window, cx| {
                    ContextMenu::build(window, cx, |mut menu, _, _| {
                        for (layer_ix, layer) in active_buffer.syntax_layers().enumerate() {
                            let view = view.clone();
                            menu = menu.entry(
                                format!(
                                    "{} {}",
                                    layer.language.name(),
                                    format_node_range(layer.node())
                                ),
                                None,
                                move |window, cx| {
                                    view.update(cx, |view, cx| {
                                        view.select_layer(layer_ix, window, cx);
                                    })
                                    .ok();
                                },
                            );
                        }
                        menu
                    })
                    .into()
                }),
        )
    }

    /// 切换语法解析层
    fn select_layer(
        &mut self,
        layer_ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<()> {
        let tree_view = self.tree_view.as_ref()?;
        tree_view.update(cx, |view, cx| {
            let editor_state = view.editor.as_mut()?;
            let buffer_state = editor_state.active_buffer.as_mut()?;
            let snapshot = buffer_state.buffer.read(cx).snapshot();
            let layer = snapshot.syntax_layers().nth(layer_ix)?;
            buffer_state.active_layer = Some(layer.to_owned());
            view.selected_descendant_ix = None;
            cx.notify();
            view.focus_handle.focus(window, cx);
            Some(())
        })
    }

    /// 渲染工具栏头部
    fn render_header(active_layer: &OwnedSyntaxLayer) -> ButtonLike {
        ButtonLike::new("syntax tree header")
            .child(Label::new(active_layer.language.name()))
            .child(Label::new(format_node_range(active_layer.node())))
    }

    /// 渲染更新按钮
    fn render_update_button(&mut self, cx: &mut Context<Self>) -> Option<IconButton> {
        self.tree_view.as_ref().and_then(|view| {
            view.update(cx, |view, cx| {
                view.last_active_editor.as_ref().map(|editor| {
                    IconButton::new("syntax-view-update", IconName::RotateCw)
                        .tooltip({
                            let active_tab_name = editor.read_with(cx, |editor, cx| {
                                editor.tab_content_text(Default::default(), cx)
                            });

                            Tooltip::text(format!("更新视图到 '{active_tab_name}'"))
                        })
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.update_active_editor(&Default::default(), window, cx);
                        }))
                })
            })
        })
    }
}

/// 格式化节点位置信息 [行:列 - 行:列]
fn format_node_range(node: Node) -> String {
    let start = node.start_position();
    let end = node.end_position();
    format!(
        "[{}:{} - {}:{}]",
        start.row + 1,
        start.column + 1,
        end.row + 1,
        end.column + 1,
    )
}

/// 渲染工具栏
impl Render for SyntaxTreeToolbarItemView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_1()
            .children(self.render_menu(cx))
            .children(self.render_update_button(cx))
    }
}

impl EventEmitter<ToolbarItemEvent> for SyntaxTreeToolbarItemView {}

/// 实现工具栏项目接口
impl ToolbarItemView for SyntaxTreeToolbarItemView {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ToolbarItemLocation {
        if let Some(item) = active_pane_item
            && let Some(view) = item.downcast::<SyntaxTreeView>()
        {
            self.tree_view = Some(view.clone());
            self.subscription = Some(cx.observe_in(&view, window, |_, _, _, cx| cx.notify()));
            return ToolbarItemLocation::PrimaryLeft;
        }
        self.tree_view = None;
        self.subscription = None;
        ToolbarItemLocation::Hidden
    }
}