use std::num::NonZeroUsize;

use collections::HashMap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use settings_macros::{MergeFrom, with_fallible_options};

use crate::{
    ActionName, CenteredPaddingSettings, DelayMs, DockPosition, DockSide, InactiveOpacity,
    ShowIndentGuides, ShowScrollbar, serialize_optional_f32_with_two_decimal_places,
};

/// 工作区核心设置
#[with_fallible_options]
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize, JsonSchema, MergeFrom)]
pub struct WorkspaceSettingsContent {
    /// 激活窗格的视觉样式设置
    pub active_pane_modifiers: Option<ActivePaneModifiers>,
    /// 文本渲染模式
    /// 默认值：platform_default（系统默认）
    pub text_rendering_mode: Option<TextRenderingMode>,
    /// 底部停靠栏的布局模式
    /// 默认值：contained（包含式）
    pub bottom_dock_layout: Option<BottomDockLayout>,
    /// 水平分割窗格的方向
    /// 默认值：up（向上）
    pub pane_split_direction_horizontal: Option<PaneSplitDirectionHorizontal>,
    /// 垂直分割窗格的方向
    /// 默认值：left（向左）
    pub pane_split_direction_vertical: Option<PaneSplitDirectionVertical>,
    /// 居中布局相关设置
    pub centered_layout: Option<CenteredLayoutSettings>,
    /// 关闭应用前是否弹出确认提示
    /// 默认值：false
    pub confirm_quit: Option<bool>,
    /// 是否在状态栏显示通话状态图标
    /// 默认值：true
    pub show_call_status_icon: Option<bool>,
    /// 自动保存编辑中缓冲区的策略
    /// 默认值：off（关闭）
    pub autosave: Option<AutosaveSetting>,
    /// 启动 Zed 时恢复上次会话的策略
    /// 可选值：empty_tab, last_workspace, last_session, launchpad
    /// 默认值：last_session（恢复上次会话）
    pub restore_on_startup: Option<RestoreOnStartupBehavior>,
    /// 从命令行打开路径时，未指定 -e/-n 标志的默认行为
    /// 默认值：existing_window（现有窗口）
    pub cli_default_open_behavior: Option<CliDefaultOpenBehavior>,
    /// 重新打开文件时，是否恢复该文件上次的编辑状态（选区、折叠、滚动位置）
    /// 状态按窗格保存，禁用则使用默认状态
    /// 默认值：true
    pub restore_on_file_reopen: Option<bool>,
    /// 工作区边缘分割拖放目标的大小
    /// 以工作区较小边的比例表示
    /// 默认值：0.2（20%）
    #[serde(serialize_with = "serialize_optional_f32_with_two_decimal_places")]
    pub drop_target_size: Option<f32>,
    /// 在无标签页的工作区执行「关闭当前项」时，是否关闭窗口
    /// 默认值：auto（macOS 开启，其他系统关闭）
    pub when_closing_with_no_tabs: Option<CloseWindowWhenNoItems>,
    /// 是否使用系统原生的文件打开/保存对话框
    /// 禁用则使用 Zed 内置的键盘优先选择器
    /// 默认值：true
    pub use_system_path_prompts: Option<bool>,
    /// 是否使用系统原生提示框
    /// 禁用则使用 Zed 内置提示框（Linux 无效）
    /// 默认值：true
    pub use_system_prompts: Option<bool>,
    /// 命令面板别名：输入别名等价于对应动作
    /// 默认值：空
    #[serde(default)]
    pub command_aliases: HashMap<String, ActionName>,
    /// 单个窗格最大打开标签页数量，不会关闭未保存文件
    /// 设置为 None 表示无限制
    /// 默认值：none
    pub max_tabs: Option<NonZeroUsize>,
    /// 关闭最后一个窗口时的行为
    /// 默认值：auto（macOS 不退出，其他系统退出）
    pub on_last_window_closed: Option<OnLastWindowClosed>,
    /// 调整停靠栏大小时，是否同时调整停靠栏内所有面板
    /// 默认值：["left"]（仅左侧）
    pub resize_all_panels_in_dock: Option<Vec<DockPosition>>,
    /// 磁盘文件被删除时，是否自动关闭对应打开的文件
    /// 默认值：false
    pub close_on_file_delete: Option<bool>,
    /// 是否允许按系统偏好组合窗口标签（仅 macOS）
    /// 默认值：false
    pub use_system_window_tabs: Option<bool>,
    /// 缩放面板时是否显示内边距
    /// 底部缩放面板显示顶部内边距，左右面板显示对应侧边内边距
    /// 默认值：true
    pub zoomed_padding: Option<bool>,
    /// 切换面板（快捷键）时，若面板已聚焦，是否直接关闭
    /// 禁用则仅将焦点切回编辑器
    /// 默认值：false
    pub close_panel_on_toggle: Option<bool>,
    /// 窗口装饰/标题栏由谁渲染：客户端（Zed）或系统
    /// 默认值：client
    pub window_decorations: Option<WindowDecorations>,
    /// 焦点是否跟随鼠标移动
    /// 默认值：false
    pub focus_follows_mouse: Option<FocusFollowsMouse>,
}

