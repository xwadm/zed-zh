mod preview;
mod repl_menu;

use agent_settings::AgentSettings;
use editor::actions::{
    AddSelectionAbove, AddSelectionBelow, CodeActionSource, DuplicateLineDown, GoToDiagnostic,
    GoToHunk, GoToPreviousDiagnostic, GoToPreviousHunk, MoveLineDown, MoveLineUp, SelectAll,
    SelectLargerSyntaxNode, SelectNext, SelectSmallerSyntaxNode, ToggleCodeActions,
    ToggleDiagnostics, ToggleGoToLine, ToggleInlineDiagnostics,
};
use editor::code_context_menus::{CodeContextMenu, ContextMenuOrigin};
use editor::{Editor, EditorSettings};
use gpui::{
    Action, Anchor, AnchoredPositionMode, ClickEvent, Context, ElementId, Entity, EventEmitter,
    FocusHandle, Focusable, InteractiveElement, ParentElement, Render, Styled, Subscription,
    WeakEntity, Window, anchored, deferred, point,
};
use project::{DisableAiSettings, project_settings::DiagnosticSeverity};
use search::{BufferSearchBar, buffer_search};
use settings::{Settings, SettingsStore};
use ui::{
    ButtonStyle, ContextMenu, ContextMenuEntry, DocumentationSide, IconButton, IconName, IconSize,
    PopoverMenu, PopoverMenuHandle, Tooltip, prelude::*,
};
use vim_mode_setting::{HelixModeSetting, VimModeSetting};
use workspace::item::ItemBufferKind;
use workspace::{
    ToolbarItemEvent, ToolbarItemLocation, ToolbarItemView, Workspace, item::ItemHandle,
};
use zed_actions::{agent::AddSelectionToThread, assistant::InlineAssist, outline::ToggleOutline};

/// 代码操作菜单最大显示行数
const MAX_CODE_ACTION_MENU_LINES: u32 = 16;

/// 快速操作栏组件
pub struct QuickActionBar {
    _inlay_hints_enabled_subscription: Option<Subscription>,
    _ai_settings_subscription: Subscription,
    active_item: Option<Box<dyn ItemHandle>>,
    buffer_search_bar: Entity<BufferSearchBar>,
    show: bool,
    toggle_selections_handle: PopoverMenuHandle<ContextMenu>,
    toggle_settings_handle: PopoverMenuHandle<ContextMenu>,
    workspace: WeakEntity<Workspace>,
}

impl QuickActionBar {
    /// 创建新的快速操作栏实例
    pub fn new(
        buffer_search_bar: Entity<BufferSearchBar>,
        workspace: &Workspace,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut was_agent_enabled = AgentSettings::get_global(cx).enabled(cx);
        let mut was_agent_button = AgentSettings::get_global(cx).button;

        // 监听AI设置变更
        let ai_settings_subscription = cx.observe_global::<SettingsStore>(move |_, cx| {
            let agent_settings = AgentSettings::get_global(cx);
            let is_agent_enabled = agent_settings.enabled(cx);

            if was_agent_enabled != is_agent_enabled || was_agent_button != agent_settings.button {
                was_agent_enabled = is_agent_enabled;
                was_agent_button = agent_settings.button;
                cx.notify();
            }
        });

        let mut this = Self {
            _inlay_hints_enabled_subscription: None,
            _ai_settings_subscription: ai_settings_subscription,
            active_item: None,
            buffer_search_bar,
            show: true,
            toggle_selections_handle: Default::default(),
            toggle_settings_handle: Default::default(),
            workspace: workspace.weak_handle(),
        };
        this.apply_settings(cx);
        cx.observe_global::<SettingsStore>(|this, cx| this.apply_settings(cx))
            .detach();
        this
    }

    /// 获取当前激活的编辑器
    fn active_editor(&self) -> Option<Entity<Editor>> {
        self.active_item
            .as_ref()
            .and_then(|item| item.downcast::<Editor>())
    }

