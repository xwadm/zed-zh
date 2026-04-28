use crate::{
    DIAGNOSTICS_UPDATE_DEBOUNCE, IncludeWarnings, ToggleWarnings, context_range_for_entry,
    diagnostic_renderer::{DiagnosticBlock, DiagnosticRenderer},
    toolbar_controls::DiagnosticsToolbarEditor,
};
use anyhow::Result;
use collections::HashMap;
use editor::{
    Editor, EditorEvent, ExcerptRange, MultiBuffer, PathKey,
    display_map::{BlockPlacement, BlockProperties, BlockStyle, CustomBlockId},
    multibuffer_context_lines,
};
use gpui::{
    AnyElement, App, AppContext, Context, Entity, EntityId, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, ParentElement, Render, SharedString, Styled, Subscription,
    Task, WeakEntity, Window, actions, div,
};
use language::{Buffer, Capability, DiagnosticEntry, DiagnosticEntryRef, Point};
use project::{
    DiagnosticSummary, Event, Project, ProjectItem, ProjectPath,
    project_settings::{DiagnosticSeverity, ProjectSettings},
};
use settings::Settings;
use std::{
    any::{Any, TypeId},
    cmp::{self, Ordering},
    ops::Range,
    sync::Arc,
};
use text::{Anchor, BufferSnapshot, OffsetRangeExt};
use ui::{Button, ButtonStyle, Icon, IconName, Label, Tooltip, h_flex, prelude::*};
use workspace::{
    ItemHandle, ItemNavHistory, Workspace,
    item::{Item, ItemEvent, TabContentParams},
};

actions!(
    diagnostics,
    [
        /// 为当前聚焦的文件打开项目诊断视图。
        DeployCurrentFile,
    ]
);

/// `BufferDiagnosticsEditor` 专门用于处理单个缓冲区的诊断信息，
/// 仅显示存在诊断的缓冲区片段。
pub(crate) struct BufferDiagnosticsEditor {
    pub project: Entity<Project>,
    focus_handle: FocusHandle,
    editor: Entity<Editor>,
    /// `BufferDiagnosticsEditor` 中当前的诊断条目。用于快速比较更新后的诊断，
    /// 以确认是否有变化。
    pub(crate) diagnostics: Vec<DiagnosticEntry<Anchor>>,
    /// 用于在编辑器中，紧邻诊断来源片段显示诊断内容的块。
    blocks: Vec<CustomBlockId>,
    /// 包含所有存在诊断的片段的 MultiBuffer，这些片段将在编辑器中渲染。
    multibuffer: Entity<MultiBuffer>,
    /// 编辑器正在为其显示诊断和片段的缓冲区。
    buffer: Option<Entity<Buffer>>,
    /// 编辑器正在为其显示诊断的路径。
    project_path: ProjectPath,
    /// 该路径上警告和错误数量的摘要。用于在标签页内容中显示警告和错误数量。
    summary: DiagnosticSummary,
    /// 是否在编辑器中显示的诊断列表中包含警告。
    pub(crate) include_warnings: bool,
    /// 跟踪是否已有后台任务在更新片段，以避免为此触发多个任务。
    pub(crate) update_excerpts_task: Option<Task<Result<()>>>,
    /// 项目的订阅，负责处理与诊断相关的事件。
    _subscription: Subscription,
}