/// 标签项/面板项通用设置
#[with_fallible_options]
#[derive(Clone, Default, Debug, PartialEq, Serialize, Deserialize, JsonSchema, MergeFrom)]
pub struct ItemSettingsContent {
    /// 是否在标签上显示 Git 文件状态
    /// 默认值：false
    pub git_status: Option<bool>,
    /// 标签关闭按钮的位置
    /// 默认值：right（右侧）
    pub close_position: Option<ClosePosition>,
    /// 是否在标签上显示文件图标
    /// 默认值：false
    pub file_icons: Option<bool>,
    /// 关闭当前标签后，激活哪个标签
    /// 默认值：history（历史记录）
    pub activate_on_close: Option<ActivateOnClose>,
    /// 在标签上标记包含诊断错误/警告的文件
    /// 默认值：off（关闭）
    pub show_diagnostics: Option<ShowDiagnostics>,
    /// 是否始终显示标签关闭按钮
    /// 默认值：false（仅悬停显示）
    pub show_close_button: Option<ShowCloseButton>,
}

/// 预览标签页设置
#[with_fallible_options]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema, MergeFrom)]
pub struct PreviewTabsSettingsContent {
    /// 是否将打开的编辑器显示为预览标签
    /// 预览标签不会常驻，双击/编辑后变为固定标签，文件名斜体显示
    /// 默认值：true
    pub enabled: Option<bool>,
    /// 在项目面板单击打开文件时，是否使用预览模式
    /// 默认值：true
    pub enable_preview_from_project_panel: Option<bool>,
    /// 在文件搜索器打开文件时，是否使用预览模式
    /// 默认值：false
    pub enable_preview_from_file_finder: Option<bool>,
    /// 从多缓冲区打开文件时，是否使用预览模式
    /// 默认值：true
    pub enable_preview_from_multibuffer: Option<bool>,
    /// 代码导航打开多缓冲区时，是否使用预览模式
    /// 默认值：false
    pub enable_preview_multibuffer_from_code_navigation: Option<bool>,
    /// 代码导航打开单个文件时，是否使用预览模式
    /// 默认值：true
    pub enable_preview_file_from_code_navigation: Option<bool>,
    /// 代码导航离开预览标签时，是否保持其预览状态
    /// 若关联导航预览开启，新标签可能覆盖现有标签
    /// 默认值：false
    pub enable_keep_preview_on_code_navigation: Option<bool>,
}

/// 标签关闭按钮位置
#[derive(
    Copy,
    Clone,
    Debug,
    PartialEq,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "lowercase")]
pub enum ClosePosition {
    /// 左侧
    Left,
    /// 右侧（默认）
    #[default]
    Right,
}

/// 标签关闭按钮显示策略
#[derive(
    Copy,
    Clone,
    Debug,
    PartialEq,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "lowercase")]
pub enum ShowCloseButton {
    /// 始终显示
    Always,
    /// 仅悬停显示（默认）
    #[default]
    Hover,
    /// 隐藏
    Hidden,
}

/// 诊断信息显示等级
#[derive(
    Copy,
    Clone,
    Debug,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
    MergeFrom,
    PartialEq,
    Eq,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum ShowDiagnostics {
    /// 关闭（默认）
    #[default]
    Off,
    /// 仅显示错误
    Errors,
    /// 显示错误和警告
    All,
}

/// 关闭标签后激活策略
#[derive(
    Copy,
    Clone,
    Debug,
    PartialEq,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum ActivateOnClose {
    /// 按历史记录（默认）
    #[default]
    History,
    /// 相邻标签
    Neighbour,
    /// 左侧相邻标签
    LeftNeighbour,
}

