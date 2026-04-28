use crate::{
    Copy, CopyAndTrim, CopyPermalinkToLine, Cut, DisplayPoint, DisplaySnapshot, Editor,
    EvaluateSelectedText, FindAllReferences, GoToDeclaration, GoToDefinition, GoToImplementation,
    GoToTypeDefinition, Paste, Rename, RevealInFileManager, RunToCursor, SelectMode,
    SelectionEffects, SelectionExt, ToDisplayPoint, ToggleCodeActions,
    actions::{Format, FormatSelections},
    selections_collection::SelectionsCollection,
};
use gpui::prelude::FluentBuilder;
use gpui::{Context, DismissEvent, Entity, Focusable as _, Pixels, Point, Subscription, Window};
use project::DisableAiSettings;
use std::ops::Range;
use text::PointUtf16;
use workspace::OpenInTerminal;
use zed_actions::agent::AddSelectionToThread;
use zed_actions::preview::{
    markdown::OpenPreview as OpenMarkdownPreview, svg::OpenPreview as OpenSvgPreview,
};

/// 菜单位置类型
#[derive(Debug)]
pub enum MenuPosition {
    /// 编辑器滚动时，上下文菜单固定在屏幕上的精确位置，不会消失
    PinnedToScreen(Point<Pixels>),
    /// 编辑器滚动时，上下文菜单跟随关联的位置移动
    /// 当位置不可见时，菜单自动消失
    PinnedToEditor {
        source: multi_buffer::Anchor,
        offset: Point<Pixels>,
    },
}

/// 鼠标右键上下文菜单
pub struct MouseContextMenu {
    pub(crate) position: MenuPosition,
    pub(crate) context_menu: Entity<ui::ContextMenu>,
    _dismiss_subscription: Subscription,
    _cursor_move_subscription: Subscription,
}

impl std::fmt::Debug for MouseContextMenu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MouseContextMenu")
            .field("position", &self.position)
            .field("context_menu", &self.context_menu)
            .finish()
    }
}

impl MouseContextMenu {
    /// 创建固定在编辑器上的上下文菜单
    pub(crate) fn pinned_to_editor(
        editor: &mut Editor,
        source: multi_buffer::Anchor,
        position: Point<Pixels>,
        context_menu: Entity<ui::ContextMenu>,
        window: &mut Window,
        cx: &mut Context<Editor>,
    ) -> Option<Self> {
        let editor_snapshot = editor.snapshot(window, cx);
        let content_origin = editor.last_bounds?.origin
            + Point {
                x: editor.gutter_dimensions.width,
                y: Pixels::ZERO,
            };
        let source_position = editor.to_pixel_point(source, &editor_snapshot, window, cx)?;
        let menu_position = MenuPosition::PinnedToEditor {
            source,
            offset: position - (source_position + content_origin),
        };
        Some(MouseContextMenu::new(
            editor,
            menu_position,
            context_menu,
            window,
            cx,
        ))
    }

    /// 创建新的鼠标上下文菜单
    pub(crate) fn new(
        editor: &Editor,
        position: MenuPosition,
        context_menu: Entity<ui::ContextMenu>,
        window: &mut Window,
        cx: &mut Context<Editor>,
    ) -> Self {
        let context_menu_focus = context_menu.focus_handle(cx);

        // 由于上下文菜单是延迟渲染的，它的焦点句柄需要等到下一帧才能关联到编辑器
        // 因此需要延迟聚焦，确保编辑器能正确检测到菜单获得焦点
        let focus_handle = context_menu_focus.clone();
        cx.on_next_frame(window, move |_, window, cx| {
            cx.on_next_frame(window, move |_, window, cx| {
                window.focus(&focus_handle, cx);
            });
        });

        // 订阅菜单关闭事件：关闭时清空菜单并恢复编辑器焦点
        let _dismiss_subscription = cx.subscribe_in(&context_menu, window, {
            let context_menu_focus = context_menu_focus.clone();
            move |editor, _, _event: &DismissEvent, window, cx| {
                editor.mouse_context_menu.take();
                if context_menu_focus.contains_focused(window, cx) {
                    window.focus(&editor.focus_handle(cx), cx);
                }
            }
        });

        // 记录初始选区，选区变化时自动关闭菜单
        let selection_init = editor.selections.newest_anchor().clone();

        let _cursor_move_subscription = cx.subscribe_in(
            &cx.entity(),
            window,
            move |editor, _, event: &crate::EditorEvent, window, cx| {
                let crate::EditorEvent::SelectionsChanged { local: true } = event else {
                    return;
                };
                let display_snapshot = &editor
                    .display_map
                    .update(cx, |display_map, cx| display_map.snapshot(cx));
                let selection_init_range = selection_init.display_range(display_snapshot);
                let selection_now_range = editor
                    .selections
                    .newest_anchor()
                    .display_range(display_snapshot);
                if selection_now_range == selection_init_range {
                    return;
                }
                // 选区发生变化，关闭上下文菜单
                editor.mouse_context_menu.take();
                if context_menu_focus.contains_focused(window, cx) {
                    window.focus(&editor.focus_handle(cx), cx);
                }
            },
        );

        Self {
            position,
            context_menu,
            _dismiss_subscription,
            _cursor_move_subscription,
        }
    }
}

