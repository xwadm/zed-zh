use crate::dispatcher::WebDispatcher;
use crate::display::WebDisplay;
use crate::keyboard::WebKeyboardLayout;
use crate::window::WebWindow;
use anyhow::Result;
use futures::channel::oneshot;
use gpui::{
    Action, AnyWindowHandle, BackgroundExecutor, ClipboardItem, CursorStyle, DummyKeyboardMapper,
    ForegroundExecutor, Keymap, Menu, MenuItem, PathPromptOptions, Platform, PlatformDisplay,
    PlatformKeyboardLayout, PlatformKeyboardMapper, PlatformTextSystem, PlatformWindow, Task,
    ThermalState, WindowAppearance, WindowParams,
};
use gpui_wgpu::WgpuContext;
use std::{
    borrow::Cow,
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

// 内置字体资源：IBM Plex Sans + Lilex 字体文件二进制数据
static BUNDLED_FONTS: &[&[u8]] = &[
    include_bytes!("../../../assets/fonts/ibm-plex-sans/IBMPlexSans-Regular.ttf"),
    include_bytes!("../../../assets/fonts/ibm-plex-sans/IBMPlexSans-Italic.ttf"),
    include_bytes!("../../../assets/fonts/ibm-plex-sans/IBMPlexSans-SemiBold.ttf"),
    include_bytes!("../../../assets/fonts/ibm-plex-sans/IBMPlexSans-SemiBoldItalic.ttf"),
    include_bytes!("../../../assets/fonts/lilex/Lilex-Regular.ttf"),
    include_bytes!("../../../assets/fonts/lilex/Lilex-Bold.ttf"),
    include_bytes!("../../../assets/fonts/lilex/Lilex-Italic.ttf"),
    include_bytes!("../../../assets/fonts/lilex/Lilex-BoldItalic.ttf"),
];

/// Web 平台核心实现类，适配浏览器环境的 GPUI 平台接口
pub struct WebPlatform {
    browser_window: web_sys::Window,                // 浏览器窗口对象
    background_executor: BackgroundExecutor,       // 后台任务执行器
    foreground_executor: ForegroundExecutor,       // 前台任务执行器
    text_system: Arc<dyn PlatformTextSystem>,      // 文本渲染系统
    active_window: RefCell<Option<AnyWindowHandle>>, // 当前活跃窗口
    active_display: Rc<dyn PlatformDisplay>,       // 当前显示设备
    callbacks: RefCell<WebPlatformCallbacks>,      // 平台回调函数集合
    wgpu_context: Rc<RefCell<Option<WgpuContext>>>,// WebGPU 渲染上下文
}

/// Web 平台回调函数结构体，存储各类系统事件回调
#[derive(Default)]
struct WebPlatformCallbacks {
    open_urls: Option<Box<dyn FnMut(Vec<String>)>>,                // 打开链接回调
    quit: Option<Box<dyn FnMut()>>,                                // 退出应用回调
    reopen: Option<Box<dyn FnMut()>>,                              // 重新打开应用回调
    app_menu_action: Option<Box<dyn FnMut(&dyn Action)>>,          // 应用菜单操作回调
    will_open_app_menu: Option<Box<dyn FnMut()>>,                  // 即将打开菜单回调
    validate_app_menu_command: Option<Box<dyn FnMut(&dyn Action) -> bool>>, // 菜单命令校验回调
    keyboard_layout_change: Option<Box<dyn FnMut()>>,              // 键盘布局变更回调
    thermal_state_change: Option<Box<dyn FnMut()>>,                // 设备热状态变更回调
}

impl WebPlatform {
    /// 创建 Web 平台实例
    /// allow_multi_threading: 是否允许多线程
    pub fn new(allow_multi_threading: bool) -> Self {
        // 获取浏览器窗口对象（必须在浏览器环境中运行）
        let browser_window =
            web_sys::window().expect("必须在浏览器窗口环境中运行");
        // 创建 Web 事件分发器
        let dispatcher = Arc::new(WebDispatcher::new(
            browser_window.clone(),
            allow_multi_threading,
        ));
        // 初始化前后台任务执行器
        let background_executor = BackgroundExecutor::new(dispatcher.clone());
        let foreground_executor = ForegroundExecutor::new(dispatcher);
        // 初始化文本渲染系统（不加载系统字体，仅使用内置字体）
        let text_system = Arc::new(gpui_wgpu::CosmicTextSystem::new_without_system_fonts(
            "IBM Plex Sans",
        ));
        // 加载内置字体资源
        let fonts = BUNDLED_FONTS
            .iter()
            .map(|bytes| Cow::Borrowed(*bytes))
            .collect();
        if let Err(error) = text_system.add_fonts(fonts) {
            log::error!("加载内置字体失败: {error:#}");
        }
        let text_system: Arc<dyn PlatformTextSystem> = text_system;
        // 初始化显示设备
        let active_display: Rc<dyn PlatformDisplay> =
            Rc::new(WebDisplay::new(browser_window.clone()));

        Self {
            browser_window,
            background_executor,
            foreground_executor,
            text_system,
            active_window: RefCell::new(None),
            active_display,
            callbacks: RefCell::new(WebPlatformCallbacks::default()),
            wgpu_context: Rc::new(RefCell::new(None)),
        }
    }
}

/// 实现 GPUI Platform 平台接口
impl Platform for WebPlatform {
    /// 获取后台任务执行器
    fn background_executor(&self) -> BackgroundExecutor {
        self.background_executor.clone()
    }

    /// 获取前台任务执行器
    fn foreground_executor(&self) -> ForegroundExecutor {
        self.foreground_executor.clone()
    }

    /// 获取文本渲染系统
    fn text_system(&self) -> Arc<dyn PlatformTextSystem> {
        self.text_system.clone()
    }

    /// 运行平台初始化逻辑
    fn run(&self, on_finish_launching: Box<dyn 'static + FnOnce()>) {
        let wgpu_context = self.wgpu_context.clone();
        // 异步初始化 WebGPU 上下文
        wasm_bindgen_futures::spawn_local(async move {
            match WgpuContext::new_web().await {
                Ok(context) => {
                    log::info!("WebGPU 上下文初始化成功");
                    *wgpu_context.borrow_mut() = Some(context);
                    on_finish_launching();
                }
                Err(err) => {
                    log::error!("WebGPU 上下文初始化失败: {err:#}");
                    on_finish_launching();
                }
            }
        });
    }

    /// 退出应用（浏览器环境不支持）
    fn quit(&self) {
        log::warn!("已调用 WebPlatform::quit 方法，但浏览器环境不支持退出应用。");
    }

    /// 重启应用（浏览器环境不支持）
    fn restart(&self, _binary_path: Option<PathBuf>) {}

    /// 激活应用
    fn activate(&self, _ignoring_other_apps: bool) {}

    /// 隐藏应用
    fn hide(&self) {}

    /// 隐藏其他应用
    fn hide_other_apps(&self) {}

    /// 显示其他应用
    fn unhide_other_apps(&self) {}

    /// 获取所有显示设备（Web 环境仅一个设备）
    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        vec![self.active_display.clone()]
    }

    /// 获取主显示设备
    fn primary_display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        Some(self.active_display.clone())
    }

    /// 获取当前活跃窗口
    fn active_window(&self) -> Option<AnyWindowHandle> {
        *self.active_window.borrow()
    }

    /// 创建新窗口
    fn open_window(
        &self,
        handle: AnyWindowHandle,
        params: WindowParams,
    ) -> anyhow::Result<Box<dyn PlatformWindow>> {
        // 检查 WebGPU 上下文是否初始化完成
        let context_ref = self.wgpu_context.borrow();
        let context = context_ref.as_ref().ok_or_else(|| {
            anyhow::anyhow!("WebGPU 上下文未初始化，是否调用了 Platform::run() 方法？")
        })?;

        // 创建 Web 窗口实例
        let window = WebWindow::new(handle, params, context, self.browser_window.clone())?;
        *self.active_window.borrow_mut() = Some(handle);
        Ok(Box::new(window))
    }

    /// 获取窗口外观（亮色/暗色模式）
    fn window_appearance(&self) -> WindowAppearance {
        let Ok(Some(media_query)) = self
            .browser_window
            .match_media("(prefers-color-scheme: dark)")
        else {
            return WindowAppearance::Light;
        };
        if media_query.matches() {
            WindowAppearance::Dark
        } else {
            WindowAppearance::Light
        }
    }

    /// 打开链接
    fn open_url(&self, url: &str) {
        if let Err(error) = self.browser_window.open_with_url(url) {
            log::warn!("打开链接 '{url}' 失败: {error:?}");
        }
    }

    /// 注册打开链接回调
    fn on_open_urls(&self, callback: Box<dyn FnMut(Vec<String>)>) {
        self.callbacks.borrow_mut().open_urls = Some(callback);
    }

    /// 注册链接协议（Web 环境不支持）
    fn register_url_scheme(&self, _url: &str) -> Task<Result<()>> {
        Task::ready(Ok(()))
    }

    /// 弹出文件选择框（Web 环境不支持）
    fn prompt_for_paths(
        &self,
        _options: PathPromptOptions,
    ) -> oneshot::Receiver<Result<Option<Vec<PathBuf>>>> {
        let (tx, rx) = oneshot::channel();
        tx.send(Err(anyhow::anyhow!(
            "Web 环境不支持文件选择对话框"
        )))
        .ok();
        rx
    }

    /// 弹出新建文件对话框（Web 环境不支持）
    fn prompt_for_new_path(
        &self,
        _directory: &Path,
        _suggested_name: Option<&str>,
    ) -> oneshot::Receiver<Result<Option<PathBuf>>> {
        let (sender, receiver) = oneshot::channel();
        sender
            .send(Err(anyhow::anyhow!(
                "Web 环境不支持新建文件对话框"
            )))
            .ok();
        receiver
    }

    /// 是否支持同时选择文件和目录（Web 环境不支持）
    fn can_select_mixed_files_and_dirs(&self) -> bool {
        false
    }

    /// 在文件管理器中显示路径（Web 环境不支持）
    fn reveal_path(&self, _path: &Path) {}

    /// 使用系统默认程序打开文件（Web 环境不支持）
    fn open_with_system(&self, _path: &Path) {}

    /// 注册退出回调
    fn on_quit(&self, callback: Box<dyn FnMut()>) {
        self.callbacks.borrow_mut().quit = Some(callback);
    }

    /// 注册重新打开应用回调
    fn on_reopen(&self, callback: Box<dyn FnMut()>) {
        self.callbacks.borrow_mut().reopen = Some(callback);
    }

    /// 设置应用菜单（Web 环境不支持）
    fn set_menus(&self, _menus: Vec<Menu>, _keymap: &Keymap) {}

    /// 设置 Dock 菜单（Web 环境不支持）
    fn set_dock_menu(&self, _menu: Vec<MenuItem>, _keymap: &Keymap) {}

    /// 注册菜单操作回调
    fn on_app_menu_action(&self, callback: Box<dyn FnMut(&dyn Action)>) {
        self.callbacks.borrow_mut().app_menu_action = Some(callback);
    }

    /// 注册菜单即将打开回调
    fn on_will_open_app_menu(&self, callback: Box<dyn FnMut()>) {
        self.callbacks.borrow_mut().will_open_app_menu = Some(callback);
    }

    /// 注册菜单命令校验回调
    fn on_validate_app_menu_command(&self, callback: Box<dyn FnMut(&dyn Action) -> bool>) {
        self.callbacks.borrow_mut().validate_app_menu_command = Some(callback);
    }

    /// 获取设备热状态
    fn thermal_state(&self) -> ThermalState {
        ThermalState::Nominal
    }

    /// 注册热状态变更回调
    fn on_thermal_state_change(&self, callback: Box<dyn FnMut()>) {
        self.callbacks.borrow_mut().thermal_state_change = Some(callback);
    }

    /// 渲染器名称
    fn compositor_name(&self) -> &'static str {
        "Web"
    }

    /// 获取应用路径（Web 环境不支持）
    fn app_path(&self) -> Result<PathBuf> {
        Err(anyhow::anyhow!("Web 环境无法获取应用路径"))
    }

    /// 获取辅助可执行文件路径（Web 环境不支持）
    fn path_for_auxiliary_executable(&self, _name: &str) -> Result<PathBuf> {
        Err(anyhow::anyhow!(
            "Web 环境无法获取辅助可执行文件路径"
        ))
    }

    /// 设置鼠标光标样式
    fn set_cursor_style(&self, style: CursorStyle) {
        // 映射 GPUI 光标样式为 CSS 光标样式
        let css_cursor = match style {
            CursorStyle::Arrow => "default",
            CursorStyle::IBeam => "text",
            CursorStyle::Crosshair => "crosshair",
            CursorStyle::ClosedHand => "grabbing",
            CursorStyle::OpenHand => "grab",
            CursorStyle::PointingHand => "pointer",
            CursorStyle::ResizeLeft | CursorStyle::ResizeRight | CursorStyle::ResizeLeftRight => {
                "ew-resize"
            }
            CursorStyle::ResizeUp | CursorStyle::ResizeDown | CursorStyle::ResizeUpDown => {
                "ns-resize"
            }
            CursorStyle::ResizeUpLeftDownRight => "nesw-resize",
            CursorStyle::ResizeUpRightDownLeft => "nwse-resize",
            CursorStyle::ResizeColumn => "col-resize",
            CursorStyle::ResizeRow => "row-resize",
            CursorStyle::IBeamCursorForVerticalLayout => "vertical-text",
            CursorStyle::OperationNotAllowed => "not-allowed",
            CursorStyle::DragLink => "alias",
            CursorStyle::DragCopy => "copy",
            CursorStyle::ContextualMenu => "context-menu",
            CursorStyle::None => "none",
        };

        // 设置页面光标样式
        if let Some(document) = self.browser_window.document() {
            if let Some(body) = document.body() {
                if let Err(error) = body.style().set_property("cursor", css_cursor) {
                    log::warn!("设置光标样式失败: {error:?}");
                }
            }
        }
    }

    /// 是否自动隐藏滚动条
    fn should_auto_hide_scrollbars(&self) -> bool {
        true
    }

    /// 读取剪贴板（Web 环境不支持）
    fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        None
    }

    /// 写入剪贴板（Web 环境不支持）
    fn write_to_clipboard(&self, _item: ClipboardItem) {}

    /// 保存凭据（Web 环境不支持）
    fn write_credentials(&self, _url: &str, _username: &str, _password: &[u8]) -> Task<Result<()>> {
        Task::ready(Err(anyhow::anyhow!(
            "Web 环境不支持凭据存储"
        )))
    }

    /// 读取凭据（Web 环境不支持）
    fn read_credentials(&self, _url: &str) -> Task<Result<Option<(String, Vec<u8>)>>> {
        Task::ready(Ok(None))
    }

    /// 删除凭据（Web 环境不支持）
    fn delete_credentials(&self, _url: &str) -> Task<Result<()>> {
        Task::ready(Err(anyhow::anyhow!(
            "Web 环境不支持凭据存储"
        )))
    }

    /// 获取键盘布局
    fn keyboard_layout(&self) -> Box<dyn PlatformKeyboardLayout> {
        Box::new(WebKeyboardLayout)
    }

    /// 获取键盘映射器
    fn keyboard_mapper(&self) -> Rc<dyn PlatformKeyboardMapper> {
        Rc::new(DummyKeyboardMapper)
    }

    /// 注册键盘布局变更回调
    fn on_keyboard_layout_change(&self, callback: Box<dyn FnMut()>) {
        self.callbacks.borrow_mut().keyboard_layout_change = Some(callback);
    }
}