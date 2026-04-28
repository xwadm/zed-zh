mod icon_theme_selector;

use fs::Fs;
use fuzzy::{StringMatch, StringMatchCandidate, match_strings};
use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, Focusable, Render, UpdateGlobal, WeakEntity,
    Window, actions,
};
use picker::{Picker, PickerDelegate};
use settings::{Settings, SettingsStore, update_settings_file};
use std::sync::Arc;
use theme::{Appearance, SystemAppearance, Theme, ThemeMeta, ThemeRegistry};
use theme_settings::{
    ThemeAppearanceMode, ThemeName, ThemeSelection, ThemeSettings, appearance_to_mode,
};
use ui::{ListItem, ListItemSpacing, prelude::*, v_flex};
use util::ResultExt;
use workspace::{ModalView, Workspace, ui::HighlightedLabel, with_active_or_new_workspace};
use zed_actions::{ExtensionCategoryFilter, Extensions};

use crate::icon_theme_selector::{IconThemeSelector, IconThemeSelectorDelegate};

// 注册动作：重新加载主题
actions!(
    theme_selector,
    [
        /// 从磁盘重新加载所有主题
        Reload
    ]
);

/// 初始化主题选择器模块
pub fn init(cx: &mut App) {
    // 监听主题选择器切换动作
    cx.on_action(|action: &zed_actions::theme_selector::Toggle, cx| {
        let action = action.clone();
        with_active_or_new_workspace(cx, move |workspace, window, cx| {
            toggle_theme_selector(workspace, &action, window, cx);
        });
    });
    // 监听图标主题选择器切换动作
    cx.on_action(|action: &zed_actions::icon_theme_selector::Toggle, cx| {
        let action = action.clone();
        with_active_or_new_workspace(cx, move |workspace, window, cx| {
            toggle_icon_theme_selector(workspace, &action, window, cx);
        });
    });
}

/// 切换主题选择器弹窗显示/隐藏
fn toggle_theme_selector(
    workspace: &mut Workspace,
    toggle: &zed_actions::theme_selector::Toggle,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let fs = workspace.app_state().fs.clone();
    workspace.toggle_modal(window, cx, |window, cx| {
        let delegate = ThemeSelectorDelegate::new(
            cx.entity().downgrade(),
            fs,
            toggle.themes_filter.as_ref(),
            cx,
        );
        ThemeSelector::new(delegate, window, cx)
    });
}

/// 切换图标主题选择器弹窗显示/隐藏
fn toggle_icon_theme_selector(
    workspace: &mut Workspace,
    toggle: &zed_actions::icon_theme_selector::Toggle,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let fs = workspace.app_state().fs.clone();
    workspace.toggle_modal(window, cx, |window, cx| {
        let delegate = IconThemeSelectorDelegate::new(
            cx.entity().downgrade(),
            fs,
            toggle.themes_filter.as_ref(),
            cx,
        );
        IconThemeSelector::new(delegate, window, cx)
    });
}

/// 实现模态窗口 trait
impl ModalView for ThemeSelector {
    fn on_before_dismiss(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> workspace::DismissDecision {
        // 关闭前恢复原始主题
        self.picker.update(cx, |picker, cx| {
            picker.delegate.revert_theme(cx);
        });
        workspace::DismissDecision::Dismiss(true)
    }
}

/// 主题选择器主结构体
struct ThemeSelector {
    picker: Entity<Picker<ThemeSelectorDelegate>>,
}

impl EventEmitter<DismissEvent> for ThemeSelector {}

impl Focusable for ThemeSelector {
    fn focus_handle(&self, cx: &App) -> gpui::FocusHandle {
        self.picker.focus_handle(cx)
    }
}

impl Render for ThemeSelector {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context("ThemeSelector")
            .w(rems(34.))
            .child(self.picker.clone())
    }
}

impl ThemeSelector {
    /// 创建主题选择器实例
    pub fn new(
        delegate: ThemeSelectorDelegate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let picker = cx.new(|cx| Picker::uniform_list(delegate, window, cx));
        Self { picker }
    }
}

/// 主题选择器委托：处理主题列表、筛选、预览、确认逻辑
struct ThemeSelectorDelegate {
    fs: Arc<dyn Fs>,
    themes: Vec<ThemeMeta>,
    matches: Vec<StringMatch>,
    /// 打开选择器前的原始主题设置（取消时恢复）
    original_theme_settings: ThemeSettings,
    /// 原始系统外观模式
    original_system_appearance: Appearance,
    /// 当前预览的新主题
    new_theme: Arc<Theme>,
    selection_completed: bool,
    selected_theme: Option<Arc<Theme>>,
    selected_index: usize,
    selector: WeakEntity<ThemeSelector>,
}

