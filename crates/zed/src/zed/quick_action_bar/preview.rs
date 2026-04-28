use csv_preview::{
    CsvPreviewView, OpenPreview as CsvOpenPreview, OpenPreviewToTheSide as CsvOpenPreviewToTheSide,
    TabularDataPreviewFeatureFlag,
};
use feature_flags::FeatureFlagAppExt as _;
use gpui::{AnyElement, Modifiers, WeakEntity};
use markdown_preview::{
    OpenPreview as MarkdownOpenPreview, OpenPreviewToTheSide as MarkdownOpenPreviewToTheSide,
    markdown_preview_view::MarkdownPreviewView,
};
use svg_preview::{
    OpenPreview as SvgOpenPreview, OpenPreviewToTheSide as SvgOpenPreviewToTheSide,
    svg_preview_view::SvgPreviewView,
};
use ui::{Tooltip, prelude::*, text_for_keystroke};
use workspace::Workspace;

use super::QuickActionBar;

/// 预览类型枚举
#[derive(Clone, Copy)]
enum PreviewType {
    Markdown,
    Svg,
    Csv,
}

impl QuickActionBar {
    /// 渲染预览按钮
    pub fn render_preview_button(
        &self,
        workspace_handle: WeakEntity<Workspace>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let mut preview_type = None;

        if let Some(workspace) = self.workspace.upgrade() {
            workspace.update(cx, |workspace, cx| {
                // 检测当前激活项是否为Markdown编辑器
                if MarkdownPreviewView::resolve_active_item_as_markdown_editor(workspace, cx)
                    .is_some()
                {
                    preview_type = Some(PreviewType::Markdown);
                }
                // 检测当前激活项是否为SVG文件
                else if SvgPreviewView::resolve_active_item_as_svg_buffer(workspace, cx).is_some()
                {
                    preview_type = Some(PreviewType::Svg);
                }
                // 检测功能标志并判断是否为CSV编辑器
                else if cx.has_flag::<TabularDataPreviewFeatureFlag>()
                    && CsvPreviewView::resolve_active_item_as_csv_editor(workspace, cx).is_some()
                {
                    preview_type = Some(PreviewType::Csv);
                }
            });
        }

        let preview_type = preview_type?;

        // 根据预览类型匹配按钮ID、提示文本和对应操作
        let (button_id, tooltip_text, open_action, open_to_side_action, open_action_for_tooltip) =
            match preview_type {
                PreviewType::Markdown => (
                    "toggle-markdown-preview",
                    "预览 Markdown",
                    Box::new(MarkdownOpenPreview) as Box<dyn gpui::Action>,
                    Box::new(MarkdownOpenPreviewToTheSide) as Box<dyn gpui::Action>,
                    &markdown_preview::OpenPreview as &dyn gpui::Action,
                ),
                PreviewType::Svg => (
                    "toggle-svg-preview",
                    "预览 SVG",
                    Box::new(SvgOpenPreview) as Box<dyn gpui::Action>,
                    Box::new(SvgOpenPreviewToTheSide) as Box<dyn gpui::Action>,
                    &svg_preview::OpenPreview as &dyn gpui::Action,
                ),
                PreviewType::Csv => (
                    "toggle-csv-preview",
                    "预览 CSV",
                    Box::new(CsvOpenPreview) as Box<dyn gpui::Action>,
                    Box::new(CsvOpenPreviewToTheSide) as Box<dyn gpui::Action>,
                    &csv_preview::OpenPreview as &dyn gpui::Action,
                ),
            };

        // 定义Alt+点击组合键
        let alt_click = gpui::Keystroke {
            key: "click".into(),
            modifiers: Modifiers::alt(),
            ..Default::default()
        };

        // 创建预览图标按钮
        let button = IconButton::new(button_id, IconName::Eye)
            .icon_size(IconSize::Small)
            .style(ButtonStyle::Subtle)
            .tooltip(move |_window, cx| {
                Tooltip::with_meta(
                    tooltip_text,
                    Some(open_action_for_tooltip),
                    format!(
                        "{} 在分栏中打开",
                        text_for_keystroke(&alt_click.modifiers, &alt_click.key, cx)
                    ),
                    cx,
                )
            })
            .on_click(move |_, window, cx| {
                if let Some(workspace) = workspace_handle.upgrade() {
                    workspace.update(cx, |_, cx| {
                        // 按住Alt键点击则在侧边分栏打开，否则直接打开
                        if window.modifiers().alt {
                            window.dispatch_action(open_to_side_action.boxed_clone(), cx);
                        } else {
                            window.dispatch_action(open_action.boxed_clone(), cx);
                        }
                    });
                }
            });

        Some(button.into_any_element())
    }
}