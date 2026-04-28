use gpui::{Action, actions};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// 如果Zed二进制文件未使用此crate中的任何内容，它将被优化移除
// 导致动作无法初始化。因此我们提供一个空的初始化函数供main调用。
//
// 相关参考链接：
// https://github.com/rust-lang/rust/issues/47384
// https://github.com/mmastrac/rust-ctor/issues/280
pub fn init() {}

/// 在系统默认浏览器中打开URL
#[derive(Clone, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = zed)]
#[serde(deny_unknown_fields)]
pub struct OpenBrowser {
    pub url: String,
}

/// 在应用内打开zed://URL
#[derive(Clone, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = zed)]
#[serde(deny_unknown_fields)]
pub struct OpenZedUrl {
    pub url: String,
}

/// 打开快捷键映射，用于添加快捷键或修改现有快捷键
#[derive(PartialEq, Clone, Default, Action, JsonSchema, Serialize, Deserialize)]
#[action(namespace = zed, no_json, no_register)]
pub struct ChangeKeybinding {
    pub action: String,
}

actions!(
    zed,
    [
        /// 打开设置编辑器
        #[action(deprecated_aliases = ["zed_actions::OpenSettingsEditor"])]
        OpenSettings,
        /// 打开设置JSON文件
        #[action(deprecated_aliases = ["zed_actions::OpenSettings"])]
        OpenSettingsFile,
        /// 打开项目专属设置
        #[action(deprecated_aliases = ["zed_actions::OpenProjectSettings"])]
        OpenProjectSettings,
        /// 打开默认快捷键映射文件
        OpenDefaultKeymap,
        /// 打开用户快捷键映射文件
        #[action(deprecated_aliases = ["zed_actions::OpenKeymap"])]
        OpenKeymapFile,
        /// 打开快捷键映射编辑器
        #[action(deprecated_aliases = ["zed_actions::OpenKeymapEditor"])]
        OpenKeymap,
        /// 打开账户设置
        OpenAccountSettings,
        /// 打开服务器设置
        OpenServerSettings,
        /// 退出应用
        Quit,
        /// 显示Zed相关信息
        About,
        /// 打开文档网站
        OpenDocs,
        /// 查看开源许可证
        OpenLicenses,
        /// 打开遥测日志
        OpenTelemetryLog,
        /// 打开性能分析器
        OpenPerformanceProfiler,
        /// 打开引导界面
        OpenOnboarding,
        /// 显示自动更新通知用于测试
        ShowUpdateNotification,
    ]
);

#[derive(PartialEq, Clone, Copy, Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionCategoryFilter {
    Themes,
    IconThemes,
    Languages,
    Grammars,
    LanguageServers,
    ContextServers,
    AgentServers,
    Snippets,
    DebugAdapters,
}

/// 打开扩展管理界面
#[derive(PartialEq, Clone, Default, Debug, Deserialize, JsonSchema, Action)]
#[action(namespace = zed)]
#[serde(deny_unknown_fields)]
pub struct Extensions {
    /// 将扩展页面筛选为指定分类下的扩展
    #[serde(default)]
    pub category_filter: Option<ExtensionCategoryFilter>,
    /// 仅聚焦指定ID的扩展
    #[serde(default)]
    pub id: Option<String>,
}

/// 打开ACP注册表
#[derive(PartialEq, Clone, Default, Debug, Deserialize, JsonSchema, Action)]
#[action(namespace = zed)]
#[serde(deny_unknown_fields)]
pub struct AcpRegistry;

/// 显示调用诊断信息和连接质量统计数据
#[derive(PartialEq, Clone, Default, Debug, Deserialize, JsonSchema, Action)]
#[action(namespace = collab)]
#[serde(deny_unknown_fields)]
pub struct ShowCallStats;

/// 减小编辑器缓冲区的字体大小
#[derive(PartialEq, Clone, Default, Debug, Deserialize, JsonSchema, Action)]
#[action(namespace = zed)]
#[serde(deny_unknown_fields)]
pub struct DecreaseBufferFontSize {
    #[serde(default)]
    pub persist: bool,
}

