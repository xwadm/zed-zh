mod active_buffer_language;

pub use active_buffer_language::ActiveBufferLanguage;
use anyhow::Context as _;
use editor::Editor;
use fuzzy::{StringMatch, StringMatchCandidate, match_strings};
use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, ParentElement,
    Render, Styled, WeakEntity, Window, actions,
};
use language::{Buffer, LanguageMatcher, LanguageName, LanguageRegistry};
use open_path_prompt::file_finder_settings::FileFinderSettings;
use picker::{Picker, PickerDelegate};
use project::Project;
use settings::Settings;
use std::{ops::Not as _, path::Path, sync::Arc};
use ui::{HighlightedLabel, ListItem, ListItemSpacing, prelude::*};
use util::ResultExt;
use workspace::{ModalView, Workspace};

// 定义语言选择器操作
actions!(
    language_selector,
    [
        /// 切换语言选择器弹窗
        Toggle
    ]
);

/// 初始化语言选择器
pub fn init(cx: &mut App) {
    cx.observe_new(LanguageSelector::register).detach();
}

/// 语言选择器主组件
pub struct LanguageSelector {
    picker: Entity<Picker<LanguageSelectorDelegate>>,
}

impl LanguageSelector {
    /// 注册语言选择器到工作区
    fn register(
        workspace: &mut Workspace,
        _window: Option<&mut Window>,
        _: &mut Context<Workspace>,
    ) {
        workspace.register_action(move |workspace, _: &Toggle, window, cx| {
            Self::toggle(workspace, window, cx);
        });
    }

    /// 切换语言选择器弹窗显示/隐藏
    fn toggle(
        workspace: &mut Workspace,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Option<()> {
        let registry = workspace.app_state().languages.clone();
        let buffer = workspace
            .active_item(cx)?
            .act_as::<Editor>(cx)?
            .read(cx)
            .active_buffer(cx)?;
        let project = workspace.project().clone();

        workspace.toggle_modal(window, cx, move |window, cx| {
            LanguageSelector::new(buffer, project, registry, window, cx)
        });
        Some(())
    }

    /// 创建新的语言选择器实例
    fn new(
        buffer: Entity<Buffer>,
        project: Entity<Project>,
        language_registry: Arc<LanguageRegistry>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let current_language_name = buffer
            .read(cx)
            .language()
            .map(|language| language.name().as_ref().to_string());
        let delegate = LanguageSelectorDelegate::new(
            cx.entity().downgrade(),
            buffer,
            project,
            language_registry,
            current_language_name,
        );

        let picker = cx.new(|cx| Picker::uniform_list(delegate, window, cx));
        Self { picker }
    }
}

impl Render for LanguageSelector {
    /// 渲染语言选择器
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context("LanguageSelector")
            .w(rems(34.))
            .child(self.picker.clone())
    }
}

impl Focusable for LanguageSelector {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.picker.focus_handle(cx)
    }
}

impl EventEmitter<DismissEvent> for LanguageSelector {}
impl ModalView for LanguageSelector {}

/// 语言选择器代理，处理选择逻辑
pub struct LanguageSelectorDelegate {
    language_selector: WeakEntity<LanguageSelector>,
    buffer: Entity<Buffer>,
    project: Entity<Project>,
    language_registry: Arc<LanguageRegistry>,
    candidates: Vec<StringMatchCandidate>,
    matches: Vec<StringMatch>,
    selected_index: usize,
    current_language_candidate_index: Option<usize>,
}

