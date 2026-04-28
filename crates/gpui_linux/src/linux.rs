// 声明模块
mod dispatcher;
mod headless;
mod keyboard;
mod platform;
// 启用 wayland 或 x11 特性时，加载文本系统模块
#[cfg(any(feature = "wayland", feature = "x11"))]
mod text_system;
// 启用 wayland 特性时，加载 wayland 模块
#[cfg(feature = "wayland")]
mod wayland;
// 启用 x11 特性时，加载 x11 模块
#[cfg(feature = "x11")]
mod x11;

// 启用 wayland 或 x11 特性时，加载 xdg 桌面门户模块
#[cfg(any(feature = "wayland", feature = "x11"))]
mod xdg_desktop_portal;

// 导出模块成员
pub use dispatcher::*;
pub(crate) use headless::*;
pub(crate) use keyboard::*;
pub(crate) use platform::*;
// 启用 wayland 或 x11 特性时，导出文本系统成员
#[cfg(any(feature = "wayland", feature = "x11"))]
pub(crate) use text_system::*;
// 启用 wayland 特性时，导出 wayland 成员
#[cfg(feature = "wayland")]
pub(crate) use wayland::*;
// 启用 x11 特性时，导出 x11 成员
#[cfg(feature = "x11")]
pub(crate) use x11::*;

use std::rc::Rc;

/// 获取当前操作系统的默认平台实现
/// headless: 是否为无窗口模式
pub fn current_platform(headless: bool) -> Rc<dyn gpui::Platform> {
    #[cfg(feature = "x11")]
    use anyhow::Context as _;

    // 无窗口模式，直接返回无头客户端
    if headless {
        return Rc::new(LinuxPlatform {
            inner: HeadlessClient::new(),
        });
    }

    // 自动检测当前桌面合成器
    match gpui::guess_compositor() {
        // Wayland 桌面环境
        #[cfg(feature = "wayland")]
        "Wayland" => Rc::new(LinuxPlatform {
            inner: WaylandClient::new(),
        }),

        // X11 桌面环境
        #[cfg(feature = "x11")]
        "X11" => Rc::new(LinuxPlatform {
            inner: X11Client::new()
                .context("X11 客户端初始化失败")
                .unwrap(),
        }),

        // 无头模式
        "Headless" => Rc::new(LinuxPlatform {
            inner: HeadlessClient::new(),
        }),
        // 未知合成器，理论上不会执行到这里
        _ => unreachable!(),
    }
}