/// 增大编辑器缓冲区的字体大小
#[derive(PartialEq, Clone, Default, Debug, Deserialize, JsonSchema, Action)]
#[action(namespace = zed)]
#[serde(deny_unknown_fields)]
pub struct IncreaseBufferFontSize {
    #[serde(default)]
    pub persist: bool,
}

/// 在指定路径打开设置编辑器
#[derive(PartialEq, Clone, Debug, Deserialize, JsonSchema, Action)]
#[action(namespace = zed)]
#[serde(deny_unknown_fields)]
pub struct OpenSettingsAt {
    /// 指向特定设置的路径（例如theme.mode）
    pub path: String,
}

/// 将缓冲区字体大小重置为默认值
#[derive(PartialEq, Clone, Default, Debug, Deserialize, JsonSchema, Action)]
#[action(namespace = zed)]
#[serde(deny_unknown_fields)]
pub struct ResetBufferFontSize {
    #[serde(default)]
    pub persist: bool,
}

/// 减小用户界面的字体大小
#[derive(PartialEq, Clone, Default, Debug, Deserialize, JsonSchema, Action)]
#[action(namespace = zed)]
#[serde(deny_unknown_fields)]
pub struct DecreaseUiFontSize {
    #[serde(default)]
    pub persist: bool,
}

/// 增大用户界面的字体大小
#[derive(PartialEq, Clone, Default, Debug, Deserialize, JsonSchema, Action)]
#[action(namespace = zed)]
#[serde(deny_unknown_fields)]
pub struct IncreaseUiFontSize {
    #[serde(default)]
    pub persist: bool,
}

/// 将界面字体大小重置为默认值
#[derive(PartialEq, Clone, Default, Debug, Deserialize, JsonSchema, Action)]
#[action(namespace = zed)]
#[serde(deny_unknown_fields)]
pub struct ResetUiFontSize {
    #[serde(default)]
    pub persist: bool,
}

/// 将所有缩放级别（界面和缓冲区字体大小，包括代理面板）重置为默认值
#[derive(PartialEq, Clone, Default, Debug, Deserialize, JsonSchema, Action)]
#[action(namespace = zed)]
#[serde(deny_unknown_fields)]
pub struct ResetAllZoom {
    #[serde(default)]
    pub persist: bool,
}

pub mod editor {
    use gpui::actions;
    actions!(
        editor,
        [
            /// 光标上移
            MoveUp,
            /// 光标下移
            MoveDown,
            /// 在系统文件管理器中显示当前文件
            RevealInFileManager,
        ]
    );
}

pub mod dev {
    use gpui::actions;

    actions!(
        dev,
        [
            /// 切换开发者检查器用于调试界面元素
            ToggleInspector
        ]
    );
}

pub mod remote_debug {
    use gpui::actions;

    actions!(
        remote_debug,
        [
            /// 模拟与远程服务器断开连接用于测试
            /// 这将触发重连逻辑
            SimulateDisconnect,
            /// 模拟到远程服务器的超时/慢速连接用于测试
            /// 这将导致心跳失败并触发重连
            SimulateTimeout,
            /// 模拟到远程服务器的超时/慢速连接用于测试
            /// 这将导致心跳失败并在所有尝试耗尽后尝试重连
            SimulateTimeoutExhausted,
        ]
    );
}

pub mod workspace {
    use gpui::actions;

    actions!(
        workspace,
        [
            #[action(deprecated_aliases = ["editor::CopyPath", "outline_panel::CopyPath", "project_panel::CopyPath"])]
            CopyPath,
            #[action(deprecated_aliases = ["editor::CopyRelativePath", "outline_panel::CopyRelativePath", "project_panel::CopyRelativePath"])]
            CopyRelativePath,
            /// 使用系统默认应用打开选中的文件
            #[action(deprecated_aliases = ["project_panel::OpenWithSystem"])]
            OpenWithSystem,
        ]
    );
}

/// 描述基于哪个引用创建新的Git工作树
/// 工作树始终以分离头状态创建；用户可后续从工作树中选择创建分支
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum NewWorktreeBranchTarget {
    /// 从当前HEAD创建分离工作树
    #[default]
    CurrentBranch,
    /// 在现有分支的最新提交处创建分离工作树
    ExistingBranch { name: String },
}