impl LanguageSelectorDelegate {
    /// 创建语言选择器代理实例
    fn new(
        language_selector: WeakEntity<LanguageSelector>,
        buffer: Entity<Buffer>,
        project: Entity<Project>,
        language_registry: Arc<LanguageRegistry>,
        current_language_name: Option<String>,
    ) -> Self {
        let candidates = language_registry
            .language_names()
            .into_iter()
            .filter_map(|name| {
                language_registry
                    .available_language_for_name(name.as_ref())?
                    .hidden()
                    .not()
                    .then_some(name)
            })
            .enumerate()
            .map(|(candidate_id, name)| StringMatchCandidate::new(candidate_id, name.as_ref()))
            .collect::<Vec<_>>();

        let current_language_candidate_index = current_language_name.as_ref().and_then(|name| {
            candidates
                .iter()
                .position(|candidate| candidate.string == *name)
        });

        Self {
            language_selector,
            buffer,
            project,
            language_registry,
            candidates,
            matches: vec![],
            selected_index: current_language_candidate_index.unwrap_or(0),
            current_language_candidate_index,
        }
    }

    /// 获取匹配项的语言数据（名称和图标）
    fn language_data_for_match(&self, mat: &StringMatch, cx: &App) -> (String, Option<Icon>) {
        let mut label = mat.string.clone();
        let buffer_language = self.buffer.read(cx).language();
        let need_icon = FileFinderSettings::get_global(cx).file_icons;

        if let Some(buffer_language) = buffer_language
            .filter(|buffer_language| buffer_language.name().as_ref() == mat.string.as_str())
        {
            label.push_str(" (当前)");
            let icon = need_icon
                .then(|| self.language_icon(&buffer_language.config().matcher, cx))
                .flatten();
            (label, icon)
        } else {
            let icon = need_icon
                .then(|| {
                    let language_name = LanguageName::new(mat.string.as_str());
                    self.language_registry
                        .available_language_for_name(language_name.as_ref())
                        .and_then(|available_language| {
                            self.language_icon(available_language.matcher(), cx)
                        })
                })
                .flatten();
            (label, icon)
        }
    }

    /// 获取语言对应的文件图标
    fn language_icon(&self, matcher: &LanguageMatcher, cx: &App) -> Option<Icon> {
        matcher
            .path_suffixes
            .iter()
            .find_map(|extension| file_icons::FileIcons::get_icon(Path::new(extension), cx))
            .map(Icon::from_path)
            .map(|icon| icon.color(Color::Muted))
    }
}

impl PickerDelegate for LanguageSelectorDelegate {
    type ListItem = ListItem;