impl BufferDiagnosticsEditor {
    /// 创建 `BufferDiagnosticsEditor` 的新实例，之后可通过将其添加到面板中来展示。
    pub fn new(
        project_path: ProjectPath,
        project_handle: Entity<Project>,
        buffer: Option<Entity<Buffer>>,
        include_warnings: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // 订阅与诊断相关的项目事件，以便 `BufferDiagnosticsEditor` 可以相应更新其状态。
        let project_event_subscription = cx.subscribe_in(
            &project_handle,
            window,
            |buffer_diagnostics_editor, _project, event, window, cx| match event {
                Event::DiskBasedDiagnosticsStarted { .. } => {
                    cx.notify();
                }
                Event::DiskBasedDiagnosticsFinished { .. } => {
                    buffer_diagnostics_editor.update_all_excerpts(window, cx);
                }
                Event::DiagnosticsUpdated {
                    paths,
                    language_server_id,
                } => {
                    // 当诊断更新时，`BufferDiagnosticsEditor` 应仅在其
                    // `project_path` 匹配路径之一时才更新自身状态，
                    // 否则应忽略该事件。
                    if paths.contains(&buffer_diagnostics_editor.project_path) {
                        buffer_diagnostics_editor.update_diagnostic_summary(cx);

                        if buffer_diagnostics_editor.editor.focus_handle(cx).contains_focused(window, cx) || buffer_diagnostics_editor.focus_handle.contains_focused(window, cx) {
                            log::debug!("语言服务器 {language_server_id} 的诊断已更新。记录变更");
                        } else {
                            log::debug!("语言服务器 {language_server_id} 的诊断已更新。正在更新片段");
                            buffer_diagnostics_editor.update_all_excerpts(window, cx);
                        }
                    }
                }
                _ => {}
            },
        );

        let focus_handle = cx.focus_handle();

        cx.on_focus_in(
            &focus_handle,
            window,
            |buffer_diagnostics_editor, window, cx| buffer_diagnostics_editor.focus_in(window, cx),
        )
        .detach();

        cx.on_focus_out(
            &focus_handle,
            window,
            |buffer_diagnostics_editor, _event, window, cx| {
                buffer_diagnostics_editor.focus_out(window, cx)
            },
        )
        .detach();

        let summary = project_handle
            .read(cx)
            .diagnostic_summary_for_path(&project_path, cx);

        let multibuffer = cx.new(|cx| MultiBuffer::new(project_handle.read(cx).capability()));
        let max_severity = Self::max_diagnostics_severity(include_warnings);
        let editor = cx.new(|cx| {
            let mut editor = Editor::for_multibuffer(
                multibuffer.clone(),
                Some(project_handle.clone()),
                window,
                cx,
            );
            editor.set_vertical_scroll_margin(5, cx);
            editor.disable_inline_diagnostics();
            editor.set_max_diagnostics_severity(max_severity, cx);
            editor.set_all_diagnostics_active(cx);
            editor
        });

        // 订阅由编辑器触发的事件，以便正确更新缓冲区的片段。
        cx.subscribe_in(
            &editor,
            window,
            |buffer_diagnostics_editor, _editor, event: &EditorEvent, window, cx| {
                cx.emit(event.clone());

                match event {
                    // 如果用户尝试聚焦到编辑器，但缓冲区实际上没有任何片段，
                    // 则将焦点重新放回 `BufferDiagnosticsEditor` 实例上。
                    EditorEvent::Focused => {
                        if buffer_diagnostics_editor.multibuffer.read(cx).is_empty() {
                            window.focus(&buffer_diagnostics_editor.focus_handle, cx);
                        }
                    }
                    EditorEvent::Blurred => {
                        buffer_diagnostics_editor.update_all_excerpts(window, cx)
                    }
                    _ => {}
                }
            },
        )
        .detach();

        let diagnostics = vec![];
        let update_excerpts_task = None;
        let mut buffer_diagnostics_editor = Self {
            project: project_handle,
            focus_handle,
            editor,
            diagnostics,
            blocks: Default::default(),
            multibuffer,
            buffer,
            project_path,
            summary,
            include_warnings,
            update_excerpts_task,
            _subscription: project_event_subscription,
        };

        buffer_diagnostics_editor.update_all_diagnostics(window, cx);
        buffer_diagnostics_editor
    }

