use bitflags::bitflags;
pub use buffer_search::BufferSearchBar;
pub use editor::HighlightKey;
use editor::SearchSettings;
use gpui::{Action, App, ClickEvent, FocusHandle, IntoElement, actions};
use project::search::SearchQuery;
pub use project_search::ProjectSearchView;
use ui::{ButtonStyle, IconButton, IconButtonShape};
use ui::{Tooltip, prelude::*};
use workspace::notifications::NotificationId;
use workspace::{Toast, Workspace};
pub use zed_actions::search::ToggleIncludeIgnored;

pub use search_status_button::SEARCH_ICON;

use crate::project_search::ProjectSearchBar;

pub mod buffer_search;
pub mod project_search;
pub(crate) mod search_bar;
pub mod search_status_button;

/// 初始化搜索模块
pub fn init(cx: &mut App) {
    menu::init();
    buffer_search::init(cx);
    project_search::init(cx);
}

// 定义搜索相关操作
actions!(
    search,
    [
        /// 聚焦到搜索输入框
        FocusSearch,
        /// 切换全词匹配模式
        ToggleWholeWord,
        /// 切换大小写敏感模式
        ToggleCaseSensitive,
        /// 切换正则表达式模式
        ToggleRegex,
        /// 切换替换界面
        ToggleReplace,
        /// 切换仅在选中区域内搜索
        ToggleSelection,
        /// 选中下一个搜索匹配项
        SelectNextMatch,
        /// 选中上一个搜索匹配项
        SelectPreviousMatch,
        /// 选中所有搜索匹配项
        SelectAllMatches,
        /// 循环切换搜索模式
        CycleMode,
        /// 切换到搜索历史中的下一个查询
        NextHistoryQuery,
        /// 切换到搜索历史中的上一个查询
        PreviousHistoryQuery,
        /// 替换所有匹配项
        ReplaceAll,
        /// 替换下一个匹配项
        ReplaceNext,
    ]
);

// 定义搜索选项标志位
bitflags! {
    #[derive(Debug, PartialEq, Eq, Clone, Copy, Default)]
    pub struct SearchOptions: u8 {
        const NONE = 0;
        const WHOLE_WORD = 1 << SearchOption::WholeWord as u8;
        const CASE_SENSITIVE = 1 << SearchOption::CaseSensitive as u8;
        const INCLUDE_IGNORED = 1 << SearchOption::IncludeIgnored as u8;
        const REGEX = 1 << SearchOption::Regex as u8;
        const ONE_MATCH_PER_LINE = 1 << SearchOption::OneMatchPerLine as u8;
        /// 如果设置，查找活动匹配项时反向搜索
        const BACKWARDS = 1 << SearchOption::Backwards as u8;
    }
}

/// 搜索选项枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SearchOption {
    WholeWord = 0,
    CaseSensitive,
    IncludeIgnored,
    Regex,
    OneMatchPerLine,
    Backwards,
}

/// 搜索来源类型
pub enum SearchSource<'a, 'b> {
    Buffer,
    Project(&'a Context<'b, ProjectSearchBar>),
}

impl SearchOption {
    /// 转换为对应的选项标志
    pub fn as_options(&self) -> SearchOptions {
        SearchOptions::from_bits(1 << *self as u8).unwrap()
    }

    /// 获取选项显示文本
    pub fn label(&self) -> &'static str {
        match self {
            SearchOption::WholeWord => "全词匹配",
            SearchOption::CaseSensitive => "区分大小写",
            SearchOption::IncludeIgnored => "同时搜索配置中忽略的文件",
            SearchOption::Regex => "使用正则表达式",
            SearchOption::OneMatchPerLine => "每行仅匹配一次",
            SearchOption::Backwards => "反向搜索",
        }
    }

    /// 获取选项对应图标
    pub fn icon(&self) -> ui::IconName {
        match self {
            SearchOption::WholeWord => ui::IconName::WholeWord,
            SearchOption::CaseSensitive => ui::IconName::CaseSensitive,
            SearchOption::IncludeIgnored => ui::IconName::Sliders,
            SearchOption::Regex => ui::IconName::Regex,
            _ => panic!("{self:?} 不是命名的搜索选项"),
        }
    }

    /// 获取切换该选项的操作
    pub fn to_toggle_action(self) -> &'static dyn Action {
        match self {
            SearchOption::WholeWord => &ToggleWholeWord,
            SearchOption::CaseSensitive => &ToggleCaseSensitive,
            SearchOption::IncludeIgnored => &ToggleIncludeIgnored,
            SearchOption::Regex => &ToggleRegex,
            _ => panic!("{self:?} 没有对应的切换操作"),
        }
    }

    /// 渲染为搜索选项按钮
    pub fn as_button(
        &self,
        active: SearchOptions,
        search_source: SearchSource,
        focus_handle: FocusHandle,
    ) -> impl IntoElement {
        let action = self.to_toggle_action();
        let label = self.label();
        IconButton::new(
            (label, matches!(search_source, SearchSource::Buffer) as u32),
            self.icon(),
        )
        .map(|button| match search_source {
            SearchSource::Buffer => {
                let focus_handle = focus_handle.clone();
                button.on_click(move |_: &ClickEvent, window, cx| {
                    if !focus_handle.is_focused(window) {
                        window.focus(&focus_handle, cx);
                    }
                    window.dispatch_action(action.boxed_clone(), cx);
                })
            }
            SearchSource::Project(cx) => {
                let options = self.as_options();
                button.on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.toggle_search_option(options, window, cx);
                }))
            }
        })
        .style(ButtonStyle::Subtle)
        .shape(IconButtonShape::Square)
        .toggle_state(active.contains(self.as_options()))
        .tooltip(move |_window, cx| Tooltip::for_action_in(label, action, &focus_handle, cx))
    }
}

impl SearchOptions {
    /// 创建空选项
    pub fn none() -> SearchOptions {
        SearchOptions::NONE
    }

    /// 从搜索查询中提取选项
    pub fn from_query(query: &SearchQuery) -> SearchOptions {
        let mut options = SearchOptions::NONE;
        options.set(SearchOptions::WHOLE_WORD, query.whole_word());
        options.set(SearchOptions::CASE_SENSITIVE, query.case_sensitive());
        options.set(SearchOptions::INCLUDE_IGNORED, query.include_ignored());
        options.set(SearchOptions::REGEX, query.is_regex());
        options
    }

    /// 从搜索设置中提取选项
    pub fn from_settings(settings: &SearchSettings) -> SearchOptions {
        let mut options = SearchOptions::NONE;
        options.set(SearchOptions::WHOLE_WORD, settings.whole_word);
        options.set(SearchOptions::CASE_SENSITIVE, settings.case_sensitive);
        options.set(SearchOptions::INCLUDE_IGNORED, settings.include_ignored);
        options.set(SearchOptions::REGEX, settings.regex);
        options
    }
}

/// 显示无更多匹配项提示
pub(crate) fn show_no_more_matches(window: &mut Window, cx: &mut App) {
    window.defer(cx, |window, cx| {
        struct NotifType();
        let notification_id = NotificationId::unique::<NotifType>();

        let Some(workspace) = Workspace::for_window(window, cx) else {
            return;
        };
        workspace.update(cx, |workspace, cx| {
            workspace.show_toast(
                Toast::new(notification_id.clone(), "没有更多匹配项").autohide(),
                cx,
            );
        })
    });
}