/// 创建新的Git工作树并将工作空间切换至该树
/// 当用户选择"创建新工作树"选项时，由统一工作树选择器触发
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Action)]
#[action(namespace = git)]
#[serde(deny_unknown_fields)]
pub struct CreateWorktree {
    /// 当该值为None时，Zed将随机生成工作树名称
    pub worktree_name: Option<String>,
    pub branch_target: NewWorktreeBranchTarget,
}

/// 将工作空间切换至现有的已关联工作树
/// 当用户选择现有工作树时，由统一工作树选择器触发
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Action)]
#[action(namespace = git)]
#[serde(deny_unknown_fields)]
pub struct SwitchWorktree {
    pub path: PathBuf,
    pub display_name: String,
}

/// 在新窗口中打开现有工作树
/// 由工作树选择器的"在新窗口中打开"按钮触发
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Action)]
#[action(namespace = git)]
#[serde(deny_unknown_fields)]
pub struct OpenWorktreeInNewWindow {
    pub path: PathBuf,
}

pub mod git {
    use gpui::actions;

    actions!(
        git,
        [
            /// 切换到其他Git分支
            CheckoutBranch,
            /// 切换到其他Git分支
            Switch,
            /// 选择其他仓库
            SelectRepo,
            /// 筛选远程仓库
            FilterRemotes,
            /// 创建Git远程仓库
            CreateRemote,
            /// 打开Git分支选择器
            #[action(deprecated_aliases = ["branches::OpenRecent"])]
            Branch,
            /// 打开Git储藏选择器
            ViewStash,
            /// 打开Git工作树选择器
            Worktree,
            /// 为当前分支创建拉取请求
            CreatePullRequest
        ]
    );
}

pub mod toast {
    use gpui::actions;

    actions!(
        toast,
        [
            /// 执行提示通知关联的动作
            RunAction
        ]
    );
}

pub mod command_palette {
    use gpui::actions;

    actions!(
        command_palette,
        [
            /// 切换命令面板
            Toggle,
        ]
    );
}

pub mod project_panel {
    use gpui::actions;

    actions!(
        project_panel,
        [
            /// 切换项目面板
            Toggle,
            /// 切换项目面板焦点
            ToggleFocus
        ]
    );
}
pub mod feedback {
    use gpui::actions;

    actions!(
        feedback,
        [
            /// 打开邮件客户端向Zed支持发送反馈
            EmailZed,
            /// 打开缺陷报告表单
            FileBugReport,
            /// 打开功能请求表单
            RequestFeature
        ]
    );
}

pub mod theme {
    use gpui::actions;

    actions!(theme, [ToggleMode]);
}

pub mod theme_selector {
    use gpui::Action;
    use schemars::JsonSchema;
    use serde::Deserialize;

    /// 切换主题选择器界面
    #[derive(PartialEq, Clone, Default, Debug, Deserialize, JsonSchema, Action)]
    #[action(namespace = theme_selector)]
    #[serde(deny_unknown_fields)]
    pub struct Toggle {
        /// 用于筛选主题选择器的主题名称列表
        pub themes_filter: Option<Vec<String>>,
    }
}

pub mod icon_theme_selector {
    use gpui::Action;
    use schemars::JsonSchema;
    use serde::Deserialize;

    /// 切换图标主题选择器界面
    #[derive(PartialEq, Clone, Default, Debug, Deserialize, JsonSchema, Action)]
    #[action(namespace = icon_theme_selector)]
    #[serde(deny_unknown_fields)]
    pub struct Toggle {
        /// 用于筛选主题选择器的图标主题名称列表
        pub themes_filter: Option<Vec<String>>,
    }
}

pub mod search {
    use gpui::actions;
    actions!(
        search,
        [
            /// 切换是否搜索被忽略的文件
            ToggleIncludeIgnored
        ]
    );
}
pub mod buffer_search {
    use gpui::{Action, actions};
    use schemars::JsonSchema;
    use serde::Deserialize;

    /// 使用指定配置打开缓冲区搜索界面
    #[derive(PartialEq, Clone, Deserialize, JsonSchema, Action)]
    #[action(namespace = buffer_search)]
    #[serde(deny_unknown_fields)]
    pub struct Deploy {
        #[serde(default = "util::serde::default_true")]
        pub focus: bool,
        #[serde(default)]
        pub replace_enabled: bool,
        #[serde(default)]
        pub selection_search_enabled: bool,
    }