/// 激活窗格样式配置
#[with_fallible_options]
#[derive(Copy, Clone, PartialEq, Debug, Default, Serialize, Deserialize, JsonSchema, MergeFrom)]
#[serde(rename_all = "snake_case")]
pub struct ActivePaneModifiers {
    /// 激活窗格边框宽度（内描边）
    /// 0 表示无边框
    /// 默认值：0.0
    #[serde(serialize_with = "crate::serialize_optional_f32_with_two_decimal_places")]
    pub border_size: Option<f32>,
    /// 非激活窗格透明度
    /// 1.0 表示与激活窗格一致，0 表示完全隐藏
    /// 取值范围 0.0~1.0
    /// 默认值：1.0
    #[schemars(range(min = 0.0, max = 1.0))]
    pub inactive_opacity: Option<InactiveOpacity>,
}

/// 底部停靠栏布局
#[derive(
    Copy,
    Clone,
    Debug,
    Default,
    Serialize,
    Deserialize,
    PartialEq,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum BottomDockLayout {
    /// 包含在左右停靠栏之间（默认）
    #[default]
    Contained,
    /// 全屏宽度
    Full,
    /// 左对齐，延伸到左侧停靠栏下方
    LeftAligned,
    /// 右对齐，延伸到右侧停靠栏下方
    RightAligned,
}

/// 窗口装饰渲染方式
#[derive(
    Copy,
    Clone,
    Default,
    Debug,
    Serialize,
    Deserialize,
    PartialEq,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum WindowDecorations {
    /// Zed 客户端渲染（默认）
    #[default]
    Client,
    /// 系统服务端渲染（GNOME Wayland 不支持）
    Server,
}

/// 无标签页时关闭窗口策略
#[derive(
    Copy,
    Clone,
    PartialEq,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
    MergeFrom,
    Debug,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum CloseWindowWhenNoItems {
    /// 遵循系统默认（macOS 关闭，其他保留）
    #[default]
    PlatformDefault,
    /// 关闭窗口
    CloseWindow,
    /// 保留窗口
    KeepWindowOpen,
}

impl CloseWindowWhenNoItems {
    /// 判断是否需要关闭窗口
    pub fn should_close(&self) -> bool {
        match self {
            CloseWindowWhenNoItems::PlatformDefault => cfg!(target_os = "macos"),
            CloseWindowWhenNoItems::CloseWindow => true,
            CloseWindowWhenNoItems::KeepWindowOpen => false,
        }
    }
}

/// 命令行默认打开行为
#[derive(
    Copy,
    Clone,
    PartialEq,
    Eq,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
    MergeFrom,
    Debug,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum CliDefaultOpenBehavior {
    /// 添加到现有窗口（默认）
    #[default]
    #[strum(serialize = "添加到现有窗口")]
    ExistingWindow,
    /// 打开新窗口
    #[strum(serialize = "打开新窗口")]
    NewWindow,
}

/// 启动恢复行为
#[derive(
    Copy,
    Clone,
    PartialEq,
    Eq,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
    MergeFrom,
    Debug,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum RestoreOnStartupBehavior {
    /// 空白标签页
    #[serde(alias = "none")]
    EmptyTab,
    /// 恢复上次工作区
    LastWorkspace,
    /// 恢复上次所有会话（默认）
    #[default]
    LastSession,
    /// 显示启动面板
    Launchpad,
}

/// 标签栏设置
#[with_fallible_options]
#[derive(Clone, Default, Serialize, Deserialize, JsonSchema, MergeFrom, Debug, PartialEq)]
pub struct TabBarSettingsContent {
    /// 是否显示标签栏
    /// 默认值：true
    pub show: Option<bool>,
    /// 是否显示导航历史按钮
    /// 默认值：true
    pub show_nav_history_buttons: Option<bool>,
    /// 是否显示标签栏操作按钮
    /// 默认值：true
    pub show_tab_bar_buttons: Option<bool>,
    /// 是否将固定标签显示在独立行
    /// 开启后固定标签在上，普通标签在下
    /// 默认值：false
    pub show_pinned_tabs_in_separate_row: Option<bool>,
}

