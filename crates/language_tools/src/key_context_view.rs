use gpui::{
    Action, App, AppContext as _, Entity, EventEmitter, FocusHandle, Focusable,
    KeyBindingContextPredicate, KeyContext, Keystroke, MouseButton, Render, Subscription, Task,
    actions,
};
use itertools::Itertools;
use serde_json::json;
use ui::{Button, ButtonStyle};
use ui::{
    ButtonCommon, Clickable, Context, FluentBuilder, InteractiveElement, Label, LabelCommon,
    LabelSize, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div,
    h_flex, px, v_flex,
};
use workspace::{Item, SplitDirection, Workspace};

// 定义开发者操作：打开按键上下文调试视图
actions!(
    dev,
    [
        /// 打开按键上下文视图，用于调试快捷键绑定
        OpenKeyContextView
    ]
);

/// 初始化模块：注册打开按键上下文视图的操作
pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &OpenKeyContextView, window, cx| {
            // 创建按键上下文调试面板
            let key_context_view = cx.new(|cx| KeyContextView::new(window, cx));
            // 在工作区右侧分割打开该面板
            workspace.split_item(
                SplitDirection::Right,
                Box::new(key_context_view),
                window,
                cx,
            )
        });
    })
    .detach();
}

/// 按键上下文调试视图
/// 用于实时查看当前按键上下文栈、匹配的快捷键、按键输入状态，方便调试自定义按键绑定
struct KeyContextView {
    /// 正在输入中的按键序列（多键快捷键）
    pending_keystrokes: Option<Vec<Keystroke>>,
    /// 最后一次输入的按键序列
    last_keystrokes: Option<SharedString>,
    /// 最后一次按键可能匹配的所有操作（操作名、上下文谓词、匹配状态）
    last_possibilities: Vec<(SharedString, SharedString, Option<bool>)>,
    /// 当前激活的上下文栈
    context_stack: Vec<KeyContext>,
    /// 焦点句柄，使面板可获得焦点
    focus_handle: FocusHandle,
    /// 订阅：监听按键输入、等待中的输入
    _subscriptions: [Subscription; 2],
}

impl KeyContextView {
    /// 创建按键上下文调试视图
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // 监听所有按键输入
        let sub1 = cx.observe_keystrokes(|this, e, _, cx| {
            let mut pending = this.pending_keystrokes.take().unwrap_or_default();
            pending.push(e.keystroke.clone());
            // 获取所有可能匹配的按键绑定
            let mut possibilities = cx.all_bindings_for_input(&pending);
            possibilities.reverse();

            // 记录最后一次按键序列
            this.last_keystrokes = Some(
                json!(pending.iter().map(|p| p.unparse()).join(" "))
                    .to_string()
                    .into(),
            );
            this.context_stack = e.context_stack.clone();

            // 解析所有可能的绑定及其匹配状态
            this.last_possibilities = possibilities
                .into_iter()
                .map(|binding| {
                    // 判断当前绑定是否匹配
                    let match_state = if let Some(predicate) = binding.predicate() {
                        if this.matches(&predicate) {
                            if this.action_matches(&e.action, binding.action()) {
                                Some(true)
                            } else {
                                Some(false)
                            }
                        } else {
                            None
                        }
                    } else if this.action_matches(&e.action, binding.action()) {
                        Some(true)
                    } else {
                        Some(false)
                    };

                    let predicate = if let Some(predicate) = binding.predicate() {
                        format!("{}", predicate)
                    } else {
                        "".to_string()
                    };

                    let mut name = binding.action().name();
                    if name == "zed::NoAction" {
                        name = "(null)"
                    }

                    (
                        name.to_owned().into(),
                        json!(predicate).to_string().into(),
                        match_state,
                    )
                })
                .collect();

            cx.notify();
        });

        // 监听等待中的输入（多键快捷键）
        let sub2 = cx.observe_pending_input(window, |this, window, cx| {
            this.pending_keystrokes = window.pending_input_keystrokes().map(|k| k.to_vec());
            if this.pending_keystrokes.is_some() {
                this.last_keystrokes.take();
            }
            cx.notify();
        });

        Self {
            context_stack: Vec::new(),
            pending_keystrokes: None,
            last_keystrokes: None,
            last_possibilities: Vec::new(),
            focus_handle: cx.focus_handle(),
            _subscriptions: [sub1, sub2],
        }
    }

    /// 设置上下文栈并刷新视图
    fn set_context_stack(&mut self, stack: Vec<KeyContext>, cx: &mut Context<Self>) {
        self.context_stack = stack;
        cx.notify()
    }

    /// 判断上下文谓词是否与当前上下文栈匹配
    fn matches(&self, predicate: &KeyBindingContextPredicate) -> bool {
        predicate.depth_of(&self.context_stack).is_some()
    }

    /// 判断两个操作是否等价
    fn action_matches(&self, a: &Option<Box<dyn Action>>, b: &dyn Action) -> bool {
        if let Some(last_action) = a {
            last_action.partial_eq(b)
        } else {
            b.name() == "zed::NoAction"
        }
    }
}

impl EventEmitter<()> for KeyContextView {}

/// 实现可焦点接口：让视图可以获得键盘焦点
impl Focusable for KeyContextView {
    fn focus_handle(&self, _: &App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

/// 实现工作区项目接口：可在 Zed 工作区中作为面板打开
impl Item for KeyContextView {
    type Event = ();

    fn to_item_events(_: &Self::Event, _: &mut dyn FnMut(workspace::item::ItemEvent)) {}

    /// 标签栏显示名称
    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        "键盘上下文".into()
    }

    fn telemetry_event_text(&self) -> Option<&'static str> {
        None
    }