impl ThemeSelectorDelegate {
    /// 创建委托实例
    fn new(
        selector: WeakEntity<ThemeSelector>,
        fs: Arc<dyn Fs>,
        themes_filter: Option<&Vec<String>>,
        cx: &mut Context<ThemeSelector>,
    ) -> Self {
        let original_theme = cx.theme().clone();
        let original_theme_settings = ThemeSettings::get_global(cx).clone();
        let original_system_appearance = SystemAppearance::global(cx).0;

        let registry = ThemeRegistry::global(cx);
        let mut themes = registry
            .list()
            .into_iter()
            .filter(|meta| {
                if let Some(theme_filter) = themes_filter {
                    theme_filter.contains(&meta.name.to_string())
                } else {
                    true
                }
            })
            .collect::<Vec<_>>();

        // 排序规则：先按明暗主题，再按名称排序
        themes.sort_unstable_by(|a, b| {
            a.appearance
                .is_light()
                .cmp(&b.appearance.is_light())
                .then(a.name.cmp(&b.name))
        });

        let matches: Vec<StringMatch> = themes
            .iter()
            .map(|meta| StringMatch {
                candidate_id: 0,
                score: 0.0,
                positions: Default::default(),
                string: meta.name.to_string(),
            })
            .collect();

        // 默认选中当前正在使用的主题
        let selected_index = matches
            .iter()
            .position(|mat| mat.string == original_theme.name)
            .unwrap_or(0);

        Self {
            fs,
            themes,
            matches,
            original_theme_settings,
            original_system_appearance,
            new_theme: original_theme,
            selected_index,
            selection_completed: false,
            selected_theme: None,
            selector,
        }
    }

    /// 显示选中的主题并实时预览
    fn show_selected_theme(
        &mut self,
        cx: &mut Context<Picker<ThemeSelectorDelegate>>,
    ) -> Option<Arc<Theme>> {
        if let Some(mat) = self.matches.get(self.selected_index) {
            let registry = ThemeRegistry::global(cx);

            match registry.get(&mat.string) {
                Ok(theme) => {
                    self.set_theme(theme.clone(), cx);
                    Some(theme)
                }
                Err(error) => {
                    log::error!("加载主题失败 {}: {}", mat.string, error);
                    None
                }
            }
        } else {
            None
        }
    }

    /// 恢复到原始主题（用户取消选择时调用）
    fn revert_theme(&mut self, cx: &mut App) {
        if !self.selection_completed {
            SettingsStore::update_global(cx, |store, _| {
                store.override_global(self.original_theme_settings.clone());
            });
            self.selection_completed = true;
        }
    }

    /// 设置当前预览主题
    fn set_theme(&mut self, new_theme: Arc<Theme>, cx: &mut App) {
        // 更新内存中的全局主题设置（不写入配置文件）
        SettingsStore::update_global(cx, |store, _| {
            override_global_theme(
                store,
                &new_theme,
                &self.original_theme_settings.theme,
                self.original_system_appearance,
            )
        });

        self.new_theme = new_theme;
    }
}

/// 覆盖全局主题设置（仅内存，不写入配置文件）
fn override_global_theme(
    store: &mut SettingsStore,
    new_theme: &Theme,
    original_theme: &ThemeSelection,
    system_appearance: Appearance,
) {
    let theme_name = ThemeName(new_theme.name.clone().into());
    let new_appearance = new_theme.appearance();
    let new_theme_is_light = new_appearance.is_light();

    let mut curr_theme_settings = store.get::<ThemeSettings>(None).clone();

    match (original_theme, &curr_theme_settings.theme) {
        // 覆盖静态选中的主题
        (ThemeSelection::Static(_), ThemeSelection::Static(_)) => {
            curr_theme_settings.theme = ThemeSelection::Static(theme_name);
        }

        // 动态主题模式：仅覆盖对应明暗模式的主题
        (
            ThemeSelection::Dynamic {
                mode: original_mode,
                light: original_light,
                dark: original_dark,
            },
            ThemeSelection::Dynamic { .. },
        ) => {
            let new_mode = update_mode_if_new_appearance_is_different_from_system(
                original_mode,
                system_appearance,
                new_appearance,
            );

            let updated_theme = retain_original_opposing_theme(
                new_theme_is_light,
                new_mode,
                theme_name,
                original_light,
                original_dark,
            );

            curr_theme_settings.theme = updated_theme;
        }

        // 配置文件被外部修改，不做处理
        _ => return,
    };

    store.override_global(curr_theme_settings);
}

