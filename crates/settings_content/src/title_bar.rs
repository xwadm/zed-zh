use gpui::WindowButtonLayout;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use settings_macros::{MergeFrom, with_fallible_options};

/// 用户设置中定义的窗口控制按钮布局格式
///
/// 自定义布局字符串遵循 GNOME `button-layout` 格式（例如
/// `"close:minimize,maximize"`）。
#[derive(
    Clone,
    PartialEq,
    Debug,
    Serialize,
    Deserialize,
    JsonSchema,
    MergeFrom,
    Default,
    strum::EnumDiscriminants,
)]
#[strum_discriminants(derive(strum::VariantArray, strum::VariantNames, strum::FromRepr))]
#[schemars(schema_with = "window_button_layout_schema")]
#[serde(from = "String", into = "String")]
pub enum WindowButtonLayoutContent {
    /// 跟随系统/桌面环境的默认配置
    #[default]
    PlatformDefault,
    /// 使用 Zed 内置的标准布局，忽略系统配置
    Standard,
    /// 原生 GNOME 风格的自定义布局字符串
    Custom(String),
}

impl WindowButtonLayoutContent {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    pub fn into_layout(self) -> Option<WindowButtonLayout> {
        use util::ResultExt;

        match self {
            Self::PlatformDefault => None,
            Self::Standard => Some(WindowButtonLayout::linux_default()),
            Self::Custom(layout) => WindowButtonLayout::parse(&layout).log_err(),
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    pub fn into_layout(self) -> Option<WindowButtonLayout> {
        None
    }
}

fn window_button_layout_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "anyOf": [
            { "enum": ["platform_default", "standard"] },
            { "type": "string" }
        ]
    })
}

impl From<WindowButtonLayoutContent> for String {
    fn from(value: WindowButtonLayoutContent) -> Self {
        match value {
            WindowButtonLayoutContent::PlatformDefault => "platform_default".to_string(),
            WindowButtonLayoutContent::Standard => "standard".to_string(),
            WindowButtonLayoutContent::Custom(s) => s,
        }
    }
}

impl From<String> for WindowButtonLayoutContent {
    fn from(layout_string: String) -> Self {
        match layout_string.as_str() {
            "platform_default" => Self::PlatformDefault,
            "standard" => Self::Standard,
            _ => Self::Custom(layout_string),
        }
    }
}

#[with_fallible_options]
#[derive(Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema, MergeFrom, Debug)]
pub struct TitleBarSettingsContent {
    /// 是否在标题栏的分支图标上显示 Git 状态指示器
    /// 启用后，分支图标会根据当前仓库状态自动变化
    ///（例如：文件已修改、已添加、已删除或存在冲突）
    ///
    /// 默认值：false
    pub show_branch_status_icon: Option<bool>,
    /// 是否在标题栏显示新手引导横幅
    ///
    /// 默认值：true
    pub show_onboarding_banner: Option<bool>,
    /// 是否在标题栏显示用户头像
    ///
    /// 默认值：true
    pub show_user_picture: Option<bool>,
    /// 是否在标题栏显示分支名称按钮
    ///
    /// 默认值：true
    pub show_branch_name: Option<bool>,
    /// 是否在标题栏显示项目主机名和项目名称
    ///
    /// 默认值：true
    pub show_project_items: Option<bool>,
    /// 是否在标题栏显示登录按钮
    ///
    /// 默认值：true
    pub show_sign_in: Option<bool>,
    /// 是否在标题栏显示用户菜单按钮
    ///
    /// 默认值：true
    pub show_user_menu: Option<bool>,
    /// 是否在标题栏显示应用菜单
    ///
    /// 默认值：false
    pub show_menus: Option<bool>,
    /// 标题栏中窗口控制按钮的布局（仅 Linux 系统生效）
    ///
    /// 可设置为 "platform_default" 跟随系统配置，
    /// 或 "standard" 使用 Zed 内置布局。
    /// 如需自定义布局，可使用 GNOME 格式字符串，例如 "close:minimize,maximize"
    ///
    /// 默认值："platform_default"
    pub button_layout: Option<WindowButtonLayoutContent>,
}