    /// 允许分割
    fn can_split(&self) -> bool {
        true
    }

    /// 分割时克隆视图
    fn clone_on_split(
        &self,
        _workspace_id: Option<workspace::WorkspaceId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Option<Entity<Self>>>
    where
        Self: Sized,
    {
        Task::ready(Some(cx.new(|cx| KeyContextView::new(window, cx))))
    }
}

/// 渲染界面
impl Render for KeyContextView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl ui::IntoElement {
        use itertools::Itertools;

        let key_equivalents = cx.keyboard_mapper().get_key_equivalents();

        v_flex()
            .id("key-context-view")
            .overflow_scroll()
            .size_full()
            .max_h_full()
            .pt_4()
            .pl_4()
            .track_focus(&self.focus_handle)
            .key_context("KeyContextView")
            // 点击外部时更新上下文栈
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.last_keystrokes.take();
                    this.set_context_stack(window.context_stack(), cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Right,
                cx.listener(|_, _, window, cx| {
                    cx.defer_in(window, |this, window, cx| {
                        this.last_keystrokes.take();
                        this.set_context_stack(window.context_stack(), cx);
                    });
                }),
            )
            // 标题
            .child(Label::new("键盘上下文").size(LabelSize::Large))
            .child(Label::new("此视图用于查看当前上下文栈，方便创建自定义按键绑定。按下快捷键时，会显示所有可能匹配的上下文及命中结果。"))
            // 功能按钮
            .child(
                h_flex()
                    .mt_4()
                    .gap_4()
                    .child(
                        Button::new("open_documentation", "打开文档")
                            .style(ButtonStyle::Filled)
                            .on_click(|_, _, cx| cx.open_url("https://zed.dev/docs/key-bindings")),
                    )
                    .child(
                        Button::new("view_default_keymap", "查看默认按键映射")
                            .style(ButtonStyle::Filled)
                            .key_binding(ui::KeyBinding::for_action(
                                &zed_actions::OpenDefaultKeymap,
                                cx
                            ))
                            .on_click(|_, window, cx| {
                                window.dispatch_action(zed_actions::OpenDefaultKeymap.boxed_clone(), cx);
                            }),
                    )
                    .child(
                        Button::new("edit_your_keymap", "编辑自定义按键映射")
                            .style(ButtonStyle::Filled)
                            .key_binding(ui::KeyBinding::for_action(&zed_actions::OpenKeymapFile, cx))
                            .on_click(|_, window, cx| {
                                window.dispatch_action(zed_actions::OpenKeymapFile.boxed_clone(), cx);
                            }),
                    ),
            )
            // 当前上下文栈
            .child(
                Label::new("当前上下文栈")
                    .size(LabelSize::Large)
                    .mt_8(),
            )
            .children({
                self.context_stack.iter().enumerate().map(|(i, context)| {
                    let primary = context.primary().map(|e| e.key.clone()).unwrap_or_default();
                    let secondary = context
                        .secondary()
                        .map(|e| {
                            if let Some(value) = e.value.as_ref() {
                                format!("{}={}", e.key, value)
                            } else {
                                e.key.to_string()
                            }
                        })
                        .join(" ");
                    Label::new(format!("{} {}", primary, secondary)).ml(px(12. * (i + 1) as f32))
                })
            })
            // 最后一次按键
            .child(Label::new("Last Keystroke").mt_4().size(LabelSize::Large))
            // 显示正在等待更多输入（多键快捷键）
            .when_some(self.pending_keystrokes.as_ref(), |el, keystrokes| {
                el.child(
                    Label::new(format!(
                        "等待更多输入: {}",
                        keystrokes.iter().map(|k| k.unparse()).join(" ")
                    ))
                    .ml(px(12.)),
                )
            })
            // 显示最后一次按键及所有匹配结果
            .when_some(self.last_keystrokes.as_ref(), |el, keystrokes| {
                el.child(Label::new(format!("输入: {}", keystrokes)).ml_4())
                    .children(
                        self.last_possibilities
                            .iter()
                            .map(|(name, predicate, state)| {
                                let (text, color) = match state {
                                    Some(true) => ("(匹配成功)", ui::Color::Success),
                                    Some(false) => ("(优先级低)", ui::Color::Hint),
                                    None => ("(不匹配)", ui::Color::Error),
                                };
                                h_flex()
                                    .gap_2()
                                    .ml_8()
                                    .child(div().min_w(px(200.)).child(Label::new(name.clone())))
                                    .child(Label::new(predicate.clone()))
                                    .child(Label::new(text).color(color))
                            }),
                    )
            })
            // 显示按键等价映射（方便不用按 option 输入符号）
            .when_some(key_equivalents, |el, key_equivalents| {
                el.child(Label::new("Key Equivalents").mt_4().size(LabelSize::Large))
                    .child(Label::new("使用部分字符定义的快捷键已被重映射，无需按住 option 即可输入"))
                    .children(
                        key_equivalents
                            .iter()
                            .sorted()
                            .map(|(key, equivalent)| {
                                Label::new(format!("cmd-{} => cmd-{}", key, equivalent)).ml_8()
                            }),
                    )
            })
    }
}