/// 获取选区对应的显示范围迭代器
fn display_ranges<'a>(
    display_map: &'a DisplaySnapshot,
    selections: &'a SelectionsCollection,
) -> impl Iterator<Item = Range<DisplayPoint>> + 'a {
    let pending = selections.pending_anchor();
    selections
        .disjoint_anchors()
        .iter()
        .chain(pending)
        .map(move |s| s.start.to_display_point(display_map)..s.end.to_display_point(display_map))
}

/// 部署编辑器右键上下文菜单
pub fn deploy_context_menu(
    editor: &mut Editor,
    position: Option<Point<Pixels>>,
    point: DisplayPoint,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    // 确保编辑器获得焦点
    if !editor.is_focused(window) {
        window.focus(&editor.focus_handle(cx), cx);
    }

    let display_map = editor.display_snapshot(cx);
    let source_anchor = display_map.display_point_to_anchor(point, text::Bias::Right);
    
    // 优先使用自定义上下文菜单
    let context_menu = if let Some(custom) = editor.custom_context_menu.take() {
        let menu = custom(editor, point, window, cx);
        editor.custom_context_menu = Some(custom);
        let Some(menu) = menu else {
            return;
        };
        menu
    } else {
        // 非完整模式的编辑器不显示默认上下文菜单
        if !editor.mode().is_full() {
            return;
        }

        // 无关联项目时不显示菜单
        let Some(project) = editor.project.clone() else {
            return;
        };

        let snapshot = editor.snapshot(window, cx);
        let display_map = editor.display_snapshot(cx);
        let buffer = snapshot.buffer_snapshot();
        let anchor = buffer.anchor_before(point.to_point(&display_map));
        
        // 如果点击位置不在选区内，移动光标到点击位置
        if !display_ranges(&display_map, &editor.selections).any(|r| r.contains(&point)) {
            editor.change_selections(SelectionEffects::no_scroll(), window, cx, |s| {
                s.clear_disjoint();
                s.set_pending_anchor_range(anchor..anchor, SelectMode::Character);
            });
        }

        let focus = window.focused(cx);
        let has_reveal_target = editor.target_file(cx).is_some();
        let has_selections = editor
            .selections
            .all::<PointUtf16>(&display_map)
            .into_iter()
            .any(|s| !s.is_empty());
        
        // 判断是否关联 Git 仓库
        let has_git_repo =
            buffer
                .anchor_to_buffer_anchor(anchor)
                .is_some_and(|(buffer_anchor, _)| {
                    project
                        .read(cx)
                        .git_store()
                        .read(cx)
                        .repository_and_path_for_buffer_id(buffer_anchor.buffer_id, cx)
                        .is_some()
                });

        // 检查可用操作
        let evaluate_selection = window.is_action_available(&EvaluateSelectedText, cx);
        let run_to_cursor = window.is_action_available(&RunToCursor, cx);
        let format_selections = window.is_action_available(&FormatSelections, cx);
        let disable_ai = DisableAiSettings::is_ai_disabled_for_buffer(
            editor.buffer.read(cx).as_singleton().as_ref(),
            cx,
        );

        // 判断文件类型
        let is_markdown = editor
            .buffer()
            .read(cx)
            .as_singleton()
            .and_then(|buffer| buffer.read(cx).language())
            .is_some_and(|language| language.name().as_ref() == "Markdown");

        let is_svg = editor
            .buffer()
            .read(cx)
            .as_singleton()
            .and_then(|buffer| buffer.read(cx).file())
            .is_some_and(|file| {
                std::path::Path::new(file.file_name(cx))
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("svg"))
            });

        // 构建默认上下文菜单
        ui::ContextMenu::build(window, cx, |menu, _window, _cx| {
            let builder = menu
                .on_blur_subscription(Subscription::new(|| {}))
                // 运行调试相关操作
                .when(run_to_cursor, |builder| {
                    builder.action("运行到光标处", Box::new(RunToCursor))
                })
                .when(evaluate_selection && has_selections, |builder| {
                    builder.action("执行选中内容", Box::new(EvaluateSelectedText))
                })
                .when(
                    run_to_cursor || (evaluate_selection && has_selections),
                    |builder| builder.separator(),
                )
                // 代码导航
                .action("跳转到定义", Box::new(GoToDefinition))
                .action("跳转到声明", Box::new(GoToDeclaration))
                .action("跳转到类型定义", Box::new(GoToTypeDefinition))
                .action("跳转到实现", Box::new(GoToImplementation))
                .action(
                    "查找所有引用",
                    Box::new(FindAllReferences::default()),
                )
                .separator()
                // 代码编辑
                .action("重命名符号", Box::new(Rename))
                .action("格式化缓冲区", Box::new(Format))
                .when(format_selections, |cx| {
                    cx.action("格式化选中内容", Box::new(FormatSelections))
                })
                .action(
                    "显示代码操作",
                    Box::new(ToggleCodeActions {
                        deployed_from: None,
                        quick_launch: false,
                    }),
                )
                .when(!disable_ai && has_selections, |this| {
                    this.action("添加到智能助手线程", Box::new(AddSelectionToThread))
                })
                .separator()
                // 剪贴板操作
                .action("剪切", Box::new(Cut))
                .action("复制", Box::new(Copy))
                .action("复制并修剪", Box::new(CopyAndTrim))
                .action("粘贴", Box::new(Paste))
                .separator()
                // 文件与工具操作
                .action_disabled_when(
                    !has_reveal_target,
                    ui::utils::reveal_in_file_manager_label(false),
                    Box::new(RevealInFileManager),
                )
                .when(is_markdown, |builder| {
                    builder.action("打开 Markdown 预览", Box::new(OpenMarkdownPreview))
                })
                .when(is_svg, |builder| {
                    builder.action("打开 SVG 预览", Box::new(OpenSvgPreview))
                })
                .action_disabled_when(
                    !has_reveal_target,
                    "在终端中打开",
                    Box::new(OpenInTerminal),
                )
                // Git 相关操作
                .action_disabled_when(
                    !has_git_repo,
                    "复制永久链接",
                    Box::new(CopyPermalinkToLine),
                )
                .action_disabled_when(
                    !has_git_repo,
                    "查看文件历史",
                    Box::new(git::FileHistory),
                );
            match focus {
                Some(focus) => builder.context(focus),
                None => builder,
            }
        })
    };

    // 设置菜单位置并保存到编辑器
    editor.mouse_context_menu = match position {
        Some(position) => MouseContextMenu::pinned_to_editor(
            editor,
            source_anchor,
            position,
            context_menu,
            window,
            cx,
        ),
        None => {
            let character_size = editor.character_dimensions(window, cx);
            let menu_position = MenuPosition::PinnedToEditor {
                source: source_anchor,
                offset: gpui::point(character_size.em_width, character_size.line_height),
            };
            Some(MouseContextMenu::new(
                editor,
                menu_position,
                context_menu,
                window,
                cx,
            ))
        }
    };
    cx.notify();
}

