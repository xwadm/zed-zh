use fs::Fs;
use fuzzy::{StringMatch, StringMatchCandidate, match_strings};
use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, Focusable, Render, UpdateGlobal, WeakEntity,
    Window,
};
use picker::{Picker, PickerDelegate};
use settings::{Settings as _, SettingsStore, update_settings_file};
use std::sync::Arc;
use theme::{Appearance, SystemAppearance, ThemeMeta, ThemeRegistry};
use theme_settings::{IconThemeName, IconThemeSelection, ThemeSettings};
use ui::{ListItem, ListItemSpacing, prelude::*, v_flex};
use util::ResultExt;
use workspace::{ModalView, ui::HighlightedLabel};
use zed_actions::{ExtensionCategoryFilter, Extensions};

/// 图标主题选择器
pub(crate) struct IconThemeSelector {
    picker: Entity<Picker<IconThemeSelectorDelegate>>,
}

impl EventEmitter<DismissEvent> for IconThemeSelector {}

impl Focusable for IconThemeSelector {
    fn focus_handle(&self, cx: &App) -> gpui::FocusHandle {
        self.picker.focus_handle(cx)
    }
}

impl ModalView for IconThemeSelector {
    /// 关闭前回调：恢复原始主题
    fn on_before_dismiss(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> workspace::DismissDecision {
        self.picker.update(cx, |picker, cx| {
            picker.delegate.revert_theme(cx);
        });
        workspace::DismissDecision::Dismiss(true)
    }
}

impl IconThemeSelector {
    /// 创建图标主题选择器实例
    pub fn new(
        delegate: IconThemeSelectorDelegate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let picker = cx.new(|cx| Picker::uniform_list(delegate, window, cx));
        Self { picker }
    }
}

impl Render for IconThemeSelector {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context("IconThemeSelector")
            .w(rems(34.))
            .child(self.picker.clone())
    }
}

/// 图标主题选择器委托
pub(crate) struct IconThemeSelectorDelegate {
    fs: Arc<dyn Fs>,
    themes: Vec<ThemeMeta>,
    matches: Vec<StringMatch>,
    original_theme: IconThemeName,
    selection_completed: bool,
    selected_theme: Option<IconThemeName>,
    selected_index: usize,
    selector: WeakEntity<IconThemeSelector>,
}

impl IconThemeSelectorDelegate {
    /// 创建委托实例
    pub fn new(
        selector: WeakEntity<IconThemeSelector>,
        fs: Arc<dyn Fs>,
        themes_filter: Option<&Vec<String>>,
        cx: &mut Context<IconThemeSelector>,
    ) -> Self {
        let theme_settings = ThemeSettings::get_global(cx);
        let original_theme = theme_settings
            .icon_theme
            .name(SystemAppearance::global(cx).0);

        let registry = ThemeRegistry::global(cx);
        let mut themes = registry
            .list_icon_themes()
            .into_iter()
            .filter(|meta| {
                if let Some(theme_filter) = themes_filter {
                    theme_filter.contains(&meta.name.to_string())
                } else {
                    true
                }
            })
            .collect::<Vec<_>>();

        // 按外观+名称排序
        themes.sort_unstable_by(|a, b| {
            a.appearance
                .is_light()
                .cmp(&b.appearance.is_light())
                .then(a.name.cmp(&b.name))
        });
        let matches = themes
            .iter()
            .map(|meta| StringMatch {
                candidate_id: 0,
                score: 0.0,
                positions: Default::default(),
                string: meta.name.to_string(),
            })
            .collect();
        let mut this = Self {
            fs,
            themes,
            matches,
            original_theme: original_theme.clone(),
            selected_index: 0,
            selected_theme: None,
            selection_completed: false,
            selector,
        };

        this.select_if_matching(&original_theme.0);
        this
    }

    /// 显示选中的主题并应用预览
    fn show_selected_theme(
        &mut self,
        cx: &mut Context<Picker<IconThemeSelectorDelegate>>,
    ) -> Option<IconThemeName> {
        let mat = self.matches.get(self.selected_index)?;
        let name = IconThemeName(mat.string.clone().into());
        Self::set_icon_theme(name.clone(), cx);
        Some(name)
    }

    /// 匹配主题名称并选中对应项
    fn select_if_matching(&mut self, theme_name: &str) {
        self.selected_index = self
            .matches
            .iter()
            .position(|mat| mat.string == theme_name)
            .unwrap_or(self.selected_index);
    }

    /// 取消选择时恢复原始主题
    fn revert_theme(&mut self, cx: &mut App) {
        if !self.selection_completed {
            Self::set_icon_theme(self.original_theme.clone(), cx);
            self.selection_completed = true;
        }
    }

    /// 设置全局图标主题
    fn set_icon_theme(name: IconThemeName, cx: &mut App) {
        SettingsStore::update_global(cx, |store, _| {
            let mut theme_settings = store.get::<ThemeSettings>(None).clone();
            theme_settings.icon_theme = IconThemeSelection::Static(name);
            store.override_global(theme_settings);
        });
    }
}

