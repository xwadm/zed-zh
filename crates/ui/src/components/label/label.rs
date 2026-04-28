use std::ops::Range;

use crate::{LabelLike, prelude::*};
use gpui::{HighlightStyle, StyleRefinement, StyledText};

/// 表示 UI 中标签元素的结构体。
///
/// `Label` 结构体存储了标签文本以及标签元素的通用属性。
/// 它提供了修改这些属性的方法。
///
/// # 示例
///
/// ```
/// use ui::prelude::*;
///
/// Label::new("Hello, World!");
/// ```
///
/// **一个带颜色的标签**，例如用于标注危险操作：
///
/// ```
/// use ui::prelude::*;
///
/// let my_label = Label::new("Delete").color(Color::Error);
/// ```
///
/// **一个带删除线的标签**，例如用于标注已删除的内容：
///
/// ```
/// use ui::prelude::*;
///
/// let my_label = Label::new("Deleted").strikethrough();
/// ```
#[derive(IntoElement, RegisterComponent)]
pub struct Label {
    base: LabelLike,
    label: SharedString,
    render_code_spans: bool,
}

impl Label {
    /// 使用给定的文本创建一个新的 [`Label`]。
    ///
    /// # 示例
    ///
    /// ```
    /// use ui::prelude::*;
    ///
    /// let my_label = Label::new("Hello, World!");
    /// ```
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            base: LabelLike::new(),
            label: label.into(),
            render_code_spans: false,
        }
    }

    /// 当启用时，被反引号包裹的文本（例如 `` `code` ``）将使用等宽字体渲染。
    pub fn render_code_spans(mut self) -> Self {
        self.render_code_spans = true;
        self
    }

    /// 设置 [`Label`] 的文本。
    pub fn set_text(&mut self, text: impl Into<SharedString>) {
        self.label = text.into();
    }

    /// 从开头截断标签文本，保持末尾部分可见。
    pub fn truncate_start(mut self) -> Self {
        self.base = self.base.truncate_start();
        self
    }
}

// 样式方法。
impl Label {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.base.style()
    }

    gpui::margin_style_methods!({
        visibility: pub
    });

    pub fn flex_1(mut self) -> Self {
        self.style().flex_grow = Some(1.);
        self.style().flex_shrink = Some(1.);
        self.style().flex_basis = Some(gpui::relative(0.).into());
        self
    }

    pub fn flex_none(mut self) -> Self {
        self.style().flex_grow = Some(0.);
        self.style().flex_shrink = Some(0.);
        self
    }

    pub fn flex_grow(mut self) -> Self {
        self.style().flex_grow = Some(1.);
        self
    }

    pub fn flex_shrink(mut self) -> Self {
        self.style().flex_shrink = Some(1.);
        self
    }

    pub fn flex_shrink_0(mut self) -> Self {
        self.style().flex_shrink = Some(0.);
        self
    }
}

impl LabelCommon for Label {
    /// 使用 [`LabelSize`] 设置标签的尺寸。
    ///
    /// # 示例
    ///
    /// ```
    /// use ui::prelude::*;
    ///
    /// let my_label = Label::new("Hello, World!").size(LabelSize::Small);
    /// ```
    fn size(mut self, size: LabelSize) -> Self {
        self.base = self.base.size(size);
        self
    }

    /// 使用 [`FontWeight`] 设置标签的字重。
    ///
    /// # 示例
    ///
    /// ```
    /// use gpui::FontWeight;
    /// use ui::prelude::*;
    ///
    /// let my_label = Label::new("Hello, World!").weight(FontWeight::BOLD);
    /// ```
    fn weight(mut self, weight: gpui::FontWeight) -> Self {
        self.base = self.base.weight(weight);
        self
    }

    /// 使用 [`LineHeightStyle`] 设置标签的行高样式。
    ///
    /// # 示例
    ///
    /// ```
    /// use ui::prelude::*;
    ///
    /// let my_label = Label::new("Hello, World!").line_height_style(LineHeightStyle::UiLabel);
    /// ```
    fn line_height_style(mut self, line_height_style: LineHeightStyle) -> Self {
        self.base = self.base.line_height_style(line_height_style);
        self
    }

