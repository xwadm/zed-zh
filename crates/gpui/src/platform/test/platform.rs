use crate::{
    AnyWindowHandle, BackgroundExecutor, ClipboardItem, CursorStyle, DevicePixels,
    DummyKeyboardMapper, ForegroundExecutor, Keymap, NoopTextSystem, Platform, PlatformDisplay,
    PlatformHeadlessRenderer, PlatformKeyboardLayout, PlatformKeyboardMapper, PlatformTextSystem,
    PromptButton, ScreenCaptureFrame, ScreenCaptureSource, ScreenCaptureStream, SourceMetadata,
    Task, TestDisplay, TestWindow, ThermalState, WindowAppearance, WindowParams, size,
};
use anyhow::Result;
use collections::VecDeque;
use futures::channel::oneshot;
use parking_lot::Mutex;
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::{Rc, Weak},
    sync::Arc,
};

/// 测试平台，实现 Platform 接口，用于单元测试
pub(crate) struct TestPlatform {
    background_executor: BackgroundExecutor,
    foreground_executor: ForegroundExecutor,

    pub(crate) active_window: RefCell<Option<TestWindow>>,
    active_display: Rc<dyn PlatformDisplay>,
    active_cursor: Mutex<CursorStyle>,
    current_clipboard_item: Mutex<Option<ClipboardItem>>,
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    current_primary_item: Mutex<Option<ClipboardItem>>,
    #[cfg(target_os = "macos")]
    current_find_pasteboard_item: Mutex<Option<ClipboardItem>>,
    pub(crate) prompts: RefCell<TestPrompts>,
    screen_capture_sources: RefCell<Vec<TestScreenCaptureSource>>,
    pub opened_url: RefCell<Option<String>>,
    pub text_system: Arc<dyn PlatformTextSystem>,
    pub expect_restart: RefCell<Option<oneshot::Sender<Option<PathBuf>>>>,
    headless_renderer_factory: Option<Box<dyn Fn() -> Option<Box<dyn PlatformHeadlessRenderer>>>>,
    weak: Weak<Self>,
}

#[derive(Clone)]
/// 模拟屏幕采集源，用于测试
pub struct TestScreenCaptureSource {}

/// 模拟屏幕采集流，用于测试
pub struct TestScreenCaptureStream {}

impl ScreenCaptureSource for TestScreenCaptureSource {
    fn metadata(&self) -> Result<SourceMetadata> {
        Ok(SourceMetadata {
            id: 0,
            is_main: None,
            label: None,
            resolution: size(DevicePixels(1), DevicePixels(1)),
        })
    }

    fn stream(
        &self,
        _foreground_executor: &ForegroundExecutor,
        _frame_callback: Box<dyn Fn(ScreenCaptureFrame) + Send>,
    ) -> oneshot::Receiver<Result<Box<dyn ScreenCaptureStream>>> {
        let (mut tx, rx) = oneshot::channel();
        let stream = TestScreenCaptureStream {};
        tx.send(Ok(Box::new(stream) as Box<dyn ScreenCaptureStream>))
            .ok();
        rx
    }
}

impl ScreenCaptureStream for TestScreenCaptureStream {
    fn metadata(&self) -> Result<SourceMetadata> {
        TestScreenCaptureSource {}.metadata()
    }
}

/// 测试用弹窗对象
struct TestPrompt {
    msg: String,
    detail: Option<String>,
    answers: Vec<String>,
    tx: oneshot::Sender<usize>,
}

#[derive(Default)]
pub(crate) struct TestPrompts {
    multiple_choice: VecDeque<TestPrompt>,
    new_path: VecDeque<(PathBuf, oneshot::Sender<Result<Option<PathBuf>>>)>,
}

impl TestPlatform {
    /// 创建测试平台实例
    pub fn new(executor: BackgroundExecutor, foreground_executor: ForegroundExecutor) -> Rc<Self> {
        Self::with_platform(
            executor,
            foreground_executor,
            Arc::new(NoopTextSystem),
            None,
        )
    }

    /// 使用自定义文本系统创建测试平台
    pub fn with_text_system(
        executor: BackgroundExecutor,
        foreground_executor: ForegroundExecutor,
        text_system: Arc<dyn PlatformTextSystem>,
    ) -> Rc<Self> {
        Self::with_platform(executor, foreground_executor, text_system, None)
    }