/// 单元测试
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{editor_tests::init_test, test::editor_lsp_test_context::EditorLspTestContext};
    use indoc::indoc;

    #[gpui::test]
    async fn test_mouse_context_menu(cx: &mut gpui::TestAppContext) {
        init_test(cx, |_| {});

        let mut cx = EditorLspTestContext::new_rust(
            lsp::ServerCapabilities {
                hover_provider: Some(lsp::HoverProviderCapability::Simple(true)),
                ..Default::default()
            },
            cx,
        )
        .await;

        // 设置测试代码状态
        cx.set_state(indoc! {"
            fn teˇst() {
                do_work();
            }
        "});
        let point = cx.display_point(indoc! {"
            fn test() {
                do_wˇork();
            }
        "});
        
        // 初始状态下无上下文菜单
        cx.editor(|editor, _window, _app| assert!(editor.mouse_context_menu.is_none()));

        // 部署上下文菜单并验证焦点状态
        cx.update_editor(|editor, window, cx| {
            deploy_context_menu(editor, Some(Default::default()), point, window, cx);
            // 验证菜单弹出后编辑器焦点状态正常，避免按钮闪烁
            assert!(editor.focus_handle.contains_focused(window, cx));
        });

        cx.assert_editor_state(indoc! {"
            fn test() {
                do_wˇork();
            }
        "});
        
        // 验证菜单已创建
        cx.editor(|editor, _window, _app| assert!(editor.mouse_context_menu.is_some()));
    }
}