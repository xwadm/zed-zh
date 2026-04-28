use collab_ui::collab_panel;
use gpui::{App, Menu, MenuItem, OsAction};
use release_channel::ReleaseChannel;
use terminal_view::terminal_panel;
use zed_actions::{debug_panel, dev};

/// 定义 Zed 编辑器的全局应用菜单
pub fn app_menus(cx: &mut App) -> Vec<Menu> {
    use zed_actions::Quit;

    // 构建【视图】子菜单项
    let mut view_items = vec![
        MenuItem::action("放大字体", zed_actions::IncreaseBufferFontSize { persist: false }),
        MenuItem::action("缩小字体", zed_actions::DecreaseBufferFontSize { persist: false }),
        MenuItem::action("重置字体缩放", zed_actions::ResetBufferFontSize { persist: false }),
        MenuItem::action("重置所有缩放", zed_actions::ResetAllZoom { persist: false }),
        MenuItem::separator(),
        MenuItem::action("切换左侧面板", workspace::ToggleLeftDock),
        MenuItem::action("切换右侧面板", workspace::ToggleRightDock),
        MenuItem::action("切换底部面板", workspace::ToggleBottomDock),
        MenuItem::action("切换所有面板", workspace::ToggleAllDocks),
        // 编辑器布局子菜单
        MenuItem::submenu(Menu {
            name: "编辑器布局".into(),
            disabled: false,
            items: vec![
                MenuItem::action("向上拆分窗格", workspace::SplitUp::default()),
                MenuItem::action("向下拆分窗格", workspace::SplitDown::default()),
                MenuItem::action("向左拆分窗格", workspace::SplitLeft::default()),
                MenuItem::action("向右拆分窗格", workspace::SplitRight::default()),
            ],
        }),
        MenuItem::separator(),
        MenuItem::action("项目面板", zed_actions::project_panel::ToggleFocus),
        MenuItem::action("大纲面板", outline_panel::ToggleFocus),
        MenuItem::action("协作面板", collab_panel::ToggleFocus),
        MenuItem::action("终端面板", terminal_panel::ToggleFocus),
        MenuItem::action("调试面板", debug_panel::ToggleFocus),
        MenuItem::separator(),
        MenuItem::action("诊断信息", diagnostics::Deploy),
        MenuItem::separator(),
    ];

    // 开发版专属：添加 GPUI 检查器
    if ReleaseChannel::try_global(cx) == Some(ReleaseChannel::Dev) {
        view_items.push(MenuItem::action("切换 GPUI 检查器", dev::ToggleInspector));
        view_items.push(MenuItem::separator());
    }

    // 定义全部顶级菜单
    vec![
        // Zed 主菜单
        Menu {
            name: "Zed".into(),
            disabled: false,
            items: vec![
                MenuItem::action("关于 Zed", zed_actions::About),
                MenuItem::action("检查更新…", auto_update::Check),
                MenuItem::separator(),
                // 设置子菜单
                MenuItem::submenu(Menu::new("设置").items([
                    MenuItem::action("打开设置", zed_actions::OpenSettings),
                    MenuItem::action("打开设置文件", super::OpenSettingsFile),
                    MenuItem::action("打开项目设置", zed_actions::OpenProjectSettings),
                    MenuItem::action("打开项目设置文件", super::OpenProjectSettingsFile),
                    MenuItem::action("打开默认设置", super::OpenDefaultSettings),
                    MenuItem::separator(),
                    MenuItem::action("打开快捷键设置", zed_actions::OpenKeymap),
                    MenuItem::action("打开快捷键文件", zed_actions::OpenKeymapFile),
                    MenuItem::action("打开默认快捷键文件", zed_actions::OpenDefaultKeymap),
                    MenuItem::separator(),
                    MenuItem::action("选择主题…", zed_actions::theme_selector::Toggle::default()),
                    MenuItem::action("选择图标主题…", zed_actions::icon_theme_selector::Toggle::default()),
                ])),
                MenuItem::separator(),
                #[cfg(target_os = "macos")]
                MenuItem::os_submenu("服务", gpui::SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("扩展…", zed_actions::Extensions::default()),
                #[cfg(not(target_os = "windows"))]
                MenuItem::action("安装命令行工具", install_cli::InstallCliBinary),
                MenuItem::separator(),
                #[cfg(target_os = "macos")]
                MenuItem::action("隐藏 Zed", super::Hide),
                #[cfg(target_os = "macos")]
                MenuItem::action("隐藏其他", super::HideOthers),
                #[cfg(target_os = "macos")]
                MenuItem::action("显示全部", super::ShowAll),
                MenuItem::separator(),
                MenuItem::action("退出 Zed", Quit),
            ],
        },
        // 文件菜单
        Menu {
            name: "文件".into(),
            disabled: false,
            items: vec![
                MenuItem::action("新建文件", workspace::NewFile),
                MenuItem::action("新建窗口", workspace::NewWindow),
                MenuItem::separator(),
                #[cfg(not(target_os = "macos"))]
                MenuItem::action("打开文件…", workspace::OpenFiles),
                MenuItem::action(
                    if cfg!(not(target_os = "macos")) { "打开文件夹…" } else { "打开…" },
                    workspace::Open::default(),
                ),
                MenuItem::action("打开最近项目…", zed_actions::OpenRecent { create_new_window: false }),
                MenuItem::action("打开远程项目…", zed_actions::OpenRemote {
                    create_new_window: false,
                    from_existing_connection: false,
                }),
                MenuItem::separator(),
                MenuItem::action("将文件夹添加到项目…", workspace::AddFolderToProject),
                MenuItem::separator(),
                MenuItem::action("保存", workspace::Save { save_intent: None }),
                MenuItem::action("另存为…", workspace::SaveAs),
                MenuItem::action("保存所有文件", workspace::SaveAll { save_intent: None }),
                MenuItem::separator(),
                MenuItem::action("关闭编辑器", workspace::CloseActiveItem {
                    save_intent: None,
                    close_pinned: true,
                }),
                MenuItem::action("关闭项目", workspace::CloseProject),
                MenuItem::action("关闭窗口", workspace::CloseWindow),
            ],
        },
        // 编辑菜单
        Menu {
            name: "编辑".into(),
            disabled: false,
            items: vec![
                MenuItem::os_action("撤销", editor::actions::Undo, OsAction::Undo),
                MenuItem::os_action("重做", editor::actions::Redo, OsAction::Redo),
                MenuItem::separator(),
                MenuItem::os_action("剪切", editor::actions::Cut, OsAction::Cut),
                MenuItem::os_action("复制", editor::actions::Copy, OsAction::Copy),
                MenuItem::action("复制并裁剪（去除首尾空白）", editor::actions::CopyAndTrim),
                MenuItem::os_action("粘贴", editor::actions::Paste, OsAction::Paste),
                MenuItem::separator(),
                MenuItem::action("查找…", search::buffer_search::Deploy::find()),
                MenuItem::action("在项目中查找…", workspace::DeploySearch::default()),
                MenuItem::separator(),
                MenuItem::action("切换行注释", editor::actions::ToggleComments::default()),
            ],
        },
        // 选择菜单
        Menu {
            name: "选择".into(),
            disabled: false,
            items: vec![
                MenuItem::os_action("全选", editor::actions::SelectAll, OsAction::SelectAll),
                MenuItem::action("扩大语法选择范围", editor::actions::SelectLargerSyntaxNode),
                MenuItem::action("缩小语法选择范围", editor::actions::SelectSmallerSyntaxNode),
                MenuItem::action("选择下一个同级节点", editor::actions::SelectNextSyntaxNode),
                MenuItem::action("选择上一个同级节点", editor::actions::SelectPreviousSyntaxNode),
                MenuItem::separator(),
                MenuItem::action("在上方添加光标", editor::actions::AddSelectionAbove { skip_soft_wrap: true }),
                MenuItem::action("在下方添加光标", editor::actions::AddSelectionBelow { skip_soft_wrap: true }),
                MenuItem::action("选择下一个匹配项", editor::actions::SelectNext { replace_newest: false }),
                MenuItem::action("选择上一个匹配项", editor::actions::SelectPrevious { replace_newest: false }),
                MenuItem::action("选择所有匹配项", editor::actions::SelectAllMatches),
                MenuItem::separator(),
                MenuItem::action("向上移动行", editor::actions::MoveLineUp),
                MenuItem::action("向下移动行", editor::actions::MoveLineDown),
                MenuItem::action("向下复制行", editor::actions::DuplicateLineDown),
            ],
        },
        // 视图菜单
        Menu {
            name: "视图".into(),
            disabled: false,
            items: view_items,
        },
        // 跳转菜单
        Menu {
            name: "跳转".into(),
            disabled: false,
            items: vec![
                MenuItem::action("后退", workspace::GoBack),
                MenuItem::action("前进", workspace::GoForward),
                MenuItem::separator(),
                MenuItem::action("命令面板…", zed_actions::command_palette::Toggle),
                MenuItem::separator(),
                MenuItem::action("跳转到文件…", workspace::ToggleFileFinder::default()),
                MenuItem::action("跳转到编辑器符号…", zed_actions::outline::ToggleOutline),
                MenuItem::action("跳转到行/列…", editor::actions::ToggleGoToLine),
                MenuItem::separator(),
                MenuItem::action("跳转到定义", editor::actions::GoToDefinition),
                MenuItem::action("跳转到声明", editor::actions::GoToDeclaration),
                MenuItem::action("跳转到类型定义", editor::actions::GoToTypeDefinition),
                MenuItem::action("查找所有引用", editor::actions::FindAllReferences::default()),
                MenuItem::separator(),
                MenuItem::action("下一个问题", editor::actions::GoToDiagnostic::default()),
                MenuItem::action("上一个问题", editor::actions::GoToPreviousDiagnostic::default()),
            ],
        },
        // 运行/调试菜单
        Menu {
            name: "运行".into(),
            disabled: false,
            items: vec![
                MenuItem::action("运行任务…", zed_actions::Spawn::ViaModal { reveal_target: None }),
                MenuItem::action("启动调试", debugger_ui::Start),
                MenuItem::separator(),
                MenuItem::action("编辑 tasks.json…", crate::zed::OpenProjectTasks),
                MenuItem::action("编辑 debug.json…", zed_actions::OpenProjectDebugTasks),
                MenuItem::separator(),
                MenuItem::action("继续", debugger_ui::Continue),
                MenuItem::action("单步跳过", debugger_ui::StepOver),
                MenuItem::action("单步进入", debugger_ui::StepInto),
                MenuItem::action("单步跳出", debugger_ui::StepOut),
                MenuItem::separator(),
                MenuItem::action("切换断点", editor::actions::ToggleBreakpoint),
                MenuItem::action("编辑断点…", editor::actions::EditLogBreakpoint),
                MenuItem::action("清除所有断点", debugger_ui::ClearAllBreakpoints),
            ],
        },
        // 窗口菜单
        Menu {
            name: "窗口".into(),
            disabled: false,
            items: vec![
                MenuItem::action("最小化", super::Minimize),
                MenuItem::action("最大化", super::Zoom),
                MenuItem::separator(),
            ],
        },
        // 帮助菜单
        Menu {
            name: "帮助".into(),
            disabled: false,
            items: vec![
                MenuItem::action("查看更新日志", auto_update_ui::ViewReleaseNotesLocally),
                MenuItem::action("查看遥测数据", zed_actions::OpenTelemetryLog),
                MenuItem::action("查看开源许可证", zed_actions::OpenLicenses),
                MenuItem::action("显示欢迎页面", onboarding::ShowWelcome),
                MenuItem::separator(),
                MenuItem::action("提交错误报告…", zed_actions::feedback::FileBugReport),
                MenuItem::action("请求功能…", zed_actions::feedback::RequestFeature),
                MenuItem::action("发送邮件给我们…", zed_actions::feedback::EmailZed),
                MenuItem::separator(),
                MenuItem::action("文档", super::OpenBrowser { url: "https://zed.dev/docs".into() }),
                MenuItem::action("Zed 源代码仓库", feedback::OpenZedRepo),
                MenuItem::action("Zed 官方推特", super::OpenBrowser { url: "https://twitter.com/zeddotdev".into() }),
                MenuItem::action("加入我们", super::OpenBrowser { url: "https://zed.dev/jobs".into() }),
            ],
        },
    ]
}