    impl Deploy {
        pub fn find() -> Self {
            Self {
                focus: true,
                replace_enabled: false,
                selection_search_enabled: false,
            }
        }

        pub fn replace() -> Self {
            Self {
                focus: true,
                replace_enabled: true,
                selection_search_enabled: false,
            }
        }
    }

    actions!(
        buffer_search,
        [
            /// 打开搜索替换界面
            DeployReplace,
            /// 关闭搜索栏
            Dismiss,
            /// 将焦点切回编辑器
            FocusEditor,
            /// 将当前选中内容设为搜索查询，不打开搜索栏或执行搜索
            UseSelectionForFind,
        ]
    );
}
pub mod settings_profile_selector {
    use gpui::Action;
    use schemars::JsonSchema;
    use serde::Deserialize;

    #[derive(PartialEq, Clone, Default, Debug, Deserialize, JsonSchema, Action)]
    #[action(namespace = settings_profile_selector)]
    pub struct Toggle;
}

pub mod agent {
    use gpui::{Action, SharedString, actions};
    use schemars::JsonSchema;
    use serde::Deserialize;

    actions!(
        agent,
        [
            /// 打开代理设置面板
            #[action(deprecated_aliases = ["agent::OpenConfiguration"])]
            OpenSettings,
            /// 打开代理引导弹窗
            OpenOnboardingModal,
            /// 重置代理引导状态
            ResetOnboarding,
            /// 开始与代理的聊天对话
            Chat,
            /// 切换语言模型选择下拉框
            #[action(deprecated_aliases = ["assistant::ToggleModelSelector", "assistant2::ToggleModelSelector"])]
            ToggleModelSelector,
            /// 触发Gemini重新认证
            ReauthenticateAgent,
            /// 将当前选中内容添加为代理面板线程的上下文
            #[action(deprecated_aliases = ["assistant::QuoteSelection", "agent::QuoteSelection"])]
            AddSelectionToThread,
            /// 重置代理面板缩放级别（代理界面和缓冲区字体大小）
            ResetAgentZoom,
            /// 粘贴剪贴板内容且不保留任何格式
            PasteRaw,
        ]
    );

    /// 打开新的代理线程，使用提供的分支差异进行审查
    #[derive(Clone, PartialEq, Deserialize, JsonSchema, Action)]
    #[action(namespace = agent)]
    #[serde(deny_unknown_fields)]
    pub struct ReviewBranchDiff {
        /// 待审查的差异完整文本
        pub diff_text: SharedString,
        /// 计算差异所基于的基准引用（例如main）
        pub base_ref: SharedString,
    }

    /// 从文件中提取的单个合并冲突区域
    #[derive(Clone, Debug, PartialEq, Deserialize, JsonSchema)]
    pub struct ConflictContent {
        pub file_path: String,
        pub conflict_text: String,
        pub ours_branch_name: String,
        pub theirs_branch_name: String,
    }

    /// 打开新的代理线程以解决特定的合并冲突
    #[derive(Clone, PartialEq, Deserialize, JsonSchema, Action)]
    #[action(namespace = agent)]
    #[serde(deny_unknown_fields)]
    pub struct ResolveConflictsWithAgent {
        /// 包含完整文本的各个冲突
        pub conflicts: Vec<ConflictContent>,
    }

    /// 打开新的代理线程以解决指定文件路径中的合并冲突
    #[derive(Clone, PartialEq, Deserialize, JsonSchema, Action)]
    #[action(namespace = agent)]
    #[serde(deny_unknown_fields)]
    pub struct ResolveConflictedFilesWithAgent {
        /// 存在未解决冲突的文件路径（用于项目范围解决）
        pub conflicted_file_paths: Vec<String>,
    }
}

pub mod assistant {
    use gpui::{Action, actions};
    use schemars::JsonSchema;
    use serde::Deserialize;
    use uuid::Uuid;

    actions!(
        agent,
        [
            /// 切换代理面板
            Toggle,
            #[action(deprecated_aliases = ["assistant::ToggleFocus"])]
            ToggleFocus,
            FocusAgent,
        ]
    );