    /// 使用自定义平台配置创建测试平台
    pub fn with_platform(
        executor: BackgroundExecutor,
        foreground_executor: ForegroundExecutor,
        text_system: Arc<dyn PlatformTextSystem>,
        headless_renderer_factory: Option<
            Box<dyn Fn() -> Option<Box<dyn PlatformHeadlessRenderer>>>,
        >,
    ) -> Rc<Self> {
        Rc::new_cyclic(|weak| TestPlatform {
            background_executor: executor,
            foreground_executor,
            prompts: Default::default(),
            screen_capture_sources: Default::default(),
            active_cursor: Default::default(),
            active_display: Rc::new(TestDisplay::new()),
            active_window: Default::default(),
            expect_restart: Default::default(),
            current_clipboard_item: Mutex::new(None),
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            current_primary_item: Mutex::new(None),
            #[cfg(target_os = "macos")]
            current_find_pasteboard_item: Mutex::new(None),
            weak: weak.clone(),
            opened_url: Default::default(),
            text_system,
            headless_renderer_factory,
        })
    }

    /// 模拟选择新文件路径
    pub(crate) fn simulate_new_path_selection(
        &self,
        select_path: impl FnOnce(&std::path::Path) -> Option<std::path::PathBuf>,
    ) {
        let (path, tx) = self
            .prompts
            .borrow_mut()
            .new_path
            .pop_front()
            .expect("没有待处理的新建路径弹窗");
        tx.send(Ok(select_path(&path))).ok();
    }

    /// 模拟弹窗回复
    #[track_caller]
    pub(crate) fn simulate_prompt_answer(&self, response: &str) {
        let prompt = self
            .prompts
            .borrow_mut()
            .multiple_choice
            .pop_front()
            .expect("没有待处理的选择弹窗");
        let Some(ix) = prompt.answers.iter().position(|a| a == response) else {
            panic!(
                "弹窗：{}\n{:?}\n{:?}\n无法使用该选项回复：{}",
                prompt.msg, prompt.detail, prompt.answers, response
            )
        };
        prompt.tx.send(ix).ok();
    }

    /// 是否存在待处理的弹窗
    pub(crate) fn has_pending_prompt(&self) -> bool {
        !self.prompts.borrow().multiple_choice.is_empty()
    }

    /// 获取当前待处理的弹窗信息
    pub(crate) fn pending_prompt(&self) -> Option<(String, String)> {
        let prompts = self.prompts.borrow();
        let prompt = prompts.multiple_choice.front()?;
        Some((
            prompt.msg.clone(),
            prompt.detail.clone().unwrap_or_default(),
        ))
    }

    /// 设置模拟的屏幕采集源
    pub(crate) fn set_screen_capture_sources(&self, sources: Vec<TestScreenCaptureSource>) {
        *self.screen_capture_sources.borrow_mut() = sources;
    }

    /// 触发选择弹窗
    pub(crate) fn prompt(
        &self,
        msg: &str,
        detail: Option<&str>,
        answers: &[PromptButton],
    ) -> oneshot::Receiver<usize> {
        let (tx, rx) = oneshot::channel();
        let answers: Vec<String> = answers.iter().map(|s| s.label().to_string()).collect();
        self.prompts
            .borrow_mut()
            .multiple_choice
            .push_back(TestPrompt {
                msg: msg.to_string(),
                detail: detail.map(|s| s.to_string()),
                answers,
                tx,
            });
        rx
    }

    /// 设置当前活跃窗口
    pub(crate) fn set_active_window(&self, window: Option<TestWindow>) {
        let executor = self.foreground_executor();
        let previous_window = self.active_window.borrow_mut().take();
        self.active_window.borrow_mut().clone_from(&window);

        executor
            .spawn(async move {
                if let Some(previous_window) = previous_window {
                    if let Some(window) = window.as_ref()
                        && Rc::ptr_eq(&previous_window.0, &window.0)
                    {
                        return;
                    }
                    previous_window.simulate_active_status_change(false);
                }
                if let Some(window) = window {
                    window.simulate_active_status_change(true);
                }
            })
            .detach();
    }

    /// 是否触发过新建路径弹窗
    pub(crate) fn did_prompt_for_new_path(&self) -> bool {
        !self.prompts.borrow().new_path.is_empty()
    }
}

impl Platform for TestPlatform {
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

    /// 获取键盘布局
    fn keyboard_layout(&self) -> Box<dyn PlatformKeyboardLayout> {
        Box::new(TestKeyboardLayout)
    }