    /// 选择器占位文本
    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        "选择语言…".into()
    }

    /// 匹配结果数量
    fn match_count(&self) -> usize {
        self.matches.len()
    }

    /// 确认选择语言
    fn confirm(&mut self, _: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        if let Some(mat) = self.matches.get(self.selected_index) {
            let language_name = &self.candidates[mat.candidate_id].string;
            let language = self.language_registry.language_for_name(language_name);
            let project = self.project.downgrade();
            let buffer = self.buffer.downgrade();
            cx.spawn_in(window, async move |_, cx| {
                let language = language.await?;
                let project = project.upgrade().context("项目已释放")?;
                let buffer = buffer.upgrade().context("缓冲区已释放")?;
                project.update(cx, |project, cx| {
                    project.set_language_for_buffer(&buffer, language, cx);
                });
                anyhow::Ok(())
            })
            .detach_and_log_err(cx);
        }
        self.dismissed(window, cx);
    }

    /// 关闭选择器
    fn dismissed(&mut self, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        self.language_selector
            .update(cx, |_, cx| cx.emit(DismissEvent))
            .log_err();
    }

    /// 获取选中项索引
    fn selected_index(&self) -> usize {
        self.selected_index
    }

    /// 设置选中项索引
    fn set_selected_index(
        &mut self,
        ix: usize,
        _window: &mut Window,
        _: &mut Context<Picker<Self>>,
    ) {
        self.selected_index = ix;
    }

    /// 根据查询更新匹配结果
    fn update_matches(
        &mut self,
        query: String,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> gpui::Task<()> {
        let background = cx.background_executor().clone();
        let candidates = self.candidates.clone();
        let query_is_empty = query.is_empty();
        cx.spawn_in(window, async move |this, cx| {
            let matches = if query_is_empty {
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

            this.update_in(cx, |this, window, cx| {
                if matches.is_empty() {
                    this.delegate.matches = matches;
                    this.delegate.selected_index = 0;
                    cx.notify();
                    return;
                }

                let selected_index = if query_is_empty {
                    this.delegate
                        .current_language_candidate_index
                        .and_then(|current_language_candidate_index| {
                            matches.iter().position(|mat| {
                                mat.candidate_id == current_language_candidate_index
                            })
                        })
                        .unwrap_or(0)
                } else {
                    0
                };

                this.delegate.matches = matches;
                this.set_selected_index(selected_index, None, false, window, cx);
                cx.notify();
            })
            .log_err();
        })
    }

    /// 渲染单个匹配项
    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let mat = &self.matches.get(ix)?;
        let (label, language_icon) = self.language_data_for_match(mat, cx);
        Some(
            ListItem::new(ix)
                .inset(true)
                .spacing(ListItemSpacing::Sparse)
                .toggle_state(selected)
                .start_slot::<Icon>(language_icon)
                .child(HighlightedLabel::new(label, mat.positions.clone())),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor::Editor;
    use gpui::{TestAppContext, VisualTestContext};
    use language::{Language, LanguageConfig};
    use project::{Project, ProjectPath};
    use serde_json::json;
    use std::sync::Arc;
    use util::{path, rel_path::rel_path};
    use workspace::{AppState, MultiWorkspace, Workspace};

    /// 初始化测试环境
    fn init_test(cx: &mut TestAppContext) -> Arc<AppState> {
        cx.update(|cx| {
            let app_state = AppState::test(cx);
            settings::init(cx);
            super::init(cx);
            editor::init(cx);
            app_state
        })
    }

    /// 注册测试用编程语言
    fn register_test_languages(project: &Entity<Project>, cx: &mut VisualTestContext) {
        project.read_with(cx, |project, _| {
            let language_registry = project.languages();
            for (language_name, path_suffix) in [
                ("C", "c"),
                ("Go", "go"),
                ("Ruby", "rb"),
                ("Rust", "rs"),
                ("TypeScript", "ts"),
            ] {
                language_registry.add(Arc::new(Language::new(
                    LanguageConfig {
                        name: language_name.into(),
                        matcher: LanguageMatcher {
                            path_suffixes: vec![path_suffix.to_string()],
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    None,
                )));
            }
        });
    }

    /// 打开文件编辑器
    async fn open_file_editor(
        workspace: &Entity<Workspace>,
        project: &Entity<Project>,
        file_path: &str,
        cx: &mut VisualTestContext,
    ) -> Entity<Editor> {
        let worktree_id = project.update(cx, |project, cx| {
            project
                .worktrees(cx)
                .next()
                .expect("项目应包含工作树")
                .read(cx)
                .id()
        });
        let project_path = ProjectPath {
            worktree_id,
            path: rel_path(file_path).into(),
        };
        let opened_item = workspace
            .update_in(cx, |workspace, window, cx| {
                workspace.open_path(project_path, None, true, window, cx)
            })
            .await
            .expect("文件应可打开");

        cx.update(|_, cx| {
            opened_item
                .act_as::<Editor>(cx)
                .expect("打开的项应为编辑器")
        })
    }

    /// 打开空白编辑器
    async fn open_empty_editor(
        workspace: &Entity<Workspace>,
        project: &Entity<Project>,
        cx: &mut VisualTestContext,
    ) -> Entity<Editor> {
        let editor = open_new_buffer_editor(workspace, project, cx).await;
        // 确保编辑器创建后缓冲区无语言设置
        let buffer = editor.read_with(cx, |editor, cx| {
            editor
                .active_buffer(cx)
                .expect("编辑器应包含活动缓冲区")
        });
        buffer.update(cx, |buffer, cx| {
            buffer.set_language(None, cx);
        });
        editor
    }

    /// 打开新缓冲区编辑器
    async fn open_new_buffer_editor(
        workspace: &Entity<Workspace>,
        project: &Entity<Project>,
        cx: &mut VisualTestContext,
    ) -> Entity<Editor> {
        let create_buffer = project.update(cx, |project, cx| project.create_buffer(None, true, cx));
        let buffer = create_buffer.await.expect("应创建空白缓冲区");
        let editor = cx.new_window_entity(|window, cx| {
            Editor::for_buffer(buffer.clone(), Some(project.clone()), window, cx)
        });
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.add_item_to_center(Box::new(editor.clone()), window, cx);
        });
        editor
    }

    /// 设置编辑器语言
    async fn set_editor_language(
        project: &Entity<Project>,
        editor: &Entity<Editor>,
        language_name: &str,
        cx: &mut VisualTestContext,
    ) {
        let language = project
            .read_with(cx, |project, _| {
                project.languages().language_for_name(language_name)
            })
            .await
            .expect("语言应存在于注册表中");
        editor.update(cx, move |editor, cx| {
            let buffer = editor
                .active_buffer(cx)
                .expect("编辑器应包含活动片段");
            buffer.update(cx, |buffer, cx| {
                buffer.set_language(Some(language), cx);
            });
        });
    }

    /// 获取当前激活的选择器
    fn active_picker(
        workspace: &Entity<Workspace>,
        cx: &mut VisualTestContext,
    ) -> Entity<Picker<LanguageSelectorDelegate>> {
        workspace.update(cx, |workspace, cx| {
            workspace
                .active_modal::<LanguageSelector>(cx)
                .expect("语言选择器应已打开")
                .read(cx)
                .picker
                .clone()
        })
    }

    /// 打开语言选择器
    fn open_selector(
        workspace: &Entity<Workspace>,
        cx: &mut VisualTestContext,
    ) -> Entity<Picker<LanguageSelectorDelegate>> {
        cx.dispatch_action(Toggle);
        cx.run_until_parked();
        active_picker(workspace, cx)
    }

    /// 关闭语言选择器
    fn close_selector(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) {
        cx.dispatch_action(Toggle);
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, cx| {
            assert!(
                workspace.active_modal::<LanguageSelector>(cx).is_none(),
                "语言选择器应已关闭"
            );
        });
    }

    /// 断言编辑器选中的语言
    fn assert_selected_language_for_editor(
        workspace: &Entity<Workspace>,
        editor: &Entity<Editor>,
        expected_language_name: Option<&str>,
        cx: &mut VisualTestContext,
    ) {
        workspace.update_in(cx, |workspace, window, cx| {
            let was_activated = workspace.activate_item(editor, true, true, window, cx);
            assert!(
                was_activated,
                "打开弹窗前应先激活编辑器"
            );
        });
        cx.run_until_parked();

        let picker = open_selector(workspace, cx);
        picker.read_with(cx, |picker, _| {
            let selected_match = picker
                .delegate
                .matches
                .get(picker.delegate.selected_index)
                .expect("选中索引应指向有效匹配项");
            let selected_candidate = picker
                .delegate
                .candidates
                .get(selected_match.candidate_id)
                .expect("选中匹配项应映射到有效候选项");

            if let Some(expected_language_name) = expected_language_name {
                let current_language_candidate_index = picker
                    .delegate
                    .current_language_candidate_index
                    .expect("当前语言应映射到有效候选项");
                assert_eq!(
                    selected_match.candidate_id,
                    current_language_candidate_index
                );
                assert_eq!(selected_candidate.string, expected_language_name);
            } else {
                assert!(picker.delegate.current_language_candidate_index.is_none());
                assert_eq!(picker.delegate.selected_index, 0);
            }
        });
        close_selector(workspace, cx);
    }

    #[gpui::test]
    /// 测试语言选择器根据活动编辑器选中当前语言
    async fn test_language_selector_selects_current_language_per_active_editor(
        cx: &mut TestAppContext,
    ) {
        let app_state = init_test(cx);
        app_state
            .fs
            .as_fake()
            .insert_tree(
                path!("/test"),
                json!({
                    "rust_file.rs": "fn main() {}\n",
                    "typescript_file.ts": "const value = 1;\n",
                }),
            )
            .await;

        let project = Project::test(app_state.fs.clone(), [path!("/test").as_ref()], cx).await;
        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
        let workspace =
            multi_workspace.read_with(cx, |multi_workspace, _| multi_workspace.workspace().clone());
        register_test_languages(&project, cx);

        let rust_editor = open_file_editor(&workspace, &project, "rust_file.rs", cx).await;
        let typescript_editor =
            open_file_editor(&workspace, &project, "typescript_file.ts", cx).await;
        let empty_editor = open_empty_editor(&workspace, &project, cx).await;

        set_editor_language(&project, &rust_editor, "Rust", cx).await;
        set_editor_language(&project, &typescript_editor, "TypeScript", cx).await;
        cx.run_until_parked();

        assert_selected_language_for_editor(&workspace, &rust_editor, Some("Rust"), cx);
        assert_selected_language_for_editor(&workspace, &typescript_editor, Some("TypeScript"), cx);
        // 确保断言前空白编辑器缓冲区无语言
        let buffer = empty_editor.read_with(cx, |editor, cx| {
            editor
                .active_buffer(cx)
                .expect("编辑器应包含活动片段")
        });
        buffer.update(cx, |buffer, cx| {
            buffer.set_language(None, cx);
        });
        assert_selected_language_for_editor(&workspace, &empty_editor, None, cx);
    }

    #[gpui::test]
    /// 测试新建缓冲区查询后语言选择器选中第一个匹配项
    async fn test_language_selector_selects_first_match_after_querying_new_buffer(
        cx: &mut TestAppContext,
    ) {
        let app_state = init_test(cx);
        app_state
            .fs
            .as_fake()
            .insert_tree(path!("/test"), json!({}))
            .await;

        let project = Project::test(app_state.fs.clone(), [path!("/test").as_ref()], cx).await;
        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
        let workspace =
            multi_workspace.read_with(cx, |multi_workspace, _| multi_workspace.workspace().clone());
        register_test_languages(&project, cx);

        let editor = open_new_buffer_editor(&workspace, &project, cx).await;
        workspace.update_in(cx, |workspace, window, cx| {
            let was_activated = workspace.activate_item(&editor, true, true, window, cx);
            assert!(
                was_activated,
                "打开弹窗前应先激活编辑器"
            );
        });
        cx.run_until_parked();

        let picker = open_selector(&workspace, cx);
        picker.read_with(cx, |picker, _| {
            let selected_match = picker
                .delegate
                .matches
                .get(picker.delegate.selected_index)
                .expect("选中索引应指向有效匹配项");
            let selected_candidate = picker
                .delegate
                .candidates
                .get(selected_match.candidate_id)
                .expect("选中匹配项应映射到有效候选项");

            assert_eq!(selected_candidate.string, "Plain Text");
            assert!(
                picker
                    .delegate
                    .current_language_candidate_index
                    .is_some_and(|current_language_candidate_index| {
                        current_language_candidate_index > 1
                    }),
                "测试环境应将纯文本置于至少两个前置语言之后",
            );
        });

        picker.update_in(cx, |picker, window, cx| {
            picker.update_matches("ru".to_string(), window, cx)
        });
        cx.run_until_parked();

        picker.read_with(cx, |picker, _| {
            assert!(
                picker.delegate.matches.len() > 1,
                "查询应返回多个匹配项"
            );
            assert_eq!(picker.delegate.selected_index, 0);

            let first_match = picker
                .delegate
                .matches
                .first()
                .expect("查询应至少产生一个匹配项");
            let selected_match = picker
                .delegate
                .matches
                .get(picker.delegate.selected_index)
                .expect("选中索引应指向有效匹配项");

            assert_eq!(selected_match.candidate_id, first_match.candidate_id);
        });
    }
}