/// 状态栏设置
#[with_fallible_options]
#[derive(Clone, Default, Serialize, Deserialize, JsonSchema, MergeFrom, Debug, PartialEq, Eq)]
pub struct StatusBarSettingsContent {
    /// 是否显示状态栏（实验性）
    /// 默认值：true
    #[serde(rename = "experimental.show")]
    pub show: Option<bool>,
    /// 是否在状态栏显示当前激活文件名
    /// 默认值：false
    pub show_active_file: Option<bool>,
    /// 是否显示当前语言按钮
    /// 默认值：true
    pub active_language_button: Option<bool>,
    /// 是否显示光标位置按钮
    /// 默认值：true
    pub cursor_position_button: Option<bool>,
    /// 是否显示行尾格式按钮
    /// 默认值：false
    pub line_endings_button: Option<bool>,
    /// 是否显示文件编码按钮
    /// 默认值：non_utf8（仅非 UTF-8 显示）
    pub active_encoding_button: Option<EncodingDisplayOptions>,
}

/// 编码显示选项
#[derive(
    Copy,
    Clone,
    Debug,
    Eq,
    PartialEq,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
    MergeFrom,
    strum::VariantNames,
    strum::VariantArray,
)]
#[serde(rename_all = "snake_case")]
pub enum EncodingDisplayOptions {
    /// 始终显示
    Enabled,
    /// 始终隐藏
    Disabled,
    /// 仅非 UTF-8 显示（默认）
    #[default]
    NonUtf8,
}

impl EncodingDisplayOptions {
    /// 判断是否需要显示编码
    pub fn should_show(&self, is_utf8: bool, has_bom: bool) -> bool {
        match self {
            Self::Disabled => false,
            Self::Enabled => true,
            Self::NonUtf8 => {
                let is_standard_utf8 = is_utf8 && !has_bom;
                !is_standard_utf8
            }
        }
    }
}

/// 自动保存策略
#[derive(
    Copy,
    Clone,
    Debug,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    JsonSchema,
    MergeFrom,
    strum::EnumDiscriminants,
)]
#[strum_discriminants(derive(strum::VariantArray, strum::VariantNames, strum::FromRepr))]
#[serde(rename_all = "snake_case")]
pub enum AutosaveSetting {
    /// 关闭自动保存
    Off,
    /// 闲置指定毫秒后保存
    AfterDelay { milliseconds: DelayMs },
    /// 焦点改变时保存
    OnFocusChange,
    /// 窗口切换时保存
    OnWindowChange,
}

impl AutosaveSetting {
    /// 关闭时是否需要保存
    pub fn should_save_on_close(&self) -> bool {
        matches!(
            &self,
            AutosaveSetting::OnFocusChange
                | AutosaveSetting::OnWindowChange
                | AutosaveSetting::AfterDelay { .. }
        )
    }
}

/// 水平分割方向
#[derive(
    Copy,
    Clone,
    Debug,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum PaneSplitDirectionHorizontal {
    /// 向上
    Up,
    /// 向下
    Down,
}

/// 垂直分割方向
#[derive(
    Copy,
    Clone,
    Debug,
    Serialize,
    Deserialize,
    PartialEq,
    Eq,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum PaneSplitDirectionVertical {
    /// 向左
    Left,
    /// 向右
    Right,
}

/// 居中布局设置
#[derive(Copy, Clone, Debug, Serialize, Deserialize, JsonSchema, MergeFrom, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
#[with_fallible_options]
pub struct CenteredLayoutSettings {
    /// 居中布局左侧内边距比例
    /// 默认值：0.2
    pub left_padding: Option<CenteredPaddingSettings>,
    /// 居中布局右侧内边距比例
    /// 默认值：0.2
    pub right_padding: Option<CenteredPaddingSettings>,
}

/// 关闭最后一个窗口行为
#[derive(
    Copy,
    Clone,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
    MergeFrom,
    PartialEq,
    Debug,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum OnLastWindowClosed {
    /// 系统默认（macOS 不退出，其他退出）
    #[default]
    PlatformDefault,
    /// 退出应用
    QuitApp,
}

/// 文本渲染模式
#[derive(
    Copy,
    Clone,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
    MergeFrom,
    PartialEq,
    Eq,
    Debug,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum TextRenderingMode {
    /// 系统默认
    #[default]
    PlatformDefault,
    /// 子像素渲染（ClearType）
    Subpixel,
    /// 灰度渲染
    Grayscale,
}

impl OnLastWindowClosed {
    pub fn is_quit_app(&self) -> bool {
        match self {
            OnLastWindowClosed::PlatformDefault => false,
            OnLastWindowClosed::QuitApp => true,
        }
    }
}

