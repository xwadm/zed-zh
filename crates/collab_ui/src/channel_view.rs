use anyhow::Result;
use call::ActiveCall;
use channel::{Channel, ChannelBuffer, ChannelBufferEvent, ChannelStore};
use client::{
    ChannelId, Collaborator, ParticipantIndex,
    proto::{self, PeerId},
};
use collections::HashMap;
use editor::{
    CollaborationHub, DisplayPoint, Editor, EditorEvent, SelectionEffects,
    display_map::ToDisplayPoint, scroll::Autoscroll,
};
use gpui::{
    App, ClipboardItem, Context, Entity, EventEmitter, Focusable, Pixels, Point, Render,
    Subscription, Task, VisualContext as _, WeakEntity, Window, actions,
};
use project::Project;
use rpc::proto::ChannelVisibility;
use std::{
    any::{Any, TypeId},
    sync::Arc,
};
use ui::prelude::*;
use util::ResultExt;
use workspace::{CollaboratorId, item::TabContentParams};
use workspace::{
    ItemNavHistory, Pane, SaveIntent, Toast, ViewId, Workspace, WorkspaceId,
    item::{FollowableItem, Item, ItemEvent},
    searchable::SearchableItemHandle,
};
use workspace::{item::Dedup, notifications::NotificationId};

actions!(
    collab,
    [
        /// 复制频道缓冲区当前位置的链接
        CopyLink
    ]
);

/// 注册频道视图为可跟随视图
pub fn init(cx: &mut App) {
    workspace::FollowableViewRegistry::register::<ChannelView>(cx)
}

/// 频道视图，承载频道笔记的编辑器界面
pub struct ChannelView {
    pub editor: Entity<Editor>,
    workspace: WeakEntity<Workspace>,
    project: Entity<Project>,
    channel_store: Entity<ChannelStore>,
    channel_buffer: Entity<ChannelBuffer>,
    remote_id: Option<ViewId>,
    _editor_event_subscription: Subscription,
    _reparse_subscription: Option<Subscription>,
}