    /// 使用 [`Color`] 设置标签的颜色。
    ///
    /// # 示例
    ///
    /// ```
    /// use ui::prelude::*;
    ///
    /// let my_label = Label::new("Hello, World!").color(Color::Accent);
    /// ```
    fn color(mut self, color: Color) -> Self {
        self.base = self.base.color(color);
        self
    }

    /// 设置标签的删除线属性。
    ///
    /// # 示例
    ///
    /// ```
    /// use ui::prelude::*;
    ///
    /// let my_label = Label::new("Hello, World!").strikethrough();
    /// ```
    fn strikethrough(mut self) -> Self {
        self.base = self.base.strikethrough();
        self
    }

    /// 设置标签的斜体属性。
    ///
    /// # 示例
    ///
    /// ```
    /// use ui::prelude::*;
    ///
    /// let my_label = Label::new("Hello, World!").italic();
    /// ```
    fn italic(mut self) -> Self {
        self.base = self.base.italic();
        self
    }

    /// 设置标签颜色的透明度。
    ///
    /// # 示例
    ///
    /// ```
    /// use ui::prelude::*;
    ///
    /// let my_label = Label::new("Hello, World!").alpha(0.5);
    /// ```
    fn alpha(mut self, alpha: f32) -> Self {
        self.base = self.base.alpha(alpha);
        self
    }

    fn underline(mut self) -> Self {
        self.base = self.base.underline();
        self
    }

    /// 在必要时用省略号 (`…`) 截断溢出的文本。
    fn truncate(mut self) -> Self {
        self.base = self.base.truncate();
        self
    }

    fn single_line(mut self) -> Self {
        self.label = SharedString::from(self.label.replace('\n', "⏎"));
        self.base = self.base.single_line();
        self
    }

    fn buffer_font(mut self, cx: &App) -> Self {
        self.base = self.base.buffer_font(cx);
        self
    }

    /// 将标签样式设置为类似内联代码的样式。
    fn inline_code(mut self, cx: &App) -> Self {
        self.base = self.base.inline_code(cx);
        self
    }
}

impl RenderOnce for Label {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        if self.render_code_spans {
            if let Some((stripped, code_ranges)) = parse_backtick_spans(&self.label) {
                let buffer_font_family = theme::theme_settings(cx).buffer_font(cx).family.clone();
                let background_color = cx.theme().colors().element_background;

                let highlights = code_ranges.iter().map(|range| {
                    (
                        range.clone(),
                        HighlightStyle {
                            background_color: Some(background_color),
                            ..Default::default()
                        },
                    )
                });

                let font_overrides = code_ranges
                    .iter()
                    .map(|range| (range.clone(), buffer_font_family.clone()));

                return self.base.child(
                    StyledText::new(stripped)
                        .with_highlights(highlights)
                        .with_font_family_overrides(font_overrides),
                );
            }
        }
        self.base.child(self.label)
    }
}