/// 项目面板自动打开设置
#[with_fallible_options]
#[derive(Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema, MergeFrom, Debug)]
pub struct ProjectPanelAutoOpenSettings {
    /// 创建新文件时是否自动在编辑器打开
    /// 默认值：true
    pub on_create: Option<bool>,
    /// 粘贴/复制文件后是否自动打开
    /// 默认值：true
    pub on_paste: Option<bool>,
    /// 拖入外部文件时是否自动打开
    /// 默认值：true
    pub on_drop: Option<bool>,
}

/// 项目面板设置
#[with_fallible_options]
#[derive(Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema, MergeFrom, Debug)]
pub struct ProjectPanelSettingsContent {
    /// 是否在状态栏显示项目面板按钮
    /// 默认值：true
    pub button: Option<bool>,
    /// 是否隐藏 .gitignore 规则匹配的文件
    /// 默认值：false
    pub hide_gitignore: Option<bool>,
    /// 项目面板默认宽度（像素）
    /// 默认值：240
    #[serde(serialize_with = "crate::serialize_optional_f32_with_two_decimal_places")]
    pub default_width: Option<f32>,
    /// 项目面板停靠位置
    /// 默认值：left（左侧）
    pub dock: Option<DockSide>,
    /// 项目条目间距
    /// 默认值：comfortable（舒适）
    pub entry_spacing: Option<ProjectPanelEntrySpacing>,
    /// 是否显示文件图标
    /// 默认值：true
    pub file_icons: Option<bool>,
    /// 文件夹是否显示图标/箭头
    /// 默认值：true
    pub folder_icons: Option<bool>,
    /// 是否显示 Git 状态
    /// 默认值：true
    pub git_status: Option<bool>,
    /// 嵌套条目缩进大小（像素）
    /// 默认值：20
    #[serde(serialize_with = "serialize_optional_f32_with_two_decimal_places")]
    pub indent_size: Option<f32>,
    /// 激活文件时，是否在项目面板自动定位
    /// .gitignore 文件不会自动定位
    /// 默认值：true
    pub auto_reveal_entries: Option<bool>,
    /// 单层子文件夹是否自动折叠
    /// 默认值：true
    pub auto_fold_dirs: Option<bool>,
    /// 文件夹名称是否加粗
    /// 默认值：false
    pub bold_folder_labels: Option<bool>,
    /// 启动时是否自动打开项目面板
    /// 默认值：true
    pub starts_open: Option<bool>,
    /// 滚动条设置
    pub scrollbar: Option<ProjectPanelScrollbarSettingsContent>,
    /// 在项目面板标记诊断信息的等级
    /// 默认值：all
    pub show_diagnostics: Option<ShowDiagnostics>,
    /// 缩进引导线设置
    pub indent_guides: Option<ProjectPanelIndentGuidesSettings>,
    /// 单文件夹窗口时是否隐藏根目录
    /// 默认值：false
    pub hide_root: Option<bool>,
    /// 是否隐藏隐藏文件
    /// 默认值：false
    pub hide_hidden: Option<bool>,
    /// 父目录是否固定在顶部
    /// 默认值：true
    pub sticky_scroll: Option<bool>,
    /// 是否启用拖放操作
    /// 默认值：true
    pub drag_and_drop: Option<bool>,
    /// 自动打开文件设置
    pub auto_open: Option<ProjectPanelAutoOpenSettings>,
    /// 同级条目排序方式
    /// 默认值：directories_first（文件夹优先）
    pub sort_mode: Option<ProjectPanelSortMode>,
    /// 排序是否区分大小写
    /// 默认值：default
    pub sort_order: Option<ProjectPanelSortOrder>,
    /// 是否在文件名旁显示诊断数量徽章
    /// 默认值：false
    pub diagnostic_badges: Option<bool>,
    /// 是否显示 Git 状态指示器
    /// 默认值：false
    pub git_status_indicator: Option<bool>,
}

/// 项目面板条目间距
#[derive(
    Copy,
    Clone,
    Debug,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
    MergeFrom,
    PartialEq,
    Eq,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum ProjectPanelEntrySpacing {
    /// 舒适间距（默认）
    #[default]
    Comfortable,
    /// 标准紧凑间距
    Standard,
}

/// 项目面板排序模式
#[derive(
    Copy,
    Clone,
    Debug,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
    MergeFrom,
    PartialEq,
    Eq,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum ProjectPanelSortMode {
    /// 文件夹优先（默认）
    #[default]
    DirectoriesFirst,
    /// 文件与文件夹混合排序
    Mixed,
    /// 文件优先
    FilesFirst,
}