impl ChannelView {
    /// 打开频道笔记视图并添加到活动窗格
    pub fn open(
        channel_id: ChannelId,
        link_position: Option<String>,
        workspace: Entity<Workspace>,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Entity<Self>>> {
        let pane = workspace.read(cx).active_pane().clone();
        let channel_view = Self::open_in_pane(
            channel_id,
            link_position,
            pane.clone(),
            workspace,
            window,
            cx,
        );
        window.spawn(cx, async move |cx| {
            let channel_view = channel_view.await?;
            pane.update_in(cx, |pane, window, cx| {
                telemetry::event!(
                    "Channel Notes Opened",
                    channel_id,
                    room_id = ActiveCall::global(cx)
                        .read(cx)
                        .room()
                        .map(|r| r.read(cx).id())
                );
                pane.add_item(Box::new(channel_view.clone()), true, true, None, window, cx);
            })?;
            anyhow::Ok(channel_view)
        })
    }

    /// 在指定窗格中打开频道笔记视图
    pub fn open_in_pane(
        channel_id: ChannelId,
        link_position: Option<String>,
        pane: Entity<Pane>,
        workspace: Entity<Workspace>,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Entity<Self>>> {
        let channel_view = Self::load(channel_id, workspace, window, cx);
        window.spawn(cx, async move |cx| {
            let channel_view = channel_view.await?;

            pane.update_in(cx, |pane, window, cx| {
                let buffer_id = channel_view.read(cx).channel_buffer.read(cx).remote_id(cx);

                let existing_view = pane
                    .items_of_type::<Self>()
                    .find(|view| view.read(cx).channel_buffer.read(cx).remote_id(cx) == buffer_id);

                // 如果频道缓冲区已在当前窗格打开，直接返回现有视图
                if let Some(existing_view) = existing_view.clone()
                    && existing_view.read(cx).channel_buffer == channel_view.read(cx).channel_buffer
                {
                    if let Some(link_position) = link_position {
                        existing_view.update(cx, |channel_view, cx| {
                            channel_view.focus_position_from_link(link_position, true, window, cx)
                        });
                    }
                    return existing_view;
                }

                // 如果窗格包含该频道缓冲区的断开连接视图，替换它
                if let Some(existing_item) = existing_view
                    && let Some(ix) = pane.index_for_item(&existing_item)
                {
                    pane.close_item_by_id(existing_item.entity_id(), SaveIntent::Skip, window, cx)
                        .detach();
                    pane.add_item(
                        Box::new(channel_view.clone()),
                        true,
                        true,
                        Some(ix),
                        window,
                        cx,
                    );
                }

                if let Some(link_position) = link_position {
                    channel_view.update(cx, |channel_view, cx| {
                        channel_view.focus_position_from_link(link_position, true, window, cx)
                    });
                }

                channel_view
            })
        })
    }

    /// 加载频道缓冲区并创建视图
    pub fn load(
        channel_id: ChannelId,
        workspace: Entity<Workspace>,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Entity<Self>>> {
        let weak_workspace = workspace.downgrade();
        let workspace = workspace.read(cx);
        let project = workspace.project().to_owned();
        let channel_store = ChannelStore::global(cx);
        let language_registry = workspace.app_state().languages.clone();
        let markdown = language_registry.language_for_name("Markdown");
        let channel_buffer =
            channel_store.update(cx, |store, cx| store.open_channel_buffer(channel_id, cx));

        window.spawn(cx, async move |cx| {
            let channel_buffer = channel_buffer.await?;
            let markdown = markdown.await.log_err();

            channel_buffer.update(cx, |channel_buffer, cx| {
                channel_buffer.buffer().update(cx, |buffer, cx| {
                    buffer.set_language_registry(language_registry);
                    let Some(markdown) = markdown else {
                        return;
                    };
                    buffer.set_language(Some(markdown), cx);
                })
            });

            cx.new_window_entity(|window, cx| {
                let mut this = Self::new(
                    project,
                    weak_workspace,
                    channel_store,
                    channel_buffer,
                    window,
                    cx,
                );
                this.acknowledge_buffer_version(cx);
                this
            })
        })
    }

    /// 初始化频道视图
    pub fn new(
        project: Entity<Project>,
        workspace: WeakEntity<Workspace>,
        channel_store: Entity<ChannelStore>,
        channel_buffer: Entity<ChannelBuffer>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let buffer = channel_buffer.read(cx).buffer();
        let this = cx.entity().downgrade();
        let editor = cx.new(|cx| {
            let mut editor = Editor::for_buffer(buffer, None, window, cx);
            editor.set_collaboration_hub(Box::new(ChannelBufferCollaborationHub(
                channel_buffer.clone(),
            )));
            editor.set_custom_context_menu(move |_, position, window, cx| {
                let this = this.clone();
                Some(ui::ContextMenu::build(window, cx, move |menu, _, _| {
                    menu.entry("复制章节链接", None, move |window, cx| {
                        this.update(cx, |this, cx| {
                            this.copy_link_for_position(position, window, cx)
                        })
                        .ok();
                    })
                }))
            });
            editor.set_show_bookmarks(false, cx);
            editor.set_show_breakpoints(false, cx);
            editor.set_show_runnables(false, cx);
            editor
        });
        let _editor_event_subscription =
            cx.subscribe(&editor, |_, _, e: &EditorEvent, cx| cx.emit(e.clone()));

        cx.subscribe_in(&channel_buffer, window, Self::handle_channel_buffer_event)
            .detach();

        Self {
            editor,
            workspace,
            project,
            channel_store,
            channel_buffer,
            remote_id: None,
            _editor_event_subscription,
            _reparse_subscription: None,
        }
    }

    /// 根据链接定位并聚焦到指定位置
    fn focus_position_from_link(
        &mut self,
        position: String,
        first_attempt: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let position = Channel::slug(&position).to_lowercase();
        let snapshot = self
            .editor
            .update(cx, |editor, cx| editor.snapshot(window, cx));

        if let Some(outline) = snapshot.buffer_snapshot().outline(None)
            && let Some(item) = outline
                .items
                .iter()
                .find(|item| &Channel::slug(&item.text).to_lowercase() == &position)
        {
            self.editor.update(cx, |editor, cx| {
                editor.change_selections(
                    SelectionEffects::scroll(Autoscroll::focused()),
                    window,
                    cx,
                    |s| s.replace_cursors_with(|map| vec![item.range.start.to_display_point(map)]),
                )
            });
            return;
        }

        if !first_attempt {
            return;
        }
        self._reparse_subscription = Some(cx.subscribe_in(
            &self.editor,
            window,
            move |this, _, e: &EditorEvent, window, cx| {
                match e {
                    EditorEvent::Reparsed(_) => {
                        this.focus_position_from_link(position.clone(), false, window, cx);
                        this._reparse_subscription.take();
                    }
                    EditorEvent::Edited { .. } | EditorEvent::SelectionsChanged { local: true } => {
                        this._reparse_subscription.take();
                    }
                    _ => {}
                };
            },
        ));
    }

    /// 复制链接动作处理
    fn copy_link(&mut self, _: &CopyLink, window: &mut Window, cx: &mut Context<Self>) {
        let position = self.editor.update(cx, |editor, cx| {
            editor
                .selections
                .newest_display(&editor.display_snapshot(cx))
                .start
        });
        self.copy_link_for_position(position, window, cx)
    }

    /// 复制指定位置的链接
    fn copy_link_for_position(
        &self,
        position: DisplayPoint,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let snapshot = self
            .editor
            .update(cx, |editor, cx| editor.snapshot(window, cx));

        let mut closest_heading = None;

        if let Some(outline) = snapshot.buffer_snapshot().outline(None) {
            for item in outline.items {
                if item.range.start.to_display_point(&snapshot) > position {
                    break;
                }
                closest_heading = Some(item);
            }
        }

        let Some(channel) = self.channel(cx) else {
            return;
        };

        let link = channel.notes_link(closest_heading.map(|heading| heading.text), cx);
        cx.write_to_clipboard(ClipboardItem::new_string(link));
        self.workspace
            .update(cx, |workspace, cx| {
                struct CopyLinkForPositionToast;

                workspace.show_toast(
                    Toast::new(
                        NotificationId::unique::<CopyLinkForPositionToast>(),
                        "链接已复制到剪贴板",
                    ),
                    cx,
                );
            })
            .ok();
    }

    /// 获取当前关联的频道
    pub fn channel(&self, cx: &App) -> Option<Arc<Channel>> {
        self.channel_buffer.read(cx).channel(cx)
    }

    /// 处理频道缓冲区事件
    fn handle_channel_buffer_event(
        &mut self,
        _: &Entity<ChannelBuffer>,
        event: &ChannelBufferEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ChannelBufferEvent::Disconnected => self.editor.update(cx, |editor, cx| {
                editor.set_read_only(true);
                cx.notify();
            }),
            ChannelBufferEvent::Connected => self.editor.update(cx, |editor, cx| {
                editor.set_read_only(false);
                cx.notify();
            }),
            ChannelBufferEvent::ChannelChanged => {
                self.editor.update(cx, |_, cx| {
                    cx.emit(editor::EditorEvent::TitleChanged);
                    cx.notify()
                });
            }
            ChannelBufferEvent::BufferEdited => {
                if self.editor.read(cx).is_focused(window) {
                    self.acknowledge_buffer_version(cx);
                } else {
                    self.channel_store.update(cx, |store, cx| {
                        let channel_buffer = self.channel_buffer.read(cx);
                        store.update_latest_notes_version(
                            channel_buffer.channel_id,
                            channel_buffer.epoch(),
                            &channel_buffer.buffer().read(cx).version(),
                            cx,
                        )
                    });
                }
            }
            ChannelBufferEvent::CollaboratorsChanged => {}
        }
    }

    /// 确认缓冲区版本
    fn acknowledge_buffer_version(&mut self, cx: &mut Context<ChannelView>) {
        self.channel_store.update(cx, |store, cx| {
            let channel_buffer = self.channel_buffer.read(cx);
            store.acknowledge_notes_version(
                channel_buffer.channel_id,
                channel_buffer.epoch(),
                &channel_buffer.buffer().read(cx).version(),
                cx,
            )
        });
        self.channel_buffer.update(cx, |buffer, cx| {
            buffer.acknowledge_buffer_version(cx);
        });
    }

    /// 获取频道名称和状态
    fn get_channel(&self, cx: &App) -> (SharedString, Option<SharedString>) {
        if let Some(channel) = self.channel(cx) {
            let status = match (
                self.channel_buffer.read(cx).buffer().read(cx).read_only(),
                self.channel_buffer.read(cx).is_connected(),
            ) {
                (false, true) => None,
                (true, true) => Some("只读"),
                (_, false) => Some("已断开连接"),
            };

            (channel.name.clone(), status.map(Into::into))
        } else {
            ("<未知>".into(), Some("已断开连接".into()))
        }
    }
}

impl EventEmitter<EditorEvent> for ChannelView {}

impl Render for ChannelView {
    /// 渲染频道视图
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .on_action(cx.listener(Self::copy_link))
            .child(self.editor.clone())
    }
}

impl Focusable for ChannelView {
    fn focus_handle(&self, cx: &App) -> gpui::FocusHandle {
        self.editor.read(cx).focus_handle(cx)
    }
}

impl Item for ChannelView {
    type Event = EditorEvent;