    fn deploy(
        workspace: &mut Workspace,
        _: &DeployCurrentFile,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        // 通过查找活动编辑器并确定其缓冲区的项目路径，来获取当前打开的路径。
        // 如果没有活跃的编辑器具有项目路径，则避免部署缓冲区诊断视图。
        if let Some(editor) = workspace.active_item_as::<Editor>(cx)
            && let Some(project_path) = editor.project_path(cx)
        {
            // 检查是否已存在同一路径的 `BufferDiagnosticsEditor` 标签页，
            // 如果存在，则聚焦到该标签页，而非创建新的。
            let existing_editor = workspace
                .items_of_type::<BufferDiagnosticsEditor>(cx)
                .find(|editor| editor.read(cx).project_path == project_path);

            if let Some(editor) = existing_editor {
                workspace.activate_item(&editor, true, true, window, cx);
            } else {
                let include_warnings = match cx.try_global::<IncludeWarnings>() {
                    Some(include_warnings) => include_warnings.0,
                    None => ProjectSettings::get_global(cx).diagnostics.include_warnings,
                };

                let item = cx.new(|cx| {
                    Self::new(
                        project_path,
                        workspace.project().clone(),
                        editor.read(cx).buffer().read(cx).as_singleton(),
                        include_warnings,
                        window,
                        cx,
                    )
                });

                workspace.add_item_to_active_pane(Box::new(item), None, true, window, cx);
            }
        }
    }

    pub fn register(
        workspace: &mut Workspace,
        _window: Option<&mut Window>,
        _: &mut Context<Workspace>,
    ) {
        workspace.register_action(Self::deploy);
    }

    fn update_all_diagnostics(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.update_all_excerpts(window, cx);
    }

    fn update_diagnostic_summary(&mut self, cx: &mut Context<Self>) {
        let project = self.project.read(cx);

        self.summary = project.diagnostic_summary_for_path(&self.project_path, cx);
    }

    /// 将编辑器中的片段和诊断块的更新任务加入队列。
    pub(crate) fn update_all_excerpts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 如果已有任务在更新片段，则提前返回，等待其他任务完成。
        if self.update_excerpts_task.is_some() {
            return;
        }

        let buffer = self.buffer.clone();