/// 项目面板排序规则
#[derive(
    Copy,
    Clone,
    Debug,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
    MergeFrom,
    PartialEq,
    Eq,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum ProjectPanelSortOrder {
    /// 默认自然排序（不区分大小写，数字按值排序）
    #[default]
    Default,
    /// 大写优先分组
    Upper,
    /// 小写优先分组
    Lower,
    /// 纯 Unicode 编码排序
    Unicode,
}

impl From<ProjectPanelSortMode> for util::paths::SortMode {
    fn from(mode: ProjectPanelSortMode) -> Self {
        match mode {
            ProjectPanelSortMode::DirectoriesFirst => Self::DirectoriesFirst,
            ProjectPanelSortMode::Mixed => Self::Mixed,
            ProjectPanelSortMode::FilesFirst => Self::FilesFirst,
        }
    }
}

impl From<ProjectPanelSortOrder> for util::paths::SortOrder {
    fn from(order: ProjectPanelSortOrder) -> Self {
        match order {
            ProjectPanelSortOrder::Default => Self::Default,
            ProjectPanelSortOrder::Upper => Self::Upper,
            ProjectPanelSortOrder::Lower => Self::Lower,
            ProjectPanelSortOrder::Unicode => Self::Unicode,
        }
    }
}

/// 项目面板滚动条设置
#[with_fallible_options]
#[derive(
    Copy, Clone, Debug, Serialize, Deserialize, JsonSchema, MergeFrom, PartialEq, Eq, Default,
)]
pub struct ProjectPanelScrollbarSettingsContent {
    /// 滚动条显示策略
    /// 默认值：继承编辑器设置
    pub show: Option<ShowScrollbar>,
    /// 是否允许水平滚动
    /// 禁用则长文件名截断，视图锁定左侧
    /// 默认值：true
    pub horizontal_scroll: Option<bool>,
}

/// 项目面板缩进引导线设置
#[with_fallible_options]
#[derive(
    Copy, Clone, Debug, Serialize, Deserialize, JsonSchema, MergeFrom, PartialEq, Eq, Default,
)]
pub struct ProjectPanelIndentGuidesSettings {
    pub show: Option<ShowIndentGuides>,
}

/// 语义化标记使用模式（语法高亮）
#[derive(
    Debug,
    PartialEq,
    Eq,
    Clone,
    Copy,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
    strum::EnumMessage,
)]
#[serde(rename_all = "snake_case")]
pub enum SemanticTokens {
    /// 关闭（不请求 LSP 语义标记）
    #[default]
    Off,
    /// 混合使用：Tree-sitter + LSP 语义标记
    Combined,
    /// 仅使用 LSP 语义标记，禁用 Tree-sitter
    Full,
}

impl SemanticTokens {
    /// 是否启用语义标记
    pub fn enabled(&self) -> bool {
        self != &Self::Off
    }

    /// 是否使用 Tree-sitter 高亮
    pub fn use_tree_sitter(&self) -> bool {
        self != &Self::Full
    }
}

/// 文档折叠范围来源
#[derive(
    Debug,
    PartialEq,
    Eq,
    Clone,
    Copy,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum DocumentFoldingRanges {
    /// 关闭 LSP 折叠，仅使用 Tree-sitter/缩进折叠
    #[default]
    Off,
    /// 优先使用 LSP 折叠，失败回退到默认方案
    On,
}

impl DocumentFoldingRanges {
    pub fn enabled(&self) -> bool {
        self != &Self::Off
    }
}

/// 文档符号来源（大纲/面包屑）
#[derive(
    Debug,
    PartialEq,
    Eq,
    Clone,
    Copy,
    Default,
    Serialize,
    Deserialize,
    JsonSchema,
    MergeFrom,
    strum::VariantArray,
    strum::VariantNames,
)]
#[serde(rename_all = "snake_case")]
pub enum DocumentSymbols {
    /// 使用 Tree-sitter（默认）
    #[default]
    #[serde(alias = "tree_sitter")]
    Off,
    /// 使用 LSP 语言服务器
    #[serde(alias = "language_server")]
    On,
}

impl DocumentSymbols {
    pub fn lsp_enabled(&self) -> bool {
        self == &Self::On
    }
}

/// 鼠标跟随焦点设置
#[with_fallible_options]
#[derive(Copy, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema, MergeFrom, Debug)]
pub struct FocusFollowsMouse {
    /// 是否启用
    pub enabled: Option<bool>,
    /// 防抖延迟（毫秒）
    pub debounce_ms: Option<u64>,
}