    fn act_as_type<'a>(
        &'a self,
        type_id: TypeId,
        self_handle: &'a Entity<Self>,
        _: &'a App,
    ) -> Option<gpui::AnyEntity> {
        if type_id == TypeId::of::<Self>() {
            Some(self_handle.clone().into())
        } else if type_id == TypeId::of::<Editor>() {
            Some(self.editor.clone().into())
        } else {
            None
        }
    }

    /// 获取标签图标
    fn tab_icon(&self, _: &Window, cx: &App) -> Option<Icon> {
        let channel = self.channel(cx)?;
        let icon = match channel.visibility {
            ChannelVisibility::Public => IconName::Public,
            ChannelVisibility::Members => IconName::Hash,
        };

        Some(Icon::new(icon))
    }

    fn tab_content_text(&self, _detail: usize, cx: &App) -> SharedString {
        let (name, status) = self.get_channel(cx);
        if let Some(status) = status {
            format!("{name} - {status}").into()
        } else {
            name
        }
    }

    /// 渲染标签内容
    fn tab_content(&self, params: TabContentParams, _: &Window, cx: &App) -> gpui::AnyElement {
        let (name, status) = self.get_channel(cx);
        h_flex()
            .gap_2()
            .child(
                Label::new(name)
                    .color(params.text_color())
                    .when(params.preview, |this| this.italic()),
            )
            .when_some(status, |element, status| {
                element.child(
                    Label::new(status)
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                )
            })
            .into_any_element()
    }