        self.update_excerpts_task = Some(cx.spawn_in(window, async move |editor, cx| {
            cx.background_executor()
                .timer(DIAGNOSTICS_UPDATE_DEBOUNCE)
                .await;

            if let Some(buffer) = buffer {
                editor
                    .update_in(cx, |editor, window, cx| {
                        editor.update_excerpts(buffer, window, cx)
                    })?
                    .await?;
            };

            let _ = editor.update(cx, |editor, cx| {
                editor.update_excerpts_task = None;
                cx.notify();
            });

            Ok(())
        }));
    }

    /// 为单个缓冲区更新 `BufferDiagnosticsEditor` 中的片段。
    fn update_excerpts(
        &mut self,
        buffer: Entity<Buffer>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        let was_empty = self.multibuffer.read(cx).is_empty();
        let multibuffer_context = multibuffer_context_lines(cx);
        let buffer_snapshot = buffer.read(cx).snapshot();
        let buffer_snapshot_max = buffer_snapshot.max_point();
        let max_severity = Self::max_diagnostics_severity(self.include_warnings)
            .into_lsp()
            .unwrap_or(lsp::DiagnosticSeverity::WARNING);

        cx.spawn_in(window, async move |buffer_diagnostics_editor, mut cx| {
            // 获取整个缓冲区的诊断信息（`Point::zero()..buffer_snapshot.max_point()`），
            // 以确认诊断是否发生变化，若未变化则提前返回，因为无内容需要更新。
            let diagnostics = buffer_snapshot
                .diagnostics_in_range::<_, Anchor>(Point::zero()..buffer_snapshot_max, false)
                .collect::<Vec<_>>();

            let unchanged =
                buffer_diagnostics_editor.update(cx, |buffer_diagnostics_editor, _cx| {
                    if buffer_diagnostics_editor
                        .diagnostics_are_unchanged(&diagnostics, &buffer_snapshot)
                    {
                        return true;
                    }

                    buffer_diagnostics_editor.set_diagnostics(&diagnostics);
                    return false;
                })?;

            if unchanged {
                return Ok(());
            }

            // 将诊断按组 ID 映射到对应的 DiagnosticEntry 向量。
            let mut grouped: HashMap<usize, Vec<_>> = HashMap::default();
            for entry in diagnostics {
                grouped
                    .entry(entry.diagnostic.group_id)
                    .or_default()
                    .push(DiagnosticEntryRef {
                        range: entry.range.to_point(&buffer_snapshot),
                        diagnostic: entry.diagnostic,
                    })
            }

            let mut blocks: Vec<DiagnosticBlock> = Vec::new();
            for (_, group) in grouped {
                // 如果该组的最低严重程度高于允许的最大严重程度，或者根本没有严重程度，
                // 则跳过此组。
                if group
                    .iter()
                    .map(|d| d.diagnostic.severity)
                    .min()
                    .is_none_or(|severity| severity > max_severity)
                {
                    continue;
                }

                let languages = buffer_diagnostics_editor
                    .read_with(cx, |b, cx| b.project.read(cx).languages().clone())
                    .ok();

                let diagnostic_blocks = cx.update(|_window, cx| {
                    DiagnosticRenderer::diagnostic_blocks_for_group(
                        group,
                        buffer_snapshot.remote_id(),
                        Some(Arc::new(buffer_diagnostics_editor.clone())),
                        languages,
                        cx,
                    )
                })?;

                // 对于要在编辑器中显示的每个诊断块，确定其在块列表中的顺序。
                //
                // 排序规则如下：
                // 1. 起始位置较小的块排在前面。
                // 2. 如果两个块的起始位置相同，则结束位置较大的块排在前面。
                for diagnostic_block in diagnostic_blocks {
                    let index = blocks.partition_point(|probe| {
                        match probe
                            .initial_range
                            .start
                            .cmp(&diagnostic_block.initial_range.start)
                        {
                            Ordering::Less => true,
                            Ordering::Greater => false,
                            Ordering::Equal => {
                                probe.initial_range.end > diagnostic_block.initial_range.end
                            }
                        }
                    });

                    blocks.insert(index, diagnostic_block);
                }
            }

            // 为当前缓冲区的诊断构建片段范围，以便后续用这些范围更新编辑器中显示的片段。
            // 这通过遍历诊断块列表，并确定每个诊断块所覆盖的范围来实现。
            let mut excerpt_ranges: Vec<ExcerptRange<_>> = Vec::new();

            for diagnostic_block in blocks.iter() {
                let excerpt_range = context_range_for_entry(
                    diagnostic_block.initial_range.clone(),
                    multibuffer_context,
                    buffer_snapshot.clone(),
                    &mut cx,
                )
                .await;
                let initial_range = buffer_snapshot
                    .anchor_after(diagnostic_block.initial_range.start)
                    ..buffer_snapshot.anchor_before(diagnostic_block.initial_range.end);

                let bin_search = |probe: &ExcerptRange<text::Anchor>| {
                    let context_start = || {
                        probe
                            .context
                            .start
                            .cmp(&excerpt_range.start, &buffer_snapshot)
                    };
                    let context_end =
                        || probe.context.end.cmp(&excerpt_range.end, &buffer_snapshot);
                    let primary_start = || {
                        probe
                            .primary
                            .start
                            .cmp(&initial_range.start, &buffer_snapshot)
                    };
                    let primary_end =
                        || probe.primary.end.cmp(&initial_range.end, &buffer_snapshot);
                    context_start()
                        .then_with(context_end)
                        .then_with(primary_start)
                        .then_with(primary_end)
                        .then(cmp::Ordering::Greater)
                };

                let index = excerpt_ranges
                    .binary_search_by(bin_search)
                    .unwrap_or_else(|i| i);

                excerpt_ranges.insert(
                    index,
                    ExcerptRange {
                        context: excerpt_range,
                        primary: initial_range,
                    },
                )
            }

            // 最后，用新的片段范围和诊断块更新编辑器的内容。
            buffer_diagnostics_editor.update_in(cx, |buffer_diagnostics_editor, window, cx| {
                // 从编辑器的显示映射中移除当前的所有 `CustomBlockId`，确保若任何诊断已被解决，
                // 相关联的块将不再显示。
                let block_ids = buffer_diagnostics_editor.blocks.clone();

                buffer_diagnostics_editor.editor.update(cx, |editor, cx| {
                    editor.display_map.update(cx, |display_map, cx| {
                        display_map.remove_blocks(block_ids.into_iter().collect(), cx);
                    })
                });

                let excerpt_ranges: Vec<_> = excerpt_ranges
                    .into_iter()
                    .map(|range| ExcerptRange {
                        context: range.context.to_point(&buffer_snapshot),
                        primary: range.primary.to_point(&buffer_snapshot),
                    })
                    .collect();
                buffer_diagnostics_editor
                    .multibuffer
                    .update(cx, |multibuffer, cx| {
                        multibuffer.set_excerpt_ranges_for_path(
                            PathKey::for_buffer(&buffer, cx),
                            buffer.clone(),
                            &buffer_snapshot,
                            excerpt_ranges.clone(),
                            cx,
                        )
                    });
                let multibuffer_snapshot =
                    buffer_diagnostics_editor.multibuffer.read(cx).snapshot(cx);
                let anchor_ranges: Vec<Range<editor::Anchor>> = excerpt_ranges
                    .into_iter()
                    .filter_map(|range| {
                        let text_range = buffer_snapshot.anchor_range_inside(range.primary);
                        let start = multibuffer_snapshot.anchor_in_buffer(text_range.start)?;
                        let end = multibuffer_snapshot.anchor_in_buffer(text_range.end)?;
                        Some(start..end)
                    })
                    .collect();

                if was_empty {
                    if let Some(anchor_range) = anchor_ranges.first() {
                        let range_to_select = anchor_range.start..anchor_range.start;

                        buffer_diagnostics_editor.editor.update(cx, |editor, cx| {
                            editor.change_selections(Default::default(), window, cx, |selection| {
                                selection.select_anchor_ranges([range_to_select])
                            })
                        });

                        // 如果 `BufferDiagnosticsEditor` 当前处于聚焦状态，则将焦点移至其编辑器。
                        if buffer_diagnostics_editor.focus_handle.is_focused(window) {
                            buffer_diagnostics_editor
                                .editor
                                .read(cx)
                                .focus_handle(cx)
                                .focus(window, cx);
                        }
                    }
                }

                // 克隆块数据后转移所有权，以便后续在测试中用于设置块内容。
                #[cfg(test)]
                let cloned_blocks = blocks.clone();

                // 为新的诊断构建将添加到编辑器显示映射中的诊断块。
                // 在结束前更新 `blocks` 属性，以确保下次执行时可以移除这些块。
                let editor_blocks =
                    anchor_ranges
                        .into_iter()
                        .zip(blocks.into_iter())
                        .map(|(anchor, block)| {
                            let editor = buffer_diagnostics_editor.editor.downgrade();

                            BlockProperties {
                                placement: BlockPlacement::Near(anchor.start),
                                height: Some(1),
                                style: BlockStyle::Flex,
                                render: Arc::new(move |block_context| {
                                    block.render_block(editor.clone(), block_context)
                                }),
                                priority: 1,
                            }
                        });

                let block_ids = buffer_diagnostics_editor.editor.update(cx, |editor, cx| {
                    editor.display_map.update(cx, |display_map, cx| {
                        display_map.insert_blocks(editor_blocks, cx)
                    })
                });

                // 为了能够验证编辑器中渲染了哪些诊断块，必须使用
                // `set_block_content_for_tests` 函数，这样
                // `editor::test::editor_content_with_blocks` 函数才能随后被调用以获取这些块。
                #[cfg(test)]
                {
                    for (block_id, block) in block_ids.iter().zip(cloned_blocks.iter()) {
                        let markdown = block.markdown.clone();
                        editor::test::set_block_content_for_tests(
                            &buffer_diagnostics_editor.editor,
                            *block_id,
                            cx,
                            move |cx| {
                                markdown::MarkdownElement::rendered_text(
                                    markdown.clone(),
                                    cx,
                                    editor::hover_popover::diagnostics_markdown_style,
                                )
                            },
                        );
                    }
                }

                buffer_diagnostics_editor.blocks = block_ids;
                cx.notify()
            })
        })
    }

    fn set_diagnostics(&mut self, diagnostics: &[DiagnosticEntryRef<'_, Anchor>]) {
        self.diagnostics = diagnostics
            .iter()
            .map(DiagnosticEntryRef::to_owned)
            .collect();
    }

    fn diagnostics_are_unchanged(
        &self,
        diagnostics: &Vec<DiagnosticEntryRef<'_, Anchor>>,
        snapshot: &BufferSnapshot,
    ) -> bool {
        if self.diagnostics.len() != diagnostics.len() {
            return false;
        }

        self.diagnostics
            .iter()
            .zip(diagnostics.iter())
            .all(|(existing, new)| {
                existing.diagnostic.message == new.diagnostic.message
                    && existing.diagnostic.severity == new.diagnostic.severity
                    && existing.diagnostic.is_primary == new.diagnostic.is_primary
                    && existing.range.to_offset(snapshot) == new.range.to_offset(snapshot)
            })
    }

    fn focus_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 当 `BufferDiagnosticsEditor` 获得焦点且 MultiBuffer 非空时，
        // 将焦点转移给编辑器，以便用户可以开始交互和编辑缓冲区内容。
        if self.focus_handle.is_focused(window) && !self.multibuffer.read(cx).is_empty() {
            self.editor.focus_handle(cx).focus(window, cx)
        }
    }

    fn focus_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus_handle.is_focused(window) && !self.editor.focus_handle(cx).is_focused(window)
        {
            self.update_all_excerpts(window, cx);
        }
    }

    pub fn toggle_warnings(
        &mut self,
        _: &ToggleWarnings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let include_warnings = !self.include_warnings;
        let max_severity = Self::max_diagnostics_severity(include_warnings);

        self.editor.update(cx, |editor, cx| {
            editor.set_max_diagnostics_severity(max_severity, cx);
        });

        self.include_warnings = include_warnings;
        self.diagnostics.clear();
        self.update_all_diagnostics(window, cx);
    }

    fn max_diagnostics_severity(include_warnings: bool) -> DiagnosticSeverity {
        match include_warnings {
            true => DiagnosticSeverity::Warning,
            false => DiagnosticSeverity::Error,
        }
    }

    #[cfg(test)]
    pub fn editor(&self) -> &Entity<Editor> {
        &self.editor
    }

    #[cfg(test)]
    pub fn summary(&self) -> &DiagnosticSummary {
        &self.summary
    }
}

