#![cfg_attr(target_family = "wasm", no_main)]

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use gpui::{
    App, AppContext, AssetSource, Bounds, Context, ImageSource, KeyBinding, Menu, MenuItem, Point,
    SharedString, SharedUri, TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div,
    img, prelude::*, px, rgb, size,
};
#[cfg(not(target_family = "wasm"))]
use reqwest_client::ReqwestClient;

/// 自定义资源加载器
struct Assets {
    base: PathBuf,
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        fs::read(self.base.join(path))
            .map(|data| Some(std::borrow::Cow::Owned(data)))
            .map_err(|e| e.into())
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        fs::read_dir(self.base.join(path))
            .map(|entries| {
                entries
                    .filter_map(|entry| {
                        entry
                            .ok()
                            .and_then(|entry| entry.file_name().into_string().ok())
                            .map(SharedString::from)
                    })
                    .collect()
            })
            .map_err(|e| e.into())
    }
}

/// 图片展示容器组件
#[derive(IntoElement)]
struct ImageContainer {
    text: SharedString,
    src: ImageSource,
}

impl ImageContainer {
    pub fn new(text: impl Into<SharedString>, src: impl Into<ImageSource>) -> Self {
        Self {
            text: text.into(),
            src: src.into(),
        }
    }
}

impl RenderOnce for ImageContainer {
    fn render(self, _window: &mut Window, _: &mut App) -> impl IntoElement {
        div().child(
            div()
                .flex_row()
                .size_full()
                .gap_4()
                .child(self.text)
                .child(img(self.src).size(px(256.0))),
        )
    }
}

/// 图片展示示例主界面
struct ImageShowcase {
    local_resource: Arc<std::path::Path>,
    remote_resource: SharedUri,
    asset_resource: SharedString,
}

impl Render for ImageShowcase {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("main")
            .bg(gpui::white())
            .overflow_y_scroll()
            .p_5()
            .size_full()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .items_center()
                    .gap_8()
                    .child(img(
                        "https://github.com/zed-industries/zed/actions/workflows/ci.yml/badge.svg",
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_center()
                            .items_center()
                            .gap_8()
                            .child(ImageContainer::new(
                                "从本地文件加载的图片",
                                self.local_resource.clone(),
                            ))
                            .child(ImageContainer::new(
                                "从远程资源加载的图片",
                                self.remote_resource.clone(),
                            ))
                            .child(ImageContainer::new(
                                "从内置资源加载的图片",
                                self.asset_resource.clone(),
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_8()
                            .child(
                                div()
                                    .flex_col()
                                    .child("自动宽度")
                                    .child(img("https://picsum.photos/800/400").h(px(180.))),
                            )
                            .child(
                                div()
                                    .flex_col()
                                    .child("自动高度")
                                    .child(img("https://picsum.photos/800/400").w(px(180.))),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .justify_center()
                            .items_center()
                            .w_full()
                            .border_1()
                            .border_color(rgb(0xC0C0C0))
                            .child("最大宽度100%的图片")
                            .child(img("https://picsum.photos/800/400").max_w_full()),
                    ),
            )
    }
}

/// 定义应用操作
actions!(image, [退出]);

/// 运行示例程序
fn run_example() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    #[cfg(not(target_family = "wasm"))]
    let app = gpui_platform::application();
    #[cfg(target_family = "wasm")]
    let app = gpui_platform::application();
    app.with_assets(Assets {
        base: manifest_dir.join("examples"),
    })
    .run(move |cx: &mut App| {
        #[cfg(not(target_family = "wasm"))]
        {
            let http_client = ReqwestClient::user_agent("gpui example").unwrap();
            cx.set_http_client(Arc::new(http_client));
        }
        #[cfg(target_family = "wasm")]
        {
            // 安全性说明：Web示例为单线程运行，客户端仅在主线程创建和使用
            let http_client = unsafe {
                gpui_web::FetchHttpClient::with_user_agent("gpui example")
                    .expect("创建FetchHttpClient失败")
            };
            cx.set_http_client(Arc::new(http_client));
        }

        cx.activate(true);
        cx.on_action(|_: &退出, cx| cx.quit());
        cx.bind_keys([KeyBinding::new("cmd-q", 退出, None)]);
        cx.set_menus(vec![Menu {
            name: "图片".into(),
            items: vec![MenuItem::action("退出", 退出)],
            disabled: false,
        }]);

        let window_options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(SharedString::from("图片示例")),
                appears_transparent: false,
                ..Default::default()
            }),

            window_bounds: Some(WindowBounds::Windowed(Bounds {
                size: size(px(1100.), px(600.)),
                origin: Point::new(px(200.), px(200.)),
            })),

            ..Default::default()
        };

        cx.open_window(window_options, |_, cx| {
            cx.new(|_| ImageShowcase {
                // 相对于项目根目录的路径
                local_resource: manifest_dir.join("examples/image/app-icon.png").into(),
                remote_resource: "https://picsum.photos/800/400".into(),
                asset_resource: "image/color.svg".into(),
            })
        })
        .unwrap();
    });
}

#[cfg(not(target_family = "wasm"))]
fn main() {
    env_logger::init();
    run_example();
}

#[cfg(target_family = "wasm")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() {
    gpui_platform::web_init();
    run_example();
}