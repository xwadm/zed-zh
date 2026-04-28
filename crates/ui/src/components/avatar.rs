use crate::prelude::*;

use documented::Documented;
use gpui::{AnyElement, Hsla, ImageSource, Img, IntoElement, Styled, img};

/// 用于渲染用户头像的 UI 组件，支持自定义外观选项
///
/// # 示例
///
/// ```
/// use ui::Avatar;
///
/// Avatar::new("path/to/image.png")
///     .grayscale(true)
///     .border_color(gpui::red());
/// ```
#[derive(IntoElement, Documented, RegisterComponent)]
pub struct Avatar {
    /// 头像图片元素
    image: Img,
    /// 头像尺寸（可选）
    size: Option<AbsoluteLength>,
    /// 边框颜色（可选）
    border_color: Option<Hsla>,
    /// 角标指示器（可选，如静音、在线状态）
    indicator: Option<AnyElement>,
}

impl Avatar {
    /// 使用指定的图片源创建一个新的头像组件
    pub fn new(src: impl Into<ImageSource>) -> Self {
        Avatar {
            image: img(src),
            size: None,
            border_color: None,
            indicator: None,
        }
    }

    /// 为头像图片应用灰度滤镜
    ///
    /// # 示例
    ///
    /// ```
    /// use ui::Avatar;
    ///
    /// let avatar = Avatar::new("path/to/image.png").grayscale(true);
    /// ```
    pub fn grayscale(mut self, grayscale: bool) -> Self {
        self.image = self.image.grayscale(grayscale);
        self
    }

    /// 设置头像的边框颜色
    ///
    /// 通常用于将边框颜色与父元素背景匹配，
    /// 制造出裁剪形状的视觉效果（例如堆叠头像）
    pub fn border_color(mut self, color: impl Into<Hsla>) -> Self {
        self.border_color = Some(color.into());
        self
    }

    /// 自定义头像尺寸，默认大小为 1rem
    pub fn size<L: Into<AbsoluteLength>>(mut self, size: impl Into<Option<L>>) -> Self {
        self.size = size.into().map(Into::into);
        self
    }

    /// 设置显示在头像上的状态指示器（可选）
    pub fn indicator<E: IntoElement>(mut self, indicator: impl Into<Option<E>>) -> Self {
        self.indicator = indicator.into().map(IntoElement::into_any_element);
        self
    }
}

impl RenderOnce for Avatar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        // 如果设置了边框颜色，则启用 1px 边框
        let border_width = if self.border_color.is_some() {
            px(1.)
        } else {
            px(0.)
        };

        // 头像图片大小：默认 1rem
        let image_size = self.size.unwrap_or_else(|| rems(1.).into());
        // 容器大小 = 图片大小 + 双边边框
        let container_size = image_size.to_pixels(window.rem_size()) + border_width * 2.;

        div()
            // 容器尺寸
            .size(container_size)
            // 圆形裁剪
            .rounded_full()
            // 可选：设置边框
            .when_some(self.border_color, |this, color| {
                this.border(border_width).border_color(color)
            })
            // 头像图片主体
            .child(
                self.image
                    .size(image_size)
                    .rounded_full()
                    // 禁用状态背景色
                    .bg(cx.theme().colors().element_disabled)
                    // 图片加载失败时的占位图标
                    .with_fallback(|| {
                        h_flex()
                            .size_full()
                            .justify_center()
                            .child(
                                Icon::new(IconName::Person)
                                    .color(Color::Muted)
                                    .size(IconSize::Small),
                            )
                            .into_any_element()
                    }),
            )
            // 可选状态指示器
            .children(self.indicator.map(|indicator| div().child(indicator)))
    }
}

use gpui::AnyView;

/// 协作成员的音频状态，用于在头像上可视化展示
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy)]
pub enum AudioStatus {
    /// 麦克风已静音
    Muted,
    /// 麦克风静音 + 音频播放关闭（完全听不见）
    Deafened,
}

/// 展示用户音频状态的头像角标组件
#[derive(IntoElement)]
pub struct AvatarAudioStatusIndicator {
    audio_status: AudioStatus,
    tooltip: Option<Box<dyn Fn(&mut Window, &mut App) -> AnyView>>,
}

impl AvatarAudioStatusIndicator {
    /// 创建一个新的音频状态指示器
    pub fn new(audio_status: AudioStatus) -> Self {
        Self {
            audio_status,
            tooltip: None,
        }
    }

    /// 为指示器设置悬浮提示文本
    pub fn tooltip(mut self, tooltip: impl Fn(&mut Window, &mut App) -> AnyView + 'static) -> Self {
        self.tooltip = Some(Box::new(tooltip));
        self
    }
}

impl RenderOnce for AvatarAudioStatusIndicator {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let icon_size = IconSize::Indicator;

        let width_in_px = icon_size.rems() * window.rem_size();
        let padding_x = px(4.);