    /// 应用编辑器设置
    fn apply_settings(&mut self, cx: &mut Context<Self>) {
        let new_show = EditorSettings::get_global(cx).toolbar.quick_actions;
        if new_show != self.show {
            self.show = new_show;
            cx.emit(ToolbarItemEvent::ChangeLocation(
                self.get_toolbar_item_location(),
            ));
        }
    }

    /// 获取工具栏项显示位置
    fn get_toolbar_item_location(&self) -> ToolbarItemLocation {
        if self.show && self.active_editor().is_some() {
            ToolbarItemLocation::PrimaryRight
        } else {
            ToolbarItemLocation::Hidden
        }
    }
}

impl Render for QuickActionBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(editor) = self.active_editor() else {
            return div().id("empty quick action bar");
        };

        let supports_inlay_hints = editor.update(cx, |editor, cx| editor.supports_inlay_hints(cx));
        let supports_semantic_tokens =
            editor.update(cx, |editor, cx| editor.supports_semantic_tokens(cx));
        let supports_code_lens = editor.update(cx, |editor, cx| editor.supports_code_lens(cx));
        let editor_value = editor.read(cx);
        let selection_menu_enabled = editor_value.selection_menu_enabled(cx);
        let inlay_hints_enabled = editor_value.inlay_hints_enabled();
        let inline_values_enabled = editor_value.inline_values_enabled();
        let semantic_highlights_enabled = editor_value.semantic_highlights_enabled();
        let code_lens_enabled = editor_value.code_lens_enabled();
        let is_full = editor_value.mode().is_full();
        let diagnostics_enabled = editor_value.diagnostics_max_severity != DiagnosticSeverity::Off;
        let supports_inline_diagnostics = editor_value.inline_diagnostics_enabled();
        let inline_diagnostics_enabled = editor_value.show_inline_diagnostics();
        let git_blame_inline_enabled = editor_value.git_blame_inline_enabled();
        let show_git_blame_gutter = editor_value.show_git_blame_gutter();
        let auto_signature_help_enabled = editor_value.auto_signature_help_enabled(cx);
        let show_line_numbers = editor_value.line_numbers_enabled(cx);
        let has_edit_prediction_provider = editor_value.edit_prediction_provider().is_some();
        let show_edit_predictions = editor_value.edit_predictions_enabled();
        let edit_predictions_enabled_at_cursor =
            editor_value.edit_predictions_enabled_at_cursor(cx);
        let supports_minimap = editor_value.supports_minimap(cx);
        let minimap_enabled = supports_minimap && editor_value.minimap().is_some();
        let has_available_code_actions = editor_value.has_available_code_actions_for_selection();
        let code_action_enabled = editor_value.code_actions_enabled_for_toolbar(cx);
        let focus_handle = editor_value.focus_handle(cx);

        // 缓冲区搜索按钮
        let search_button = (editor.buffer_kind(cx) == ItemBufferKind::Singleton).then(|| {
            QuickActionBarButton::new(
                "toggle buffer search",
                search::SEARCH_ICON,
                !self.buffer_search_bar.read(cx).is_dismissed(),
                Box::new(buffer_search::Deploy::find()),
                focus_handle.clone(),
                "缓冲区搜索",
                {
                    let buffer_search_bar = self.buffer_search_bar.clone();
                    move |_, window, cx| {
                        buffer_search_bar.update(cx, |search_bar, cx| {
                            search_bar.toggle(&buffer_search::Deploy::find(), window, cx)
                        });
                    }
                },
            )
        });

        // 助手按钮
        let assistant_button = QuickActionBarButton::new(
            "toggle inline assistant",
            IconName::ZedAssistant,
            false,
            Box::new(InlineAssist::default()),
            focus_handle,
            "行内助手",
            move |_, window, cx| {
                window.dispatch_action(Box::new(InlineAssist::default()), cx);
            },
        );

        // 代码操作下拉菜单
        let code_actions_dropdown = code_action_enabled.then(|| {
            let is_deployed = {
                let menu_ref = editor.read(cx).context_menu().borrow();
                let code_action_menu = menu_ref
                    .as_ref()
                    .filter(|menu| matches!(menu, CodeContextMenu::CodeActions(..)));
                code_action_menu
                    .as_ref()
                    .is_some_and(|menu| matches!(menu.origin(), ContextMenuOrigin::QuickActionBar))
            };
            let code_action_element = is_deployed
                .then(|| {
                    editor.update(cx, |editor, cx| {
                        editor.render_context_menu(MAX_CODE_ACTION_MENU_LINES, window, cx)
                    })
                })
                .flatten();
            v_flex()
                .child(
                    IconButton::new("toggle_code_actions_icon", IconName::BoltOutlined)
                        .icon_size(IconSize::Small)
                        .style(ButtonStyle::Subtle)
                        .disabled(!has_available_code_actions)
                        .toggle_state(is_deployed)
                        .when(!is_deployed, |this| {
                            this.when(has_available_code_actions, |this| {
                                this.tooltip(Tooltip::for_action_title(
                                    "代码操作",
                                    &ToggleCodeActions::default(),
                                ))
                            })
                            .when(
                                !has_available_code_actions,
                                |this| {
                                    this.tooltip(Tooltip::for_action_title(
                                        "无可用代码操作",
                                        &ToggleCodeActions::default(),
                                    ))
                                },
                            )
                        })
                        .on_click({
                            let editor = editor.clone();
                            move |_, window, cx| {
                                editor.update(cx, |editor, cx| {
                                    editor.toggle_code_actions(
                                        &ToggleCodeActions {
                                            deployed_from: Some(CodeActionSource::QuickActionBar),
                                            quick_launch: false,
                                        },
                                        window,
                                        cx,
                                    );
                                })
                            }
                        }),
                )
                .children(code_action_element.map(|menu| {
                    deferred(
                        anchored()
                            .position_mode(AnchoredPositionMode::Local)
                            .position(point(px(20.), px(20.)))
                            .anchor(Anchor::TopRight)
                            .child(menu),
                    )
                }))
        });

        // 编辑器选择下拉菜单
        let editor_selections_dropdown = selection_menu_enabled.then(|| {
            let has_diff_hunks = editor
                .read(cx)
                .buffer()
                .read(cx)
                .snapshot(cx)
                .has_diff_hunks();
            let has_selection = editor.update(cx, |editor, cx| {
                editor.has_non_empty_selection(&editor.display_snapshot(cx))
            });

            let focus = editor.focus_handle(cx);

            let disable_ai = DisableAiSettings::get_global(cx).disable_ai;

            PopoverMenu::new("editor-selections-dropdown")
                .trigger_with_tooltip(
                    IconButton::new("toggle_editor_selections_icon", IconName::CursorIBeam)
                        .icon_size(IconSize::Small)
                        .style(ButtonStyle::Subtle)
                        .toggle_state(self.toggle_selections_handle.is_deployed()),
                    Tooltip::text("选择控制"),
                )
                .with_handle(self.toggle_selections_handle.clone())
                .anchor(Anchor::TopRight)
                .menu(move |window, cx| {
                    let focus = focus.clone();
                    let menu = ContextMenu::build(window, cx, move |menu, _, _| {
                        menu.context(focus.clone())
                            .action("全选", Box::new(SelectAll))
                            .action(
                                "选择下一个匹配项",
                                Box::new(SelectNext {
                                    replace_newest: false,
                                }),
                            )
                            .action("扩大选择范围", Box::new(SelectLargerSyntaxNode))
                            .action("缩小选择范围", Box::new(SelectSmallerSyntaxNode))
                            .action(
                                "在上方添加光标",
                                Box::new(AddSelectionAbove {
                                    skip_soft_wrap: true,
                                }),
                            )
                            .action(
                                "在下方添加光标",
                                Box::new(AddSelectionBelow {
                                    skip_soft_wrap: true,
                                }),
                            )
                            .when(!disable_ai, |this| {
                                this.separator().action_disabled_when(
                                    !has_selection,
                                    "添加到助手线程",
                                    Box::new(AddSelectionToThread),
                                )
                            })
                            .separator()
                            .action("跳转到符号", Box::new(ToggleOutline))
                            .action("跳转到行/列", Box::new(ToggleGoToLine))
                            .separator()
                            .action("下一个问题", Box::new(GoToDiagnostic::default()))
                            .action(
                                "上一个问题",
                                Box::new(GoToPreviousDiagnostic::default()),
                            )
                            .separator()
                            .action_disabled_when(!has_diff_hunks, "下一个代码块", Box::new(GoToHunk))
                            .action_disabled_when(
                                !has_diff_hunks,
                                "上一个代码块",
                                Box::new(GoToPreviousHunk),
                            )
                            .separator()
                            .action("向上移动行", Box::new(MoveLineUp))
                            .action("向下移动行", Box::new(MoveLineDown))
                            .action("复制选中内容", Box::new(DuplicateLineDown))
                    });
                    Some(menu)
                })
        });

        let editor_focus_handle = editor.focus_handle(cx);
        let editor = editor.downgrade();
        // 编辑器设置下拉菜单
        let editor_settings_dropdown = {
            let vim_mode_enabled = VimModeSetting::get_global(cx).0;
            let helix_mode_enabled = HelixModeSetting::get_global(cx).0;

            PopoverMenu::new("editor-settings")
                .trigger_with_tooltip(
                    IconButton::new("toggle_editor_settings_icon", IconName::Sliders)
                        .icon_size(IconSize::Small)
                        .style(ButtonStyle::Subtle)
                        .toggle_state(self.toggle_settings_handle.is_deployed()),
                    Tooltip::text("编辑器控制"),
                )
                .anchor(Anchor::TopRight)
                .with_handle(self.toggle_settings_handle.clone())
                .menu(move |window, cx| {
                    let menu = ContextMenu::build(window, cx, {
                        let focus_handle = editor_focus_handle.clone();
                        |mut menu, _, _| {
                            menu = menu.context(focus_handle);

                            if supports_inlay_hints {
                                menu = menu.toggleable_entry(
                                    "内联提示",
                                    inlay_hints_enabled,
                                    IconPosition::Start,
                                    Some(editor::actions::ToggleInlayHints.boxed_clone()),
                                    {
                                        let editor = editor.clone();
                                        move |window, cx| {
                                            editor
                                                .update(cx, |editor, cx| {
                                                    editor.toggle_inlay_hints(
                                                        &editor::actions::ToggleInlayHints,
                                                        window,
                                                        cx,
                                                    );
                                                })
                                                .ok();
                                        }
                                    },
                                );

                                menu = menu.toggleable_entry(
                                    "行内值",
                                    inline_values_enabled,
                                    IconPosition::Start,
                                    Some(editor::actions::ToggleInlineValues.boxed_clone()),
                                    {
                                        let editor = editor.clone();
                                        move |window, cx| {
                                            editor
                                                .update(cx, |editor, cx| {
                                                    editor.toggle_inline_values(
                                                        &editor::actions::ToggleInlineValues,
                                                        window,
                                                        cx,
                                                    );
                                                })
                                                .ok();
                                        }
                                    }
                                );
                            }

                            if supports_semantic_tokens {
                                menu = menu.toggleable_entry(
                                    "语义高亮",
                                    semantic_highlights_enabled,
                                    IconPosition::Start,
                                    Some(editor::actions::ToggleSemanticHighlights.boxed_clone()),
                                    {
                                        let editor = editor.clone();
                                        move |window, cx| {
                                            editor
                                                .update(cx, |editor, cx| {
                                                    editor.toggle_semantic_highlights(
                                                        &editor::actions::ToggleSemanticHighlights,
                                                        window,
                                                        cx,
                                                    );
                                                })
                                                .ok();
                                        }
                                    },
                                );
                            }

                            if supports_code_lens {
                                menu = menu.toggleable_entry(
                                    "代码透镜",
                                    code_lens_enabled,
                                    IconPosition::Start,
                                    Some(editor::actions::ToggleCodeLens.boxed_clone()),
                                    {
                                        let editor = editor.clone();
                                        move |window, cx| {
                                            editor
                                                .update(cx, |editor, cx| {
                                                    editor.toggle_code_lens_action(
                                                        &editor::actions::ToggleCodeLens,
                                                        window,
                                                        cx,
                                                    );
                                                })
                                                .ok();
                                        }
                                    },
                                );
                            }

                            if supports_minimap {
                                menu = menu.toggleable_entry("小地图", minimap_enabled, IconPosition::Start, Some(editor::actions::ToggleMinimap.boxed_clone()), {
                                    let editor = editor.clone();
                                    move |window, cx| {
                                        editor
                                            .update(cx, |editor, cx| {
                                                editor.toggle_minimap(
                                                    &editor::actions::ToggleMinimap,
                                                    window,
                                                    cx,
                                                );
                                            })
                                            .ok();
                                    }
                                },)
                            }

                            if has_edit_prediction_provider {
                                let mut edit_prediction_entry = ContextMenuEntry::new("编辑预测")
                                    .toggleable(IconPosition::Start, edit_predictions_enabled_at_cursor && show_edit_predictions)
                                    .disabled(!edit_predictions_enabled_at_cursor)
                                    .action(
                                        editor::actions::ToggleEditPrediction.boxed_clone(),
                                    ).handler({
                                        let editor = editor.clone();
                                        move |window, cx| {
                                            editor
                                                .update(cx, |editor, cx| {
                                                    editor.toggle_edit_predictions(
                                                        &editor::actions::ToggleEditPrediction,
                                                        window,
                                                        cx,
                                                    );
                                                })
                                                .ok();
                                        }
                                    });
                                if !edit_predictions_enabled_at_cursor {
                                    edit_prediction_entry = edit_prediction_entry.documentation_aside(DocumentationSide::Left, |_| {
                                        Label::new("该文件在排除列表中，无法切换编辑预测功能。").into_any_element()
                                    });
                                }

                                menu = menu.item(edit_prediction_entry);
                            }

                            menu = menu.separator();

                            if is_full {
                                menu = menu.toggleable_entry(
                                    "诊断信息",
                                    diagnostics_enabled,
                                    IconPosition::Start,
                                    Some(ToggleDiagnostics.boxed_clone()),
                                    {
                                        let editor = editor.clone();
                                        move |window, cx| {
                                            editor
                                                .update(cx, |editor, cx| {
                                                    editor.toggle_diagnostics(
                                                        &ToggleDiagnostics,
                                                        window,
                                                        cx,
                                                    );
                                                })
                                                .ok();
                                        }
                                    },
                                );

                                if supports_inline_diagnostics {
                                    let mut inline_diagnostics_item = ContextMenuEntry::new("行内诊断")
                                        .toggleable(IconPosition::Start, diagnostics_enabled && inline_diagnostics_enabled)
                                        .action(ToggleInlineDiagnostics.boxed_clone())
                                        .handler({
                                            let editor = editor.clone();
                                            move |window, cx| {
                                                editor
                                                    .update(cx, |editor, cx| {
                                                        editor.toggle_inline_diagnostics(
                                                            &ToggleInlineDiagnostics,
                                                            window,
                                                            cx,
                                                        );
                                                    })
                                                    .ok();
                                            }
                                        });
                                    if !diagnostics_enabled {
                                        inline_diagnostics_item = inline_diagnostics_item.disabled(true).documentation_aside(DocumentationSide::Left, |_|  Label::new("启用常规诊断后才能使用行内诊断。").into_any_element());
                                    }
                                    menu = menu.item(inline_diagnostics_item)
                                }

                                menu = menu.separator();
                            }

                            menu = menu.toggleable_entry(
                                "行号",
                                show_line_numbers,
                                IconPosition::Start,
                                Some(editor::actions::ToggleLineNumbers.boxed_clone()),
                                {
                                    let editor = editor.clone();
                                    move |window, cx| {
                                        editor
                                            .update(cx, |editor, cx| {
                                                editor.toggle_line_numbers(
                                                    &editor::actions::ToggleLineNumbers,
                                                    window,
                                                    cx,
                                                );
                                            })
                                            .ok();
                                    }
                                },
                            );

                            menu = menu.toggleable_entry(
                                "选择菜单",
                                selection_menu_enabled,
                                IconPosition::Start,
                                Some(editor::actions::ToggleSelectionMenu.boxed_clone()),
                                {
                                    let editor = editor.clone();
                                    move |window, cx| {
                                        editor
                                            .update(cx, |editor, cx| {
                                                editor.toggle_selection_menu(
                                                    &editor::actions::ToggleSelectionMenu,
                                                    window,
                                                    cx,
                                                )
                                            })
                                            .ok();
                                    }
                                },
                            );

                            menu = menu.toggleable_entry(
                                "自动签名帮助",
                                auto_signature_help_enabled,
                                IconPosition::Start,
                                Some(editor::actions::ToggleAutoSignatureHelp.boxed_clone()),
                                {
                                    let editor = editor.clone();
                                    move |window, cx| {
                                        editor
                                            .update(cx, |editor, cx| {
                                                editor.toggle_auto_signature_help_menu(
                                                    &editor::actions::ToggleAutoSignatureHelp,
                                                    window,
                                                    cx,
                                                );
                                            })
                                            .ok();
                                    }
                                },
                            );

                            menu = menu.separator();

                            menu = menu.toggleable_entry(
                                "行内Git注释",
                                git_blame_inline_enabled,
                                IconPosition::Start,
                                Some(editor::actions::ToggleGitBlameInline.boxed_clone()),
                                {
                                    let editor = editor.clone();
                                    move |window, cx| {
                                        editor
                                            .update(cx, |editor, cx| {
                                                editor.toggle_git_blame_inline(
                                                    &editor::actions::ToggleGitBlameInline,
                                                    window,
                                                    cx,
                                                )
                                            })
                                            .ok();
                                    }
                                },
                            );

                            menu = menu.toggleable_entry(
                                "侧边Git注释",
                                show_git_blame_gutter,
                                IconPosition::Start,
                                Some(git::Blame.boxed_clone()),
                                {
                                    let editor = editor.clone();
                                    move |window, cx| {
                                        editor
                                            .update(cx, |editor, cx| {
                                                editor.toggle_git_blame(
                                                    &git::Blame,
                                                    window,
                                                    cx,
                                                )
                                            })
                                            .ok();
                                    }
                                },
                            );

                            menu = menu.separator();

                            menu = menu.toggleable_entry(
                                "Vim模式",
                                vim_mode_enabled,
                                IconPosition::Start,
                                None,
                                {
                                    move |window, cx| {
                                        let new_value = !vim_mode_enabled;
                                        VimModeSetting::override_global(VimModeSetting(new_value), cx);
                                        HelixModeSetting::override_global(HelixModeSetting(false), cx);
                                        window.refresh();
                                    }
                                },
                            );
                            menu = menu.toggleable_entry(
                                "Helix模式",
                                helix_mode_enabled,
                                IconPosition::Start,
                                None,
                                {
                                    move |window, cx| {
                                        let new_value = !helix_mode_enabled;
                                        HelixModeSetting::override_global(HelixModeSetting(new_value), cx);
                                        VimModeSetting::override_global(VimModeSetting(false), cx);
                                        window.refresh();
                                    }
                                }
                            );

                            menu
                        }
                    });
                    Some(menu)
                })
        };

        h_flex()
            .id("quick action bar")
            .gap(DynamicSpacing::Base01.rems(cx))
            .children(self.render_repl_menu(cx))
            .children(self.render_preview_button(self.workspace.clone(), cx))
            .children(search_button)
            .when(
                AgentSettings::get_global(cx).enabled(cx) && AgentSettings::get_global(cx).button,
                |bar| bar.child(assistant_button),
            )
            .children(code_actions_dropdown)
            .children(editor_selections_dropdown)
            .child(editor_settings_dropdown)
    }
}

