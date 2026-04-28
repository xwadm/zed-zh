use std::sync::Arc;

use ai_onboarding::{AgentPanelOnboardingCard, PlanDefinitions};
use client::zed_urls;
use gpui::{AnyElement, App, IntoElement, RenderOnce, Window};
use ui::{Divider, Tooltip, prelude::*};

/// 试用到期升级提示组件
/// 当 Zed Pro 试用过期后，展示给用户的付费升级卡片
#[derive(IntoElement, RegisterComponent)]
pub struct EndTrialUpsell {
    /// 关闭升级提示的回调函数
    dismiss_upsell: Arc<dyn Fn(&mut Window, &mut App)>,
}

impl EndTrialUpsell {
    /// 创建一个试用到期升级提示组件
    /// 参数：关闭提示的回调
    pub fn new(dismiss_upsell: Arc<dyn Fn(&mut Window, &mut App)>) -> Self {
        Self { dismiss_upsell }
    }
}

impl RenderOnce for EndTrialUpsell {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        // Pro 版本功能介绍区域
        let pro_section = v_flex()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Label::new("Pro")
                            .size(LabelSize::Small)
                            .color(Color::Accent)
                            .buffer_font(cx),
                    )
                    .child(Divider::horizontal()),
            )
            // 展示 Pro 版包含的功能
            .child(PlanDefinitions.pro_plan())
            .child(
                Button::new("cta-button", "升级到 Zed Pro")
                    .full_width()
                    .style(ButtonStyle::Tinted(ui::TintColor::Accent))
                    .on_click(move |_, _window, cx| {
                        // 埋点：用户点击升级按钮
                        telemetry::event!("Upgrade To Pro Clicked", state = "end-of-trial");
                        // 打开升级页面
                        cx.open_url(&zed_urls::upgrade_to_zed_pro_url(cx))
                    }),
            );

        // 免费版功能介绍区域
        let free_section = v_flex()
            .mt_1p5()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Label::new("免费版")
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                            .buffer_font(cx),
                    )
                    .child(
                        Label::new("(当前套餐)")
                            .size(LabelSize::Small)
                            .color(Color::Custom(cx.theme().colors().text_muted.opacity(0.6)))
                            .buffer_font(cx),
                    )
                    .child(Divider::horizontal()),
            )
            // 展示免费版包含的功能
            .child(PlanDefinitions.free_plan());

        // 主体卡片：试用到期提示
        AgentPanelOnboardingCard::new()
            .child(Headline::new("你的 Zed Pro 试用已到期"))
            .child(
                Label::new("你已自动切换回免费版套餐。")
                    .color(Color::Muted)
                    .mb_2(),
            )
            // 放入 Pro 版介绍
            .child(pro_section)
            // 放入免费版介绍
            .child(free_section)
            // 右上角关闭按钮
            .child(
                h_flex().absolute().top_4().right_4().child(
                    IconButton::new("dismiss_onboarding", IconName::Close)
                        .icon_size(IconSize::Small)
                        .tooltip(Tooltip::text("关闭"))
                        .on_click({
                            let callback = self.dismiss_upsell.clone();
                            move |_, window, cx| {
                                // 埋点：用户关闭横幅
                                telemetry::event!("Banner Dismissed", source = "AI Onboarding");
                                callback(window, cx)
                            }
                        }),
                ),
            )
    }
}

/// 组件注册（用于编辑器内组件预览）
impl Component for EndTrialUpsell {
    fn scope() -> ComponentScope {
        ComponentScope::Onboarding
    }

    fn name() -> &'static str {
        "试用到期升级提示横幅"
    }

    fn sort_name() -> &'static str {
        "试用到期升级提示横幅"
    }

    /// 预览渲染
    fn preview(_window: &mut Window, _cx: &mut App) -> Option<AnyElement> {
        Some(
            v_flex()
                .child(EndTrialUpsell {
                    dismiss_upsell: Arc::new(|_, _| {}),
                })
                .into_any_element(),
        )
    }
}