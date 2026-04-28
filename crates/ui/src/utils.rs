//! UI 相关工具函数

use gpui::App;
use theme::ActiveTheme;

mod apca_contrast;
mod color_contrast;
mod constants;
mod corner_solver;
mod format_distance;
mod search_input;
mod with_rem_size;

pub use apca_contrast::*;
pub use color_contrast::*;
pub use constants::*;
pub use corner_solver::{CornerSolver, inner_corner_radius};
pub use format_distance::*;
pub use search_input::*;
pub use with_rem_size::*;

/// 判断当前主题是否为浅色或活力浅色主题
pub fn is_light(cx: &mut App) -> bool {
    cx.theme().appearance.is_light()
}

/// 根据系统平台返回「在文件管理器中显示」的本地化文本
pub fn reveal_in_file_manager_label(is_remote: bool) -> &'static str {
    if cfg!(target_os = "macos") && !is_remote {
        "在访达中显示"
    } else if cfg!(target_os = "windows") && !is_remote {
        "在文件资源管理器中显示"
    } else {
        "在文件管理器中显示"
    }
}

/// 将字符串的首字母大写
///
/// 接收字符串切片，返回首字母大写的新字符串
///
/// # 示例
///
/// ```
/// use ui::utils::capitalize;
///
/// assert_eq!(capitalize("hello"), "Hello");
/// assert_eq!(capitalize("WORLD"), "WORLD");
/// assert_eq!(capitalize(""), "");
/// ```
pub fn capitalize(str: &str) -> String {
    let mut chars = str.chars();
    match chars.next() {
        None => String::new(),
        Some(first_char) => first_char.to_uppercase().collect::<String>() + chars.as_str(),
    }
}