        div()
            // 绝对定位
            .absolute()
            // 放在头像右下角
            .bottom(rems_from_px(-3.))
            .right(rems_from_px(-6.))
            .w(width_in_px + padding_x)
            .h(icon_size.rems())
            .child(
                h_flex()
                    .id("muted-indicator")
                    .justify_center()
                    .px(padding_x)
                    .py(px(2.))
                    // 错误状态背景
                    .bg(cx.theme().status().error_background)
                    .rounded_sm()
                    // 根据状态显示不同图标
                    .child(
                        Icon::new(match self.audio_status {
                            AudioStatus::Muted => IconName::MicMute,
                            AudioStatus::Deafened => IconName::AudioOff,
                        })
                        .size(icon_size)
                        .color(Color::Error),
                    )
                    // 可选悬浮提示
                    .when_some(self.tooltip, |this, tooltip| {
                        this.tooltip(move |window, cx| tooltip(window, cx))
                    }),
            )
    }
}

/// 协作成员的可用状态
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy)]
pub enum CollaboratorAvailability {
    /// 空闲
    Free,
    /// 忙碌
    Busy,
}

/// 展示协作成员在线/忙碌状态的头像角标
#[derive(IntoElement)]
pub struct AvatarAvailabilityIndicator {
    availability: CollaboratorAvailability,
    avatar_size: Option<Pixels>,
}

impl AvatarAvailabilityIndicator {
    /// 创建一个新的可用状态指示器
    pub fn new(availability: CollaboratorAvailability) -> Self {
        Self {
            availability,
            avatar_size: None,
        }
    }

    /// 设置该指示器所依附的头像大小
    pub fn avatar_size(mut self, size: impl Into<Option<Pixels>>) -> Self {
        self.avatar_size = size.into();
        self
    }
}

impl RenderOnce for AvatarAvailabilityIndicator {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let avatar_size = self.avatar_size.unwrap_or_else(|| window.rem_size());

        // 技巧：使用整数尺寸避免指示器变成椭圆
        let indicator_size = (avatar_size * 0.4).round();

        div()
            .absolute()
            .bottom_0()
            .right_0()
            .size(indicator_size)
            .rounded(indicator_size)
            // 空闲=绿色，忙碌=红色
            .bg(match self.availability {
                CollaboratorAvailability::Free => cx.theme().status().created,
                CollaboratorAvailability::Busy => cx.theme().status().deleted,
            })
    }
}

// 使用 `workspace: open component-preview` 查看组件预览
impl Component for Avatar {
    fn scope() -> ComponentScope {
        ComponentScope::Collaboration
    }

    fn description() -> Option<&'static str> {
        Some(Avatar::DOCS)
    }

    /// 组件预览示例（编辑器内可视化预览）
    fn preview(_window: &mut Window, cx: &mut App) -> Option<AnyElement> {
        let example_avatar = "https://avatars.githubusercontent.com/u/1714999?v=4";

        Some(
            v_flex()
                .gap_6()
                .children(vec![
                    example_group(vec![
                        single_example("默认样式", Avatar::new(example_avatar).into_any_element()),
                        single_example(
                            "灰度效果",
                            Avatar::new(example_avatar)
                                .grayscale(true)
                                .into_any_element(),
                        ),
                        single_example(
                            "带边框",
                            Avatar::new(example_avatar)
                                .border_color(cx.theme().colors().border)
                                .into_any_element(),
                        ).description("通过将边框颜色设置为与背景匹配，可以在头像周围制造视觉空隙效果。"),
                    ]),
                    example_group_with_title(
                        "角标样式",
                        vec![
                            single_example(
                                "麦克风静音",
                                Avatar::new(example_avatar)
                                    .indicator(AvatarAudioStatusIndicator::new(AudioStatus::Muted))
                                    .into_any_element(),
                            ).description("表示协作成员的麦克风已静音。"),
                            single_example(
                                "完全静音（听不见）",
                                Avatar::new(example_avatar)
                                    .indicator(AvatarAudioStatusIndicator::new(
                                        AudioStatus::Deafened,
                                    ))
                                    .into_any_element(),
                            ).description("表示麦克风和音频播放都已关闭。"),
                            single_example(
                                "状态：空闲",
                                Avatar::new(example_avatar)
                                    .indicator(AvatarAvailabilityIndicator::new(
                                        CollaboratorAvailability::Free,
                                    ))
                                    .into_any_element(),
                            ).description("表示用户空闲，通常是不在通话中。"),
                            single_example(
                                "状态：忙碌",
                                Avatar::new(example_avatar)
                                    .indicator(AvatarAvailabilityIndicator::new(
                                        CollaboratorAvailability::Busy,
                                    ))
                                    .into_any_element(),
                            ).description("表示用户忙碌，通常是在频道或通话中。"),
                        ],
                    ),
                ])
                .into_any_element(),
        )
    }
}