    /// 获取键盘映射器
    fn keyboard_mapper(&self) -> Rc<dyn PlatformKeyboardMapper> {
        Rc::new(DummyKeyboardMapper)
    }

    /// 注册键盘布局变更回调（测试用空实现）
    fn on_keyboard_layout_change(&self, _: Box<dyn FnMut()>) {}

    /// 注册设备热状态变更回调（测试用空实现）
    fn on_thermal_state_change(&self, _: Box<dyn FnMut()>) {}

    /// 获取设备热状态
    fn thermal_state(&self) -> ThermalState {
        ThermalState::Nominal
    }

    /// 运行平台（测试未实现）
    fn run(&self, _on_finish_launching: Box<dyn FnOnce()>) {
        unimplemented!()
    }

    /// 退出应用（测试空实现）
    fn quit(&self) {}

    /// 重启应用
    fn restart(&self, path: Option<PathBuf>) {
        if let Some(tx) = self.expect_restart.take() {
            tx.send(path).unwrap();
        }
    }

    /// 激活应用
    fn activate(&self, _ignoring_other_apps: bool) {
        //
    }

    /// 隐藏应用（测试未实现）
    fn hide(&self) {
        unimplemented!()
    }

    /// 隐藏其他应用（测试未实现）
    fn hide_other_apps(&self) {
        unimplemented!()
    }

    /// 显示其他应用（测试未实现）
    fn unhide_other_apps(&self) {
        unimplemented!()
    }

    /// 获取所有显示设备
    fn displays(&self) -> Vec<std::rc::Rc<dyn crate::PlatformDisplay>> {
        vec![self.active_display.clone()]
    }

    /// 获取主显示设备
    fn primary_display(&self) -> Option<std::rc::Rc<dyn crate::PlatformDisplay>> {
        Some(self.active_display.clone())
    }

    /// 是否支持屏幕采集
    fn is_screen_capture_supported(&self) -> bool {
        true
    }

    /// 获取屏幕采集源
    fn screen_capture_sources(
        &self,
    ) -> oneshot::Receiver<Result<Vec<Rc<dyn ScreenCaptureSource>>>> {
        let (mut tx, rx) = oneshot::channel();
        tx.send(Ok(self
            .screen_capture_sources
            .borrow()
            .iter()
            .map(|source| Rc::new(source.clone()) as Rc<dyn ScreenCaptureSource>)
            .collect()))
            .ok();
        rx
    }

    /// 获取当前活跃窗口
    fn active_window(&self) -> Option<crate::AnyWindowHandle> {
        self.active_window
            .borrow()
            .as_ref()
            .map(|window| window.0.lock().handle)
    }

    /// 创建测试窗口
    fn open_window(
        &self,
        handle: AnyWindowHandle,
        params: WindowParams,
    ) -> anyhow::Result<Box<dyn crate::PlatformWindow>> {
        let renderer = self.headless_renderer_factory.as_ref().and_then(|f| f());
        let window = TestWindow::new(
            handle,
            params,
            self.weak.clone(),
            self.active_display.clone(),
            renderer,
        );
        Ok(Box::new(window))
    }

    /// 获取窗口外观（固定为浅色模式）
    fn window_appearance(&self) -> WindowAppearance {
        WindowAppearance::Light
    }

    /// 打开链接（记录到测试平台）
    fn open_url(&self, url: &str) {
        *self.opened_url.borrow_mut() = Some(url.to_string())
    }

    /// 注册打开链接回调（测试未实现）
    fn on_open_urls(&self, _callback: Box<dyn FnMut(Vec<String>)>) {
        unimplemented!()
    }

    /// 弹出文件选择框（测试未实现）
    fn prompt_for_paths(
        &self,
        _options: crate::PathPromptOptions,
    ) -> oneshot::Receiver<Result<Option<Vec<std::path::PathBuf>>>> {
        unimplemented!()
    }

    /// 弹出新建文件对话框
    fn prompt_for_new_path(
        &self,
        directory: &std::path::Path,
        _suggested_name: Option<&str>,
    ) -> oneshot::Receiver<Result<Option<std::path::PathBuf>>> {
        let (tx, rx) = oneshot::channel();
        self.prompts
            .borrow_mut()
            .new_path
            .push_back((directory.to_path_buf(), tx));
        rx
    }