impl Focusable for BufferDiagnosticsEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<EditorEvent> for BufferDiagnosticsEditor {}

impl Item for BufferDiagnosticsEditor {
    type Event = EditorEvent;

    fn act_as_type<'a>(
        &'a self,
        type_id: std::any::TypeId,
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

    fn added_to_workspace(
        &mut self,
        workspace: &mut Workspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |editor, cx| {
            editor.added_to_workspace(workspace, window, cx)
        });
    }

    fn can_save(&self, _cx: &App) -> bool {
        true
    }

    fn can_split(&self) -> bool {
        true
    }

    fn clone_on_split(
        &self,
        _workspace_id: Option<workspace::WorkspaceId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Option<Entity<Self>>>
    where
        Self: Sized,
    {
        Task::ready(Some(cx.new(|cx| {
            BufferDiagnosticsEditor::new(
                self.project_path.clone(),
                self.project.clone(),
                self.buffer.clone(),
                self.include_warnings,
                window,
                cx,
            )
        })))
    }

    fn deactivated(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor
            .update(cx, |editor, cx| editor.deactivated(window, cx));
    }

    fn for_each_project_item(&self, cx: &App, f: &mut dyn FnMut(EntityId, &dyn ProjectItem)) {
        self.editor.for_each_project_item(cx, f);
    }

    fn has_conflict(&self, cx: &App) -> bool {
        self.multibuffer.read(cx).has_conflict(cx)
    }

    fn has_deleted_file(&self, cx: &App) -> bool {
        self.multibuffer.read(cx).has_deleted_file(cx)
    }

    fn is_dirty(&self, cx: &App) -> bool {
        self.multibuffer.read(cx).is_dirty(cx)
    }

    fn capability(&self, cx: &App) -> Capability {
        self.multibuffer.read(cx).capability()
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

    fn reload(
        &mut self,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        self.editor.reload(project, window, cx)
    }

    fn save(
        &mut self,
        options: workspace::item::SaveOptions,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        self.editor.save(options, project, window, cx)
    }

    fn save_as(
        &mut self,
        _project: Entity<Project>,
        _path: ProjectPath,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        unreachable!()
    }

    fn set_nav_history(
        &mut self,
        nav_history: ItemNavHistory,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |editor, _| {
            editor.set_nav_history(Some(nav_history));
        })
    }

    // 构建要在标签页中显示的内容。
    fn tab_content(&self, params: TabContentParams, _window: &Window, cx: &App) -> AnyElement {
        let path_style = self.project.read(cx).path_style(cx);
        let error_count = self.summary.error_count;
        let warning_count = self.summary.warning_count;
        let label = Label::new(
            self.project_path
                .path
                .file_name()
                .map(|s| s.to_string())
                .unwrap_or_else(|| self.project_path.path.display(path_style).to_string()),
        );

        h_flex()
            .gap_1()
            .child(label)
            .when(error_count == 0 && warning_count == 0, |parent| {
                parent.child(
                    h_flex()
                        .gap_1()
                        .child(Icon::new(IconName::Check).color(Color::Success)),
                )
            })
            .when(error_count > 0, |parent| {
                parent.child(
                    h_flex()
                        .gap_1()
                        .child(Icon::new(IconName::XCircle).color(Color::Error))
                        .child(Label::new(error_count.to_string()).color(params.text_color())),
                )
            })
            .when(warning_count > 0, |parent| {
                parent.child(
                    h_flex()
                        .gap_1()
                        .child(Icon::new(IconName::Warning).color(Color::Warning))
                        .child(Label::new(warning_count.to_string()).color(params.text_color())),
                )
            })
            .into_any_element()
    }

    fn tab_content_text(&self, _detail: usize, _app: &App) -> SharedString {
        "Buffer Diagnostics".into()
    }

    fn tab_tooltip_text(&self, cx: &App) -> Option<SharedString> {
        let path_style = self.project.read(cx).path_style(cx);
        Some(
            format!(
                "Buffer Diagnostics - {}",
                self.project_path.path.display(path_style)
            )
            .into(),
        )
    }

    fn telemetry_event_text(&self) -> Option<&'static str> {
        Some("Buffer Diagnostics Opened")
    }

    fn to_item_events(event: &EditorEvent, f: &mut dyn FnMut(ItemEvent)) {
        Editor::to_item_events(event, f)
    }
}