/// 从字符串中解析由反引号分隔的代码片段。
///
/// 如果没有匹配的反引号对，则返回 `None`。
/// 否则返回去除反引号后的文本，以及代码片段在去除反引号字符串中的字节范围。
fn parse_backtick_spans(text: &str) -> Option<(SharedString, Vec<Range<usize>>)> {
    if !text.contains('`') {
        return None;
    }

    let mut stripped = String::with_capacity(text.len());
    let mut code_ranges = Vec::new();
    let mut in_code = false;
    let mut code_start = 0;

    for ch in text.chars() {
        if ch == '`' {
            if in_code {
                code_ranges.push(code_start..stripped.len());
            } else {
                code_start = stripped.len();
            }
            in_code = !in_code;
        } else {
            stripped.push(ch);
        }
    }

    if code_ranges.is_empty() {
        return None;
    }

    Some((SharedString::from(stripped), code_ranges))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_backtick_spans_no_backticks() {
        assert_eq!(parse_backtick_spans("plain text"), None);
    }

    #[test]
    fn test_parse_backtick_spans_single_span() {
        let (text, ranges) = parse_backtick_spans("use `zed` to open").unwrap();
        assert_eq!(text.as_ref(), "use zed to open");
        assert_eq!(ranges, vec![4..7]);
    }

    #[test]
    fn test_parse_backtick_spans_multiple_spans() {
        let (text, ranges) = parse_backtick_spans("flags `-e` or `-n`").unwrap();
        assert_eq!(text.as_ref(), "flags -e or -n");
        assert_eq!(ranges, vec![6..8, 12..14]);
    }

    #[test]
    fn test_parse_backtick_spans_unmatched_backtick() {
        // 尾随未匹配的反引号不应产生代码范围
        assert_eq!(parse_backtick_spans("trailing `backtick"), None);
    }

    #[test]
    fn test_parse_backtick_spans_empty_span() {
        let (text, ranges) = parse_backtick_spans("empty `` span").unwrap();
        assert_eq!(text.as_ref(), "empty  span");
        assert_eq!(ranges, vec![6..6]);
    }
}

impl Component for Label {
    fn scope() -> ComponentScope {
        ComponentScope::Typography
    }

    fn description() -> Option<&'static str> {
        Some("一个支持多种样式、尺寸和格式化选项的文本标签组件。")
    }

    fn preview(_window: &mut Window, cx: &mut App) -> Option<AnyElement> {
        Some(
            v_flex()
                .gap_6()
                .children(vec![
                    example_group_with_title(
                        "尺寸",
                        vec![
                            single_example("默认", Label::new("Project Explorer").into_any_element()),
                            single_example("小", Label::new("File: main.rs").size(LabelSize::Small).into_any_element()),
                            single_example("大", Label::new("Welcome to Zed").size(LabelSize::Large).into_any_element()),
                        ],
                    ),
                    example_group_with_title(
                        "颜色",
                        vec![
                            single_example("默认", Label::new("Status: Ready").into_any_element()),
                            single_example("强调色", Label::new("New Update Available").color(Color::Accent).into_any_element()),
                            single_example("错误", Label::new("Build Failed").color(Color::Error).into_any_element()),
                        ],
                    ),
                    example_group_with_title(
                        "样式",
                        vec![
                            single_example("默认", Label::new("Normal Text").into_any_element()),
                            single_example("加粗", Label::new("Important Notice").weight(gpui::FontWeight::BOLD).into_any_element()),
                            single_example("斜体", Label::new("Code Comment").italic().into_any_element()),
                            single_example("删除线", Label::new("Deprecated Feature").strikethrough().into_any_element()),
                            single_example("下划线", Label::new("Clickable Link").underline().into_any_element()),
                            single_example("内联代码", Label::new("fn main() {}").inline_code(cx).into_any_element()),
                        ],
                    ),
                    example_group_with_title(
                        "行高样式",
                        vec![
                            single_example("默认", Label::new("Multi-line\nText\nExample").into_any_element()),
                            single_example("UI 标签", Label::new("Compact\nUI\nLabel").line_height_style(LineHeightStyle::UiLabel).into_any_element()),
                        ],
                    ),
                    example_group_with_title(
                        "特殊情况",
                        vec![
                            single_example("单行", Label::new("Line 1\nLine 2\nLine 3").single_line().into_any_element()),
                            single_example("常规截断", div().max_w_24().child(Label::new("This is a very long file name that should be truncated: very_long_file_name_with_many_words.rs").truncate()).into_any_element()),
                            single_example("头部截断", div().max_w_24().child(Label::new("zed/crates/ui/src/components/label/truncate/label/label.rs").truncate_start()).into_any_element()),
                        ],
                    ),
                ])
                .into_any_element()
        )
    }
}