impl PickerDelegate for IconThemeSelectorDelegate {
    type ListItem = ui::ListItem;

    /// 选择器占位文本
    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        "选择图标主题...".into()
    }

    /// 匹配结果数量
    fn match_count(&self) -> usize {
        self.matches.len()
    }

    /// 确认选择主题
    fn confirm(
        &mut self,
        _: bool,
        window: &mut Window,
        cx: &mut Context<Picker<IconThemeSelectorDelegate>>,
    ) {
        self.selection_completed = true;

        let theme_settings = ThemeSettings::get_global(cx);
        let theme_name = theme_settings
            .icon_theme
            .name(SystemAppearance::global(cx).0);

        telemetry::event!(
            "Settings Changed",
            setting = "icon_theme",
            value = theme_name
        );

        let appearance = Appearance::from(window.appearance());

        // 更新配置文件
        update_settings_file(self.fs.clone(), cx, move |settings, _| {
            theme_settings::set_icon_theme(settings, theme_name, appearance);
        });

        // 关闭选择器
        self.selector
            .update(cx, |_, cx| {
                cx.emit(DismissEvent);
            })
            .ok();
    }

    /// 取消选择
    fn dismissed(&mut self, _: &mut Window, cx: &mut Context<Picker<IconThemeSelectorDelegate>>) {
        self.revert_theme(cx);

        self.selector
            .update(cx, |_, cx| cx.emit(DismissEvent))
            .log_err();
    }

    /// 获取选中索引
    fn selected_index(&self) -> usize {
        self.selected_index
    }

    /// 设置选中索引并预览主题
    fn set_selected_index(
        &mut self,
        ix: usize,
        _: &mut Window,
        cx: &mut Context<Picker<IconThemeSelectorDelegate>>,
    ) {
        self.selected_index = ix;
        self.selected_theme = self.show_selected_theme(cx);
    }

    /// 根据搜索词更新匹配结果
    fn update_matches(
        &mut self,
        query: String,
        window: &mut Window,
        cx: &mut Context<Picker<IconThemeSelectorDelegate>>,
    ) -> gpui::Task<()> {
        let background = cx.background_executor().clone();
        let candidates = self
            .themes
            .iter()
            .enumerate()
            .map(|(id, meta)| StringMatchCandidate::new(id, &meta.name))
            .collect::<Vec<_>>();

        cx.spawn_in(window, async move |this, cx| {
            let matches = if query.is_empty() {
                candidates
                    .into_iter()
                    .enumerate()
                    .map(|(index, candidate)| StringMatch {
                        candidate_id: index,
                        string: candidate.string,
                        positions: Vec::new(),
                        score: 0.0,
                    })
                    .collect()
            } else {
                match_strings(
                    &candidates,
                    &query,
                    false,
                    true,
                    100,
                    &Default::default(),
                    background,
                )
                .await
            };

            this.update(cx, |this, cx| {
                this.delegate.matches = matches;
                if query.is_empty() && this.delegate.selected_theme.is_none() {
                    this.delegate.selected_index = this
                        .delegate
                        .selected_index
                        .min(this.delegate.matches.len().saturating_sub(1));
                } else if let Some(selected) = this.delegate.selected_theme.as_ref() {
                    this.delegate.selected_index = this
                        .delegate
                        .matches
                        .iter()
                        .enumerate()
                        .find(|(_, mtch)| mtch.string.as_str() == selected.0.as_ref())
                        .map(|(ix, _)| ix)
                        .unwrap_or_default();
                } else {
                    this.delegate.selected_index = 0;
                }
                // 筛选无结果时保留之前选中的主题
                if let Some(theme) = this.delegate.show_selected_theme(cx) {
                    this.delegate.selected_theme = Some(theme);
                }
            })
            .log_err();
        })
    }

    /// 渲染单个匹配项
    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _window: &mut Window,
        _cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let theme_match = &self.matches.get(ix)?;

        Some(
            ListItem::new(ix)
                .inset(true)
                .spacing(ListItemSpacing::Sparse)
                .toggle_state(selected)
                .child(HighlightedLabel::new(
                    theme_match.string.clone(),
                    theme_match.positions.clone(),
                )),
        )
    }

    /// 渲染底部操作栏
    fn render_footer(
        &self,
        _window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<gpui::AnyElement> {
        Some(
            h_flex()
                .p_2()
                .w_full()
                .justify_between()
                .gap_2()
                .border_t_1()
                .border_color(cx.theme().colors().border_variant)
                .child(
                    Button::new("docs", "查看图标主题文档")
                        .end_icon(
                            Icon::new(IconName::ArrowUpRight)
                                .size(IconSize::Small)
                                .color(Color::Muted),
                        )
                        .on_click(|_event, _window, cx| {
                            cx.open_url("https://zed.dev/docs/icon-themes");
                        }),
                )
                .child(
                    Button::new("more-icon-themes", "安装更多图标主题").on_click(
                        move |_event, window, cx| {
                            window.dispatch_action(
                                Box::new(Extensions {
                                    category_filter: Some(ExtensionCategoryFilter::IconThemes),
                                    id: None,
                                }),
                                cx,
                            );
                        },
                    ),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use gpui::{TestAppContext, VisualTestContext};
    use project::Project;
    use serde_json::json;
    use theme::{ChevronIcons, DirectoryIcons, IconTheme, ThemeRegistry};
    use util::path;
    use workspace::MultiWorkspace;

    /// 初始化测试环境
    fn init_test(cx: &mut TestAppContext) -> Arc<workspace::AppState> {
        cx.update(|cx| {
            let app_state = workspace::AppState::test(cx);
            settings::init(cx);
            theme::init(theme::LoadThemes::JustBase, cx);
            editor::init(cx);
            crate::init(cx);
            app_state
        })
    }

    /// 注册测试用图标主题
    fn register_test_icon_themes(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let registry = ThemeRegistry::global(cx);
            let make_icon_theme = |name: &str, appearance: Appearance| IconTheme {
                id: name.to_lowercase().replace(' ', "-"),
                name: SharedString::from(name.to_string()),
                appearance,
                directory_icons: DirectoryIcons {
                    collapsed: None,
                    expanded: None,
                },
                named_directory_icons: HashMap::default(),
                chevron_icons: ChevronIcons {
                    collapsed: None,
                    expanded: None,
                },
                file_icons: HashMap::default(),
                file_stems: HashMap::default(),
                file_suffixes: HashMap::default(),
            };
            registry.register_test_icon_themes([
                make_icon_theme("Test Icons A", Appearance::Dark),
                make_icon_theme("Test Icons B", Appearance::Dark),
            ]);
        });
    }

    /// 测试环境准备
    async fn setup_test(cx: &mut TestAppContext) -> Arc<workspace::AppState> {
        let app_state = init_test(cx);
        register_test_icon_themes(cx);
        app_state
            .fs
            .as_fake()
            .insert_tree(path!("/test"), json!({}))
            .await;
        app_state
    }

    /// 打开图标主题选择器
    fn open_icon_theme_selector(
        workspace: &Entity<workspace::Workspace>,
        cx: &mut VisualTestContext,
    ) -> Entity<Picker<IconThemeSelectorDelegate>> {
        cx.dispatch_action(zed_actions::icon_theme_selector::Toggle {
            themes_filter: None,
        });
        cx.run_until_parked();
        workspace.update(cx, |workspace, cx| {
            workspace
                .active_modal::<IconThemeSelector>(cx)
                .expect("图标主题选择器应已打开")
                .read(cx)
                .picker
                .clone()
        })
    }

    /// 获取选中的主题名称
    fn selected_theme_name(
        picker: &Entity<Picker<IconThemeSelectorDelegate>>,
        cx: &mut VisualTestContext,
    ) -> String {
        picker.read_with(cx, |picker, _| {
            picker
                .delegate
                .matches
                .get(picker.delegate.selected_index)
                .expect("选中索引应指向有效匹配项")
                .string
                .clone()
        })
    }

    /// 获取预览中的主题名称
    fn previewed_theme_name(
        _picker: &Entity<Picker<IconThemeSelectorDelegate>>,
        cx: &mut VisualTestContext,
    ) -> String {
        cx.read(|cx| {
            ThemeSettings::get_global(cx)
                .icon_theme
                .name(SystemAppearance::global(cx).0)
                .0
                .to_string()
        })
    }

    #[gpui::test]
    /// 测试空筛选条件下保留选中的主题
    async fn test_icon_theme_selector_preserves_selection_on_empty_filter(cx: &mut TestAppContext) {
        let app_state = setup_test(cx).await;
        let project = Project::test(app_state.fs.clone(), [path!("/test").as_ref()], cx).await;
        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
        let workspace =
            multi_workspace.read_with(cx, |multi_workspace, _| multi_workspace.workspace().clone());
        let picker = open_icon_theme_selector(&workspace, cx);

        let target_index = picker.read_with(cx, |picker, _| {
            picker
                .delegate
                .matches
                .iter()
                .position(|m| m.string == "Test Icons A")
                .unwrap()
        });
        picker.update_in(cx, |picker, window, cx| {
            picker.set_selected_index(target_index, None, true, window, cx);
        });
        cx.run_until_parked();

        assert_eq!(previewed_theme_name(&picker, cx), "Test Icons A");

        // 应用无结果筛选
        picker.update_in(cx, |picker, window, cx| {
            picker.update_matches("zzz".to_string(), window, cx);
        });
        cx.run_until_parked();

        // 清空筛选条件
        picker.update_in(cx, |picker, window, cx| {
            picker.update_matches("".to_string(), window, cx);
        });
        cx.run_until_parked();

        assert_eq!(
            selected_theme_name(&picker, cx),
            "Test Icons A",
            "清空无结果筛选后应保留选中的图标主题"
        );
        assert_eq!(
            previewed_theme_name(&picker, cx),
            "Test Icons A",
            "清空无结果筛选后应保留预览的图标主题"
        );
    }
}