impl Render for BufferDiagnosticsEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let path_style = self.project.read(cx).path_style(cx);
        let filename = self.project_path.path.display(path_style).to_string();
        let error_count = self.summary.error_count;
        let warning_count = match self.include_warnings {
            true => self.summary.warning_count,
            false => 0,
        };

        let child = if error_count + warning_count == 0 {
            let label = match warning_count {
                0 => "没有发现问题",
                _ => "没有发现错误",
            };

            v_flex()
                .key_context("EmptyPane")
                .size_full()
                .gap_1()
                .justify_center()
                .items_center()
                .text_center()
                .bg(cx.theme().colors().editor_background)
                .child(
                    div()
                        .h_flex()
                        .child(Label::new(label).color(Color::Muted))
                        .child(
                            Button::new("open-file", filename)
                                .style(ButtonStyle::Transparent)
                                .tooltip(Tooltip::text("打开文件"))
                                .on_click(cx.listener(|buffer_diagnostics, _, window, cx| {
                                    if let Some(workspace) = Workspace::for_window(window, cx) {
                                        workspace.update(cx, |workspace, cx| {
                                            workspace
                                                .open_path(
                                                    buffer_diagnostics.project_path.clone(),
                                                    None,
                                                    true,
                                                    window,
                                                    cx,
                                                )
                                                .detach_and_log_err(cx);
                                        })
                                    }
                                })),
                        ),
                )
                .when(self.summary.warning_count > 0, |div| {
                    let label = match self.summary.warning_count {
                        1 => "显示 1 条警告".into(),
                        warning_count => format!("显示 {} 条警告", warning_count),
                    };

                    div.child(
                        Button::new("diagnostics-show-warning-label", label).on_click(cx.listener(
                            |buffer_diagnostics_editor, _, window, cx| {
                                buffer_diagnostics_editor.toggle_warnings(
                                    &Default::default(),
                                    window,
                                    cx,
                                );
                                cx.notify();
                            },
                        )),
                    )
                })
        } else {
            div().size_full().child(self.editor.clone())
        };

        div()
            .key_context("Diagnostics")
            .track_focus(&self.focus_handle(cx))
            .size_full()
            .child(child)
    }
}