    /// 打开规则库，用于管理代理规则和提示词
    #[derive(PartialEq, Clone, Default, Debug, Deserialize, JsonSchema, Action)]
    #[action(namespace = agent, deprecated_aliases = ["assistant::OpenRulesLibrary", "assistant::DeployPromptLibrary"])]
    #[serde(deny_unknown_fields)]
    pub struct OpenRulesLibrary {
        #[serde(skip)]
        pub prompt_to_select: Option<Uuid>,
    }

    /// 使用指定配置打开助手界面
    #[derive(Clone, Default, Deserialize, PartialEq, JsonSchema, Action)]
    #[action(namespace = assistant)]
    #[serde(deny_unknown_fields)]
    pub struct InlineAssist {
        pub prompt: Option<String>,
    }
}

/// 打开最近项目界面
#[derive(PartialEq, Clone, Deserialize, Default, JsonSchema, Action)]
#[action(namespace = projects)]
#[serde(deny_unknown_fields)]
pub struct OpenRecent {
    #[serde(default)]
    pub create_new_window: bool,
}

/// 从选中的模板创建项目
#[derive(PartialEq, Clone, Deserialize, Default, JsonSchema, Action)]
#[action(namespace = projects)]
#[serde(deny_unknown_fields)]
pub struct OpenRemote {
    #[serde(default)]
    pub from_existing_connection: bool,
    #[serde(default)]
    pub create_new_window: bool,
}

/// 打开开发容器连接弹窗
#[derive(PartialEq, Clone, Deserialize, Default, JsonSchema, Action)]
#[action(namespace = projects)]
#[serde(deny_unknown_fields)]
pub struct OpenDevContainer;

/// 任务在界面中的显示位置
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RevealTarget {
    /// 在中央面板组，主编辑区域
    Center,
    /// 在终端停靠栏，常规终端项目位置
    #[default]
    Dock,
}

/// 通过名称启动任务或打开任务选择弹窗
#[derive(Debug, PartialEq, Clone, Deserialize, JsonSchema, Action)]
#[action(namespace = task)]
#[serde(untagged)]
pub enum Spawn {
    /// 通过指定名称启动任务
    ByName {
        task_name: String,
        #[serde(default)]
        reveal_target: Option<RevealTarget>,
    },
    /// 通过指定标签启动任务
    ByTag {
        task_tag: String,
        #[serde(default)]
        reveal_target: Option<RevealTarget>,
    },
    /// 通过弹窗选择启动任务
    ViaModal {
        /// 覆盖选中任务的reveal_target属性
        #[serde(default)]
        reveal_target: Option<RevealTarget>,
    },
}

impl Spawn {
    pub fn modal() -> Self {
        Self::ViaModal {
            reveal_target: None,
        }
    }
}

/// 重新运行上一个任务
#[derive(PartialEq, Clone, Deserialize, Default, JsonSchema, Action)]
#[action(namespace = task)]
#[serde(deny_unknown_fields)]
pub struct Rerun {
    /// 控制执行任务前是否重新评估上下文
    /// 若为否，ZED_COLUMN、ZED_FILE等环境变量将与上次执行时保持一致
    /// 若为是，这些变量将更新为执行task::Rerun时编辑器的当前状态
    /// 默认值：false
    #[serde(default)]
    pub reevaluate_context: bool,
    /// 覆盖待重新运行任务的allow_concurrent_runs属性
    /// 默认值：null
    #[serde(default)]
    pub allow_concurrent_runs: Option<bool>,
    /// 覆盖待重新运行任务的use_new_terminal属性
    /// 默认值：null
    #[serde(default)]
    pub use_new_terminal: Option<bool>,

    /// 若存在，则重新运行此ID的任务，否则重新运行上一个任务
    #[serde(skip)]
    pub task_id: Option<String>,
}

pub mod outline {
    use std::sync::OnceLock;

    use gpui::{AnyView, App, Window, actions};

    actions!(
        outline,
        [
            #[action(name = "Toggle")]
            ToggleOutline
        ]
    );
    /// 指向outline::toggle函数的指针，在此处暴露以处理面包屑与大纲的依赖关系
    pub static TOGGLE_OUTLINE: OnceLock<fn(AnyView, &mut Window, &mut App)> = OnceLock::new();
}