impl EventEmitter<ToolbarItemEvent> for QuickActionBar {}

/// 快速操作栏按钮组件
#[derive(IntoElement)]
struct QuickActionBarButton {
    id: ElementId,
    icon: IconName,
    toggled: bool,
    action: Box<dyn Action>,
    focus_handle: FocusHandle,
    tooltip: SharedString,
    on_click: Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>,
}

impl QuickActionBarButton {
    /// 创建新的快速操作栏按钮
    fn new(
        id: impl Into<ElementId>,
        icon: IconName,
        toggled: bool,
        action: Box<dyn Action>,
        focus_handle: FocusHandle,
        tooltip: impl Into<SharedString>,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            icon,
            toggled,
            action,
            focus_handle,
            tooltip: tooltip.into(),
            on_click: Box::new(on_click),
        }
    }
}

impl RenderOnce for QuickActionBarButton {
    fn render(self, _window: &mut Window, _: &mut App) -> impl IntoElement {
        let tooltip = self.tooltip.clone();
        let action = self.action.boxed_clone();

        IconButton::new(self.id.clone(), self.icon)
            .icon_size(IconSize::Small)
            .style(ButtonStyle::Subtle)
            .toggle_state(self.toggled)
            .tooltip(move |_window, cx| {
                Tooltip::for_action_in(tooltip.clone(), &*action, &self.focus_handle, cx)
            })
            .on_click(move |event, window, cx| (self.on_click)(event, window, cx))
    }
}