    fn telemetry_event_text(&self) -> Option<&'static str> {
        None
    }

    fn can_split(&self) -> bool {
        true
    }

    /// 分割视图时克隆
    fn clone_on_split(
        &self,
        _: Option<WorkspaceId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Option<Entity<Self>>> {
        Task::ready(Some(cx.new(|cx| {
            Self::new(
                self.project.clone(),
                self.workspace.clone(),
                self.channel_store.clone(),
                self.channel_buffer.clone(),
                window,
                cx,
            )
        })))
    }

    fn navigate(
        &mut self,
        data: Arc<dyn Any + Send>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.editor
            .update(cx, |editor, cx| editor.navigate(data, window, cx))
    }

    fn deactivated(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor
            .update(cx, |item, cx| item.deactivated(window, cx))
    }

    fn set_nav_history(
        &mut self,
        history: ItemNavHistory,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |editor, cx| {
            Item::set_nav_history(editor, history, window, cx)
        })
    }

    fn as_searchable(&self, _: &Entity<Self>, _: &App) -> Option<Box<dyn SearchableItemHandle>> {
        Some(Box::new(self.editor.clone()))
    }

    fn show_toolbar(&self) -> bool {
        true
    }

    fn pixel_position_of_cursor(&self, cx: &App) -> Option<Point<Pixels>> {
        self.editor.read(cx).pixel_position_of_cursor(cx)
    }

    fn to_item_events(event: &EditorEvent, f: &mut dyn FnMut(ItemEvent)) {
        Editor::to_item_events(event, f)
    }
}

impl FollowableItem for ChannelView {
    fn remote_id(&self) -> Option<workspace::ViewId> {
        self.remote_id
    }

