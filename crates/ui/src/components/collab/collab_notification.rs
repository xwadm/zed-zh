use gpui::{AnyElement, SharedUri, prelude::*};
use smallvec::SmallVec;

use crate::{Avatar, prelude::*};

/// 协作通知组件，用于展示协作相关的通知（通话、屏幕共享、项目共享、联系人请求等）
#[derive(IntoElement, RegisterComponent)]
pub struct CollabNotification {
    avatar_uri: SharedUri,
    accept_button: Button,
    dismiss_button: Button,
    children: SmallVec<[AnyElement; 2]>,
}

impl CollabNotification {
    /// 创建协作通知
    /// - avatar_uri: 发起通知用户的头像地址
    /// - accept_button: 接受/确认按钮
    /// - dismiss_button: 拒绝/忽略按钮
    pub fn new(
        avatar_uri: impl Into<SharedUri>,
        accept_button: Button,
        dismiss_button: Button,
    ) -> Self {
        Self {
            avatar_uri: avatar_uri.into(),
            accept_button,
            dismiss_button,
            children: SmallVec::new(),
        }
    }
}

impl ParentElement for CollabNotification {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements)
    }
}

impl RenderOnce for CollabNotification {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        h_flex()
            .p_2()
            .size_full()
            .text_ui(cx)
            .justify_between()
            .overflow_hidden()
            .elevation_3(cx)
            .gap_1()
            .child(
                h_flex()
                    .min_w_0()
                    .gap_4()
                    // 显示用户头像
                    .child(Avatar::new(self.avatar_uri).size(px(40.)))
                    // 通知文本内容
                    .child(v_flex().truncate().children(self.children)),
            )
            .child(
                v_flex()
                    .items_center()
                    // 操作按钮：接受 + 拒绝
                    .child(self.accept_button)
                    .child(self.dismiss_button),
            )
    }
}

impl Component for CollabNotification {
    fn scope() -> ComponentScope {
        ComponentScope::Collaboration
    }

    /// 组件预览效果
    fn preview(_window: &mut Window, _cx: &mut App) -> Option<AnyElement> {
        let avatar = "https://avatars.githubusercontent.com/u/67129314?v=4";
        let container = || div().h(px(72.)).w(px(400.)); // 通知窗口固定尺寸

        // 通话 & 项目共享示例
        let call_examples = vec![
            single_example(
                "来电邀请",
                container()
                    .child(
                        CollabNotification::new(
                            avatar,
                            Button::new("accept", "接受"),
                            Button::new("decline", "拒绝"),
                        )
                        .child(Label::new("邀请你加入通话")),
                    )
                    .into_any_element(),
            ),
            single_example(
                "屏幕共享请求",
                container()
                    .child(
                        CollabNotification::new(
                            avatar,
                            Button::new("accept", "查看"),
                            Button::new("decline", "忽略"),
                        )
                        .child(Label::new("正在共享屏幕")),
                    )
                    .into_any_element(),
            ),
            single_example(
                "项目共享",
                container()
                    .child(
                        CollabNotification::new(
                            avatar,
                            Button::new("accept", "打开"),
                            Button::new("decline", "关闭"),
                        )
                        .child(Label::new("共享了一个项目"))
                        .child(Label::new("zed").color(Color::Muted)),
                    )
                    .into_any_element(),
            ),
            single_example(
                "内容溢出适配",
                container()
                    .child(
                        CollabNotification::new(
                            avatar,
                            Button::new("accept", "接受"),
                            Button::new("decline", "拒绝"),
                        )
                        .child(Label::new(
                            "a_very_long_username_that_might_overflow 正在 Zed 中共享项目：",
                        ))
                        .child(
                            Label::new("zed-cloud, zed, edit-prediction-bench, zed.dev")
                                .color(Color::Muted),
                        ),
                    )
                    .into_any_element(),
            ),
        ];

        // 联系人 & 频道通知示例
        let toast_examples = vec![
            single_example(
                "联系人请求",
                container()
                    .child(
                        CollabNotification::new(
                            avatar,
                            Button::new("accept", "接受"),
                            Button::new("decline", "拒绝"),
                        )
                        .child(Label::new("maxbrunsfeld 请求添加你为联系人")),
                    )
                    .into_any_element(),
            ),
            single_example(
                "联系人请求已接受",
                container()
                    .child(
                        CollabNotification::new(
                            avatar,
                            Button::new("dismiss", "关闭"),
                            Button::new("close", "关闭"),
                        )
                        .child(Label::new("maxbrunsfeld 接受了你的联系人请求")),
                    )
                    .into_any_element(),
            ),
            single_example(
                "频道邀请",
                container()
                    .child(
                        CollabNotification::new(
                            avatar,
                            Button::new("accept", "接受"),
                            Button::new("decline", "拒绝"),
                        )
                        .child(Label::new(
                            "maxbrunsfeld 邀请你加入 #zed 频道",
                        )),
                    )
                    .into_any_element(),
            ),
        ];

        Some(
            v_flex()
                .gap_6()
                .child(example_group_with_title("通话 & 项目共享", call_examples).vertical())
                .child(
                    example_group_with_title("联系人 & 频道通知", toast_examples).vertical(),
                )
                .into_any_element(),
        )
    }
}