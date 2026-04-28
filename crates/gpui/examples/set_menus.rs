#![cfg_attr(target_family = "wasm", no_main)]

use gpui::{
    App, Context, Global, Menu, MenuItem, SharedString, SystemMenuType, Window, WindowOptions,
    actions, div, prelude::*,
};
use gpui_platform::application;

/// 设置菜单示例视图
struct SetMenus;

impl Render for SetMenus {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .bg(gpui::white())
            .size_full()
            .justify_center()
            .items_center()
            .text_xl()
            .text_color(gpui::black())
            .child("设置菜单示例")
    }
}

/// 运行示例程序
fn run_example() {
    application().run(|cx: &mut App| {
        cx.set_global(AppState::new());

        // 将菜单栏激活到前台（便于查看菜单栏）
        cx.activate(true);
        // 注册退出函数，供菜单栏中的 MenuItem::action 调用
        cx.on_action(quit);
        cx.on_action(toggle_check);
        // 添加菜单项
        set_app_menus(cx);
        cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| SetMenus {}))
            .unwrap();
    });
}

#[cfg(not(target_family = "wasm"))]
fn main() {
    run_example();
}

#[cfg(target_family = "wasm")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() {
    gpui_platform::web_init();
    run_example();
}

/// 视图展示模式
#[derive(PartialEq)]
enum ViewMode {
    List,
    Grid,
}

impl ViewMode {
    /// 切换展示模式
    fn toggle(&mut self) {
        *self = match self {
            ViewMode::List => ViewMode::Grid,
            ViewMode::Grid => ViewMode::List,
        }
    }
}

impl Into<SharedString> for ViewMode {
    fn into(self) -> SharedString {
        match self {
            ViewMode::List => "列表模式",
            ViewMode::Grid => "网格模式",
        }
        .into()
    }
}

/// 应用全局状态
struct AppState {
    view_mode: ViewMode,
}

impl AppState {
    /// 初始化应用状态
    fn new() -> Self {
        Self {
            view_mode: ViewMode::List,
        }
    }
}

impl Global for AppState {}

/// 配置应用菜单栏
fn set_app_menus(cx: &mut App) {
    let app_state = cx.global::<AppState>();
    cx.set_menus([Menu::new("set_menus").items([
        MenuItem::os_submenu("服务", SystemMenuType::Services),
        MenuItem::separator(),
        MenuItem::action("禁用项", gpui::NoAction).disabled(true),
        MenuItem::submenu(Menu::new("禁用子菜单").disabled(true)),
        MenuItem::separator(),
        MenuItem::action("列表模式", ToggleCheck).checked(app_state.view_mode == ViewMode::List),
        MenuItem::submenu(
            Menu::new("展示模式").items([
                MenuItem::action(ViewMode::List, ToggleCheck)
                    .checked(app_state.view_mode == ViewMode::List),
                MenuItem::action(ViewMode::Grid, ToggleCheck)
                    .checked(app_state.view_mode == ViewMode::Grid),
            ]),
        ),
        MenuItem::separator(),
        MenuItem::action("退出", Quit),
    ])]);
}

// 使用 actions! 宏关联操作（也可使用 Action 派生宏）
actions!(set_menus, [退出, 切换选中状态]);

/// 定义应用退出函数
fn quit(_: &退出, cx: &mut App) {
    println!("正在优雅退出应用...");
    cx.quit();
}

/// 切换选中状态
fn toggle_check(_: &切换选中状态, cx: &mut App) {
    let app_state = cx.global_mut::<AppState>();
    app_state.view_mode.toggle();
    set_app_menus(cx);
}