    /// 转换为协议状态
    fn to_state_proto(&self, window: &mut Window, cx: &mut App) -> Option<proto::view::Variant> {
        let (is_connected, channel_id) = {
            let channel_buffer = self.channel_buffer.read(cx);
            (channel_buffer.is_connected(), channel_buffer.channel_id.0)
        };
        if !is_connected {
            return None;
        }

        let editor_proto = self
            .editor
            .update(cx, |editor, cx| editor.to_state_proto(window, cx));
        Some(proto::view::Variant::ChannelView(
            proto::view::ChannelView {
                channel_id,
                editor: if let Some(proto::view::Variant::Editor(proto)) = editor_proto {
                    Some(proto)
                } else {
                    None
                },
            },
        ))
    }

    /// 从协议状态恢复视图
    fn from_state_proto(
        workspace: Entity<workspace::Workspace>,
        remote_id: workspace::ViewId,
        state: &mut Option<proto::view::Variant>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<gpui::Task<anyhow::Result<Entity<Self>>>> {
        let Some(proto::view::Variant::ChannelView(_)) = state else {
            return None;
        };
        let Some(proto::view::Variant::ChannelView(state)) = state.take() else {
            unreachable!()
        };

        let open = ChannelView::load(ChannelId(state.channel_id), workspace, window, cx);

        Some(window.spawn(cx, async move |cx| {
            let this = open.await?;

            let task = this.update_in(cx, |this, window, cx| {
                this.remote_id = Some(remote_id);

                if let Some(state) = state.editor {
                    Some(this.editor.update(cx, |editor, cx| {
                        editor.apply_update_proto(
                            &this.project,
                            proto::update_view::Variant::Editor(proto::update_view::Editor {
                                selections: state.selections,
                                pending_selection: state.pending_selection,
                                scroll_top_anchor: state.scroll_top_anchor,
                                scroll_x: state.scroll_x,
                                scroll_y: state.scroll_y,
                                ..Default::default()
                            }),
                            window,
                            cx,
                        )
                    }))
                } else {
                    None
                }
            })?;

            if let Some(task) = task {
                task.await?;
            }

            Ok(this)
        }))
    }

    fn add_event_to_update_proto(
        &self,
        event: &EditorEvent,
        update: &mut Option<proto::update_view::Variant>,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        self.editor.update(cx, |editor, cx| {
            editor.add_event_to_update_proto(event, update, window, cx)
        })
    }

    fn apply_update_proto(
        &mut self,
        project: &Entity<Project>,
        message: proto::update_view::Variant,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Task<anyhow::Result<()>> {
        self.editor.update(cx, |editor, cx| {
            editor.apply_update_proto(project, message, window, cx)
        })
    }

    fn set_leader_id(
        &mut self,
        leader_id: Option<CollaboratorId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor
            .update(cx, |editor, cx| editor.set_leader_id(leader_id, window, cx))
    }

    fn is_project_item(&self, _window: &Window, _cx: &App) -> bool {
        false
    }

    fn to_follow_event(event: &Self::Event) -> Option<workspace::item::FollowEvent> {
        Editor::to_follow_event(event)
    }

    /// 视图去重逻辑
    fn dedup(&self, existing: &Self, _: &Window, cx: &App) -> Option<Dedup> {
        let existing = existing.channel_buffer.read(cx);
        if self.channel_buffer.read(cx).channel_id == existing.channel_id {
            if existing.is_connected() {
                Some(Dedup::KeepExisting)
            } else {
                Some(Dedup::ReplaceExisting)
            }
        } else {
            None
        }
    }
}

/// 频道缓冲区协作中心
struct ChannelBufferCollaborationHub(Entity<ChannelBuffer>);

impl CollaborationHub for ChannelBufferCollaborationHub {
    fn collaborators<'a>(&self, cx: &'a App) -> &'a HashMap<PeerId, Collaborator> {
        self.0.read(cx).collaborators()
    }

    fn user_participant_indices<'a>(&self, cx: &'a App) -> &'a HashMap<u64, ParticipantIndex> {
        self.0.read(cx).user_store().read(cx).participant_indices()
    }

    fn user_names(&self, cx: &App) -> HashMap<u64, SharedString> {
        let user_ids = self.collaborators(cx).values().map(|c| c.user_id);
        self.0
            .read(cx)
            .user_store()
            .read(cx)
            .participant_names(user_ids, cx)
    }
}