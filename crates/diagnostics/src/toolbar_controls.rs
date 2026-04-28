use crate::{BufferDiagnosticsEditor, ProjectDiagnosticsEditor, ToggleDiagnosticsRefresh};
use agent_settings::AgentSettings;
use gpui::{Context, EventEmitter, ParentElement, Render, Window};
use language::DiagnosticEntry;
use settings::Settings;
use text::{Anchor, BufferId};
use ui::{Tooltip, prelude::*};
use workspace::{ToolbarItemEvent, ToolbarItemLocation, ToolbarItemView, item::ItemHandle};
use zed_actions::assistant::InlineAssist;
use zed_actions::buffer_search;

pub struct ToolbarControls {
    editor: Option<Box<dyn DiagnosticsToolbarEditor>>,
}

/// 诊断工具栏编辑器接口
pub(crate) trait DiagnosticsToolbarEditor: Send + Sync {
    /// 查询工具栏是否在诊断信息中包含警告
    fn include_warnings(&self, cx: &App) -> bool;
    /// 切换是否显示警告类诊断信息
    fn toggle_warnings(&self, window: &mut Window, cx: &mut App);
    /// 查询诊断编辑器是否正在更新诊断信息
    fn is_updating(&self, cx: &App) -> bool;
    /// 请求编辑器停止更新诊断信息
    fn stop_updating(&self, cx: &mut App);
    /// 请求编辑器使用最新数据刷新诊断信息
    fn refresh_diagnostics(&self, window: &mut Window, cx: &mut App);
    /// 获取指定缓冲区的诊断信息列表
    fn get_diagnostics_for_buffer(
        &self,
        buffer_id: BufferId,
        cx: &App,
    ) -> Vec<DiagnosticEntry<Anchor>>;
}

impl Render for ToolbarControls {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut include_warnings = false;
        let mut is_updating = false;

        match &self.editor {
            Some(editor) => {
                include_warnings = editor.include_warnings(cx);
                is_updating = editor.is_updating(cx);
            }
            None => {}
        }

        let is_agent_enabled = AgentSettings::get_global(cx).enabled(cx);

        let (warning_tooltip, warning_color) = if include_warnings {
            ("隐藏警告", Color::Warning)
        } else {
            ("显示警告", Color::Disabled)
        };

        h_flex()
            .gap_1()
            // 缓冲区搜索
            .child({
                IconButton::new("toggle_search", IconName::MagnifyingGlass)
                    .icon_size(IconSize::Small)
                    .tooltip(Tooltip::for_action_title(
                        "缓冲区搜索",
                        &buffer_search::Deploy::find(),
                    ))
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(buffer_search::Deploy::find()), cx);
                    })
            })
            // AI 助手（仅启用时显示）
            .when(is_agent_enabled, |this| {
                this.child(
                    IconButton::new("inline_assist", IconName::ZedAssistant)
                        .icon_size(IconSize::Small)
                        .tooltip(Tooltip::for_action_title(
                            "行内助手",
                            &InlineAssist::default(),
                        ))
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(InlineAssist::default()), cx);
                        }),
                )
            })
            // 停止 / 刷新诊断
            .map(|div| {
                if is_updating {
                    div.child(
                        IconButton::new("stop-updating", IconName::Stop)
                            .icon_color(Color::Error)
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::for_action_title(
                                "停止诊断更新",
                                &ToggleDiagnosticsRefresh,
                            ))
                            .on_click(cx.listener(move |toolbar_controls, _, _, cx| {
                                if let Some(editor) = toolbar_controls.editor() {
                                    editor.stop_updating(cx);
                                    cx.notify();
                                }
                            })),
                    )
                } else {
                    div.child(
                        IconButton::new("refresh-diagnostics", IconName::ArrowCircle)
                            .icon_size(IconSize::Small)
                            .tooltip(Tooltip::for_action_title(
                                "刷新诊断",
                                &ToggleDiagnosticsRefresh,
                            ))
                            .on_click(cx.listener({
                                move |toolbar_controls, _, window, cx| {
                                    if let Some(editor) = toolbar_controls.editor() {
                                        editor.refresh_diagnostics(window, cx)
                                    }
                                }
                            })),
                    )
                }
            })
            // 显示/隐藏警告
            .child(
                IconButton::new("toggle-warnings", IconName::Warning)
                    .icon_color(warning_color)
                    .icon_size(IconSize::Small)
                    .tooltip(Tooltip::text(warning_tooltip))
                    .on_click(cx.listener(|this, _, window, cx| {
                        if let Some(editor) = &this.editor {
                            editor.toggle_warnings(window, cx)
                        }
                    })),
            )
    }
}

impl EventEmitter<ToolbarItemEvent> for ToolbarControls {}

impl ToolbarItemView for ToolbarControls {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        _: &mut Context<Self>,
    ) -> ToolbarItemLocation {
        if let Some(pane_item) = active_pane_item.as_ref() {
            if let Some(editor) = pane_item.downcast::<ProjectDiagnosticsEditor>() {
                self.editor = Some(Box::new(editor.downgrade()));
                ToolbarItemLocation::PrimaryRight
            } else if let Some(editor) = pane_item.downcast::<BufferDiagnosticsEditor>() {
                self.editor = Some(Box::new(editor.downgrade()));
                ToolbarItemLocation::PrimaryRight
            } else {
                ToolbarItemLocation::Hidden
            }
        } else {
            ToolbarItemLocation::Hidden
        }
    }
}

impl Default for ToolbarControls {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolbarControls {
    pub fn new() -> Self {
        ToolbarControls { editor: None }
    }

    fn editor(&self) -> Option<&dyn DiagnosticsToolbarEditor> {
        self.editor.as_deref()
    }
}