actions!(
    zed_predict_onboarding,
    [
        /// 打开Zed Predict引导弹窗
        OpenZedPredictOnboarding
    ]
);
actions!(
    git_onboarding,
    [
        /// 打开Git集成引导弹窗
        OpenGitIntegrationOnboarding
    ]
);

pub mod debug_panel {
    use gpui::actions;
    actions!(
        debug_panel,
        [
            /// 切换调试面板
            Toggle,
            /// 切换调试面板焦点
            ToggleFocus
        ]
    );
}

actions!(
    debugger,
    [
        /// 切换断点的启用状态
        ToggleEnableBreakpoint,
        /// 移除断点
        UnsetBreakpoint,
        /// 打开项目调试任务配置
        OpenProjectDebugTasks,
    ]
);

pub mod vim {
    use gpui::actions;

    actions!(
        vim,
        [
            /// 打开默认快捷键映射文件
            OpenDefaultKeymap
        ]
    );
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WslConnectionOptions {
    pub distro_name: String,
    pub user: Option<String>,
}

#[cfg(target_os = "windows")]
pub mod wsl_actions {
    use gpui::Action;
    use schemars::JsonSchema;
    use serde::Deserialize;

    /// 在WSL中打开文件夹
    #[derive(PartialEq, Clone, Deserialize, Default, JsonSchema, Action)]
    #[action(namespace = projects)]
    #[serde(deny_unknown_fields)]
    pub struct OpenFolderInWsl {
        #[serde(default)]
        pub create_new_window: bool,
    }

    /// 打开Wsl发行版
    #[derive(PartialEq, Clone, Deserialize, Default, JsonSchema, Action)]
    #[action(namespace = projects)]
    #[serde(deny_unknown_fields)]
    pub struct OpenWsl {
        #[serde(default)]
        pub create_new_window: bool,
    }
}

pub mod preview {
    pub mod markdown {
        use gpui::actions;

        actions!(
            markdown,
            [
                /// 为当前文件打开Markdown预览
                OpenPreview,
                /// 在分栏面板中打开Markdown预览
                OpenPreviewToTheSide,
            ]
        );
    }

    pub mod svg {
        use gpui::actions;

        actions!(
            svg,
            [
                /// 为当前文件打开SVG预览
                OpenPreview,
                /// 在分栏面板中打开SVG预览
                OpenPreviewToTheSide,
            ]
        );
    }
}

pub mod agents_sidebar {
    use gpui::{Action, actions};
    use schemars::JsonSchema;
    use serde::Deserialize;

    /// 当侧边栏获得焦点时，切换线程选择器弹出窗口
    #[derive(PartialEq, Clone, Deserialize, JsonSchema, Default, Action)]
    #[action(namespace = agents_sidebar)]
    #[serde(deny_unknown_fields)]
    pub struct ToggleThreadSwitcher {
        #[serde(default)]
        pub select_last: bool,
    }

    actions!(
        agents_sidebar,
        [
            /// 将焦点移至侧边栏的搜索/筛选编辑器
            FocusSidebarFilter,
        ]
    );
}

pub mod notebook {
    use gpui::actions;

    actions!(
        notebook,
        [
            /// 打开Jupyter笔记本文件
            OpenNotebook,
            /// 运行笔记本中的所有单元格
            RunAll,
            /// 运行当前单元格并保持选中
            Run,
            /// 运行当前单元格并切换至下一个单元格
            RunAndAdvance,
            /// 清除所有单元格输出
            ClearOutputs,
            /// 将当前单元格上移
            MoveCellUp,
            /// 将当前单元格下移
            MoveCellDown,
            /// 添加新的Markdown单元格
            AddMarkdownBlock,
            /// 添加新的代码单元格
            AddCodeBlock,
            /// 重启内核
            RestartKernel,
            /// 中断当前执行
            InterruptKernel,
            /// 在单元格中向下移动
            NotebookMoveDown,
            /// 在单元格中向上移动
            NotebookMoveUp,
            /// 进入当前单元格的编辑器（编辑模式）
            EnterEditMode,
            /// 退出单元格编辑器并返回单元格命令模式
            EnterCommandMode,
        ]
    );
}