impl DiagnosticsToolbarEditor for WeakEntity<BufferDiagnosticsEditor> {
    fn include_warnings(&self, cx: &App) -> bool {
        self.read_with(cx, |buffer_diagnostics_editor, _cx| {
            buffer_diagnostics_editor.include_warnings
        })
        .unwrap_or(false)
    }

    fn is_updating(&self, cx: &App) -> bool {
        self.read_with(cx, |buffer_diagnostics_editor, cx| {
            buffer_diagnostics_editor.update_excerpts_task.is_some()
                || buffer_diagnostics_editor
                    .project
                    .read(cx)
                    .language_servers_running_disk_based_diagnostics(cx)
                    .next()
                    .is_some()
        })
        .unwrap_or(false)
    }

    fn stop_updating(&self, cx: &mut App) {
        let _ = self.update(cx, |buffer_diagnostics_editor, cx| {
            buffer_diagnostics_editor.update_excerpts_task = None;
            cx.notify();
        });
    }

    fn refresh_diagnostics(&self, window: &mut Window, cx: &mut App) {
        let _ = self.update(cx, |buffer_diagnostics_editor, cx| {
            buffer_diagnostics_editor.update_all_excerpts(window, cx);
        });
    }

    fn toggle_warnings(&self, window: &mut Window, cx: &mut App) {
        let _ = self.update(cx, |buffer_diagnostics_editor, cx| {
            buffer_diagnostics_editor.toggle_warnings(&Default::default(), window, cx);
        });
    }

    fn get_diagnostics_for_buffer(
        &self,
        _buffer_id: text::BufferId,
        cx: &App,
    ) -> Vec<language::DiagnosticEntry<text::Anchor>> {
        self.read_with(cx, |buffer_diagnostics_editor, _cx| {
            buffer_diagnostics_editor.diagnostics.clone()
        })
        .unwrap_or_default()
    }
}