    /// 是否支持同时选择文件和目录
    fn can_select_mixed_files_and_dirs(&self) -> bool {
        true
    }

    /// 在文件管理器中显示路径（测试未实现）
    fn reveal_path(&self, _path: &std::path::Path) {
        unimplemented!()
    }

    /// 注册退出回调
    fn on_quit(&self, _callback: Box<dyn FnMut()>) {}

    /// 注册重新打开应用回调（测试未实现）
    fn on_reopen(&self, _callback: Box<dyn FnMut()>) {
        unimplemented!()
    }

    /// 设置应用菜单
    fn set_menus(&self, _menus: Vec<crate::Menu>, _keymap: &Keymap) {}
    /// 设置 Dock 菜单
    fn set_dock_menu(&self, _menu: Vec<crate::MenuItem>, _keymap: &Keymap) {}

    /// 添加最近使用文档
    fn add_recent_document(&self, _paths: &Path) {}

    /// 注册菜单操作回调
    fn on_app_menu_action(&self, _callback: Box<dyn FnMut(&dyn crate::Action)>) {}

    /// 注册菜单即将打开回调
    fn on_will_open_app_menu(&self, _callback: Box<dyn FnMut()>) {}

    /// 注册菜单命令校验回调
    fn on_validate_app_menu_command(&self, _callback: Box<dyn FnMut(&dyn crate::Action) -> bool>) {}

    /// 获取应用路径（测试未实现）
    fn app_path(&self) -> Result<std::path::PathBuf> {
        unimplemented!()
    }

    /// 获取辅助可执行文件路径（测试未实现）
    fn path_for_auxiliary_executable(&self, _name: &str) -> Result<std::path::PathBuf> {
        unimplemented!()
    }

    /// 设置鼠标光标样式
    fn set_cursor_style(&self, style: crate::CursorStyle) {
        *self.active_cursor.lock() = style;
    }

    /// 是否自动隐藏滚动条
    fn should_auto_hide_scrollbars(&self) -> bool {
        false
    }

    /// 读取剪贴板
    fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        self.current_clipboard_item.lock().clone()
    }

    /// 写入剪贴板
    fn write_to_clipboard(&self, item: ClipboardItem) {
        *self.current_clipboard_item.lock() = Some(item);
    }

    /// Linux/FreeBSD 专用：读取主剪贴板
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn read_from_primary(&self) -> Option<ClipboardItem> {
        self.current_primary_item.lock().clone()
    }

    /// Linux/FreeBSD 专用：写入主剪贴板
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    fn write_to_primary(&self, item: ClipboardItem) {
        *self.current_primary_item.lock() = Some(item);
    }

    /// macOS 专用：读取查找剪贴板
    #[cfg(target_os = "macos")]
    fn read_from_find_pasteboard(&self) -> Option<ClipboardItem> {
        self.current_find_pasteboard_item.lock().clone()
    }

    /// macOS 专用：写入查找剪贴板
    #[cfg(target_os = "macos")]
    fn write_to_find_pasteboard(&self, item: ClipboardItem) {
        *self.current_find_pasteboard_item.lock() = Some(item);
    }

    /// 保存凭据
    fn write_credentials(&self, _url: &str, _username: &str, _password: &[u8]) -> Task<Result<()>> {
        Task::ready(Ok(()))
    }

    /// 读取凭据
    fn read_credentials(&self, _url: &str) -> Task<Result<Option<(String, Vec<u8>)>>> {
        Task::ready(Ok(None))
    }

    /// 删除凭据
    fn delete_credentials(&self, _url: &str) -> Task<Result<()>> {
        Task::ready(Ok(()))
    }

    /// 注册 URL 协议（测试未实现）
    fn register_url_scheme(&self, _: &str) -> Task<anyhow::Result<()>> {
        unimplemented!()
    }

    /// 使用系统默认程序打开文件（测试未实现）
    fn open_with_system(&self, _path: &Path) {
        unimplemented!()
    }
}

impl TestScreenCaptureSource {
    /// 创建模拟屏幕采集源（用于测试）
    pub fn new() -> Self {
        Self {}
    }
}

/// 测试用键盘布局
struct TestKeyboardLayout;

impl PlatformKeyboardLayout for TestKeyboardLayout {
    fn id(&self) -> &str {
        "zed.keyboard.example"
    }

    fn name(&self) -> &str {
        "zed.keyboard.example"
    }
}