impl ToolbarItemView for QuickActionBar {
    /// 设置激活的窗格项
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> ToolbarItemLocation {
        self.active_item = active_pane_item.map(ItemHandle::boxed_clone);
        if let Some(active_item) = active_pane_item {
            self._inlay_hints_enabled_subscription.take();

            if let Some(editor) = active_item.downcast::<Editor>() {
                let (
                    mut inlay_hints_enabled,
                    mut supports_inlay_hints,
                    mut supports_semantic_tokens,
                ) = editor.update(cx, |editor, cx| {
                    (
                        editor.inlay_hints_enabled(),
                        editor.supports_inlay_hints(cx),
                        editor.supports_semantic_tokens(cx),
                    )
                });
                self._inlay_hints_enabled_subscription =
                    Some(cx.observe(&editor, move |_, editor, cx| {
                        let (
                            new_inlay_hints_enabled,
                            new_supports_inlay_hints,
                            new_supports_semantic_tokens,
                        ) = editor.update(cx, |editor, cx| {
                            (
                                editor.inlay_hints_enabled(),
                                editor.supports_inlay_hints(cx),
                                editor.supports_semantic_tokens(cx),
                            )
                        });
                        let should_notify = inlay_hints_enabled != new_inlay_hints_enabled
                            || supports_inlay_hints != new_supports_inlay_hints
                            || supports_semantic_tokens != new_supports_semantic_tokens;
                        inlay_hints_enabled = new_inlay_hints_enabled;
                        supports_inlay_hints = new_supports_inlay_hints;
                        supports_semantic_tokens = new_supports_semantic_tokens;
                        if should_notify {
                            cx.notify()
                        }
                    }));
            }
        }
        self.get_toolbar_item_location()
    }
}