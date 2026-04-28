use std::{path::Path, sync::Arc};

use gpui::{EventEmitter, FocusHandle, Focusable};
use ui::{
    App, Button, ButtonCommon, ButtonStyle, Clickable, Context, FluentBuilder, InteractiveElement,
    KeyBinding, Label, LabelCommon, LabelSize, ParentElement, Render, SharedString, Styled as _,
    Window, h_flex, v_flex,
};
use zed_actions::workspace::OpenWithSystem;

use crate::Item;

/// 当缓冲区、图片或其他项目无法打开时显示的视图
#[derive(Debug)]
pub struct InvalidItemView {
    /// 尝试打开的文件路径
    pub abs_path: Arc<Path>,
    /// 打开项目时发生的错误信息
    pub error: SharedString,
    /// 是否为本地文件
    is_local: bool,
    /// 焦点句柄
    focus_handle: FocusHandle,
}

impl InvalidItemView {
    /// 创建无效项目视图实例
    pub fn new(
        abs_path: &Path,
        is_local: bool,
        e: &anyhow::Error,
        _: &mut Window,
        cx: &mut App,
    ) -> Self {
        Self {
            is_local,
            abs_path: Arc::from(abs_path),
            error: format!("{}", e.root_cause()).into(),
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Item for InvalidItemView {
    type Event = ();

    /// 生成标签页显示文本
    fn tab_content_text(&self, mut detail: usize, _: &App) -> SharedString {
        // 确保至少显示文件名
        detail += 1;

        let path = self.abs_path.as_ref();

        let mut prefix = path;
        while detail > 0 {
            if let Some(parent) = prefix.parent() {
                prefix = parent;
                detail -= 1;
            } else {
                break;
            }
        }

        let path = if detail > 0 {
            path
        } else {
            path.strip_prefix(prefix).unwrap_or(path)
        };

        SharedString::new(path.to_string_lossy())
    }
}

impl EventEmitter<()> for InvalidItemView {}

impl Focusable for InvalidItemView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for InvalidItemView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let abs_path = self.abs_path.clone();
        v_flex()
            .size_full()
            .track_focus(&self.focus_handle(cx))
            .flex_none()
            .justify_center()
            .overflow_hidden()
            .key_context("InvalidItem")
            .child(
                h_flex().size_full().justify_center().child(
                    v_flex()
                        .justify_center()
                        .gap_2()
                        .child(h_flex().justify_center().child("无法打开文件"))
                        .child(
                            h_flex()
                                .justify_center()
                                .child(Label::new(self.error.clone()).size(LabelSize::Small)),
                        )
                        .when(self.is_local, |contents| {
                            contents.child(
                                h_flex().justify_center().child(
                                    Button::new("open-with-system", "使用默认应用打开")
                                        .on_click(move |_, _, cx| {
                                            cx.open_with_system(&abs_path);
                                        })
                                        .style(ButtonStyle::Outlined)
                                        .key_binding(KeyBinding::for_action(&OpenWithSystem, cx)),
                                ),
                            )
                        }),
                ),
            )
    }
}