/// 计算新的主题外观模式
fn update_mode_if_new_appearance_is_different_from_system(
    original_mode: &ThemeAppearanceMode,
    system_appearance: Appearance,
    new_appearance: Appearance,
) -> ThemeAppearanceMode {
    if original_mode == &ThemeAppearanceMode::System && system_appearance == new_appearance {
        ThemeAppearanceMode::System
    } else {
        appearance_to_mode(new_appearance)
    }
}

/// 保留原始的对立主题设置（亮/暗模式互补）
fn retain_original_opposing_theme(
    new_theme_is_light: bool,
    new_mode: ThemeAppearanceMode,
    theme_name: ThemeName,
    original_light: &ThemeName,
    original_dark: &ThemeName,
) -> ThemeSelection {
    if new_theme_is_light {
        ThemeSelection::Dynamic {
            mode: new_mode,
            light: theme_name,
            dark: original_dark.clone(),
        }
    } else {
        ThemeSelection::Dynamic {
            mode: new_mode,
            light: original_light.clone(),
            dark: theme_name,
        }
    }
}

/// 实现选择器委托 trait
impl PickerDelegate for ThemeSelectorDelegate {
    type ListItem = ui::ListItem;

    /// 搜索框占位文本
    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        "选择主题...".into()
    }

    /// 匹配结果数量
    fn match_count(&self) -> usize {
        self.matches.len()
    }

    /// 确认选择主题
    fn confirm(
        &mut self,
        _secondary: bool,
        _window: &mut Window,
        cx: &mut Context<Picker<ThemeSelectorDelegate>>,
    ) {
        self.selection_completed = true;

        let theme_name: Arc<str> = self.new_theme.name.as_str().into();
        let theme_appearance = self.new_theme.appearance;
        let system_appearance = SystemAppearance::global(cx).0;

        telemetry::event!("Settings Changed", setting = "theme", value = theme_name);

        // 写入配置文件，永久保存主题设置
        update_settings_file(self.fs.clone(), cx, move |settings, _| {
            theme_settings::set_theme(settings, theme_name, theme_appearance, system_appearance);
        });

        // 关闭选择器
        self.selector
            .update(cx, |_, cx| {
                cx.emit(DismissEvent);
            })
            .ok();
    }

    /// 取消选择，恢复原始主题
    fn dismissed(&mut self, _: &mut Window, cx: &mut Context<Picker<ThemeSelectorDelegate>>) {
        self.revert_theme(cx);

        self.selector
            .update(cx, |_, cx| cx.emit(DismissEvent))
            .log_err();
    }

    /// 当前选中索引
    fn selected_index(&self) -> usize {
        self.selected_index
    }

    /// 设置选中索引并预览主题
    fn set_selected_index(
        &mut self,
        ix: usize,
        _: &mut Window,
        cx: &mut Context<Picker<ThemeSelectorDelegate>>,
    ) {
        self.selected_index = ix;
        self.selected_theme = self.show_selected_theme(cx);
    }

    /// 根据搜索词更新匹配结果
    fn update_matches(
        &mut self,
        query: String,
        window: &mut Window,
        cx: &mut Context<Picker<ThemeSelectorDelegate>>,
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
                        .find(|(_, mtch)| mtch.string == selected.name)
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

    /// 渲染单个主题选项
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
        _: &mut Window,
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
                    Button::new("docs", "查看主题文档")
                        .end_icon(
                            Icon::new(IconName::ArrowUpRight)
                                .size(IconSize::Small)
                                .color(Color::Muted),
                        )
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.open_url("https://zed.dev/docs/themes");
                        })),
                )
                .child(
                    Button::new("more-themes", "安装更多主题").on_click(cx.listener({
                        move |_, _, window, cx| {
                            window.dispatch_action(
                                Box::new(Extensions {
                                    category_filter: Some(ExtensionCategoryFilter::Themes),
                                    id: None,
                                }),
                                cx,
                            );
                        }
                    })),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};
    use project::Project;
    use serde_json::json;
    use theme::{Appearance, ThemeFamily, ThemeRegistry, default_color_scales};
    use util::path;
    use workspace::MultiWorkspace;

    /// 初始化测试环境
    fn init_test(cx: &mut TestAppContext) -> Arc<workspace::AppState> {
        cx.update(|cx| {
            let app_state = workspace::AppState::test(cx);
            settings::init(cx);
            theme::init(theme::LoadThemes::JustBase, cx);
            editor::init(cx);
            super::init(cx);
            app_state
        })
    }

    /// 注册测试用主题
    fn register_test_themes(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let registry = ThemeRegistry::global(cx);
            let base_theme = registry.get("One Dark").unwrap();

            let mut test_light = (*base_theme).clone();
            test_light.id = "test-light".to_string();
            test_light.name = "Test Light".into();
            test_light.appearance = Appearance::Light;

            let mut test_dark_a = (*base_theme).clone();
            test_dark_a.id = "test-dark-a".to_string();
            test_dark_a.name = "Test Dark A".into();

            let mut test_dark_b = (*base_theme).clone();
            test_dark_b.id = "test-dark-b".to_string();
            test_dark_b.name = "Test Dark B".into();

            registry.register_test_themes([ThemeFamily {
                id: "test-family".to_string(),
                name: "Test Family".into(),
                author: "test".into(),
                themes: vec![test_light, test_dark_a, test_dark_b],
                scales: default_color_scales(),
            }]);
        })
    }

    /// 构建完整测试环境
    async fn setup_test(cx: &mut TestAppContext) -> Arc<workspace::AppState> {
        let app_state = init_test(cx);
        register_test_themes(cx);
        app_state
            .fs
            .as_fake()
            .insert_tree(path!("/test"), json!({}))
            .await;
        app_state
    }

    /// 打开主题选择器
    fn open_theme_selector(
        workspace: &Entity<workspace::Workspace>,
        cx: &mut VisualTestContext,
    ) -> Entity<Picker<ThemeSelectorDelegate>> {
        cx.dispatch_action(zed_actions::theme_selector::Toggle {
            themes_filter: None,
        });
        cx.run_until_parked();
        workspace.update(cx, |workspace, cx| {
            workspace
                .active_modal::<ThemeSelector>(cx)
                .expect("主题选择器应已打开")
                .read(cx)
                .picker
                .clone()
        })
    }

    /// 获取当前选中的主题名称
    fn selected_theme_name(
        picker: &Entity<Picker<ThemeSelectorDelegate>>,
        cx: &mut VisualTestContext,
    ) -> String {
        picker.read_with(cx, |picker, _| {
            picker
                .delegate
                .matches
                .get(picker.delegate.selected_index)
                .expect("选中索引应指向有效结果")
                .string
                .clone()
        })
    }

    /// 获取当前预览的主题名称
    fn previewed_theme_name(
        picker: &Entity<Picker<ThemeSelectorDelegate>>,
        cx: &mut VisualTestContext,
    ) -> String {
        picker.read_with(cx, |picker, _| picker.delegate.new_theme.name.to_string())
    }

    /// 测试：清空筛选后保留选中状态
    #[gpui::test]
    async fn test_theme_selector_preserves_selection_on_empty_filter(cx: &mut TestAppContext) {
        let app_state = setup_test(cx).await;
        let project = Project::test(app_state.fs.clone(), [path!("/test").as_ref()], cx).await;
        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
        let workspace =
            multi_workspace.read_with(cx, |multi_workspace, _| multi_workspace.workspace().clone());
        let picker = open_theme_selector(&workspace, cx);

        let target_index = picker.read_with(cx, |picker, _| {
            picker
                .delegate
                .matches
                .iter()
                .position(|m| m.string == "Test Light")
                .unwrap()
        });
        picker.update_in(cx, |picker, window, cx| {
            picker.set_selected_index(target_index, None, true, window, cx);
        });
        cx.run_until_parked();

        assert_eq!(previewed_theme_name(&picker, cx), "Test Light");

        // 输入无效筛选词
        picker.update_in(cx, |picker, window, cx| {
            picker.update_matches("zzz".to_string(), window, cx);
        });
        cx.run_until_parked();

        // 清空筛选
        picker.update_in(cx, |picker, window, cx| {
            picker.update_matches("".to_string(), window, cx);
        });
        cx.run_until_parked();

        assert_eq!(
            selected_theme_name(&picker, cx),
            "Test Light",
            "清空无效筛选后应保留选中主题"
        );
        assert_eq!(
            previewed_theme_name(&picker, cx),
            "Test Light",
            "清空无效筛选后应保留预览主题"
        );
    }
}