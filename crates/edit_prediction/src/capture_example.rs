use crate::{StoredEvent, example_spec::ExampleSpec};
use anyhow::Result;
use buffer_diff::BufferDiffSnapshot;
use collections::HashMap;
use gpui::{App, Entity, Task};
use language::Buffer;
use project::{Project, WorktreeId};
use std::{collections::hash_map, fmt::Write as _, ops::Range, path::Path, sync::Arc};
use text::{BufferSnapshot as TextBufferSnapshot, Point};

/// 捕获当前项目的代码示例，用于生成 AI 训练样本
/// 包含：光标位置、编辑历史、未提交差异、仓库信息等
pub fn capture_example(
    project: Entity<Project>,
    buffer: Entity<Buffer>,
    cursor_anchor: language::Anchor,
    mut events: Vec<StoredEvent>,
    populate_expected_patch: bool,
    cx: &mut App,
) -> Option<Task<Result<ExampleSpec>>> {
    let snapshot = buffer.read(cx).snapshot();
    let file = snapshot.file()?;
    let worktree_id = file.worktree_id(cx);
    let repository = project.read(cx).active_repository(cx)?;
    let repository_snapshot = repository.read(cx).snapshot();
    let worktree = project.read(cx).worktree_for_id(worktree_id, cx)?;
    let root_name = worktree.read(cx).root_name_str().to_owned();
    let cursor_path: Arc<Path> = file.path().as_std_path().into();

    // 只处理在 Git 仓库工作目录内的文件
    if worktree.read(cx).abs_path() != repository_snapshot.work_directory_abs_path {
        return None;
    }

    // 获取仓库远程地址与当前提交版本
    let repository_url = repository_snapshot
        .remote_origin_url
        .clone()
        .or_else(|| repository_snapshot.remote_upstream_url.clone())?;
    let revision = repository_snapshot.head_commit.as_ref()?.sha.to_string();

    let git_store = project.read(cx).git_store().clone();

    Some(cx.spawn(async move |mut cx| {
        // 收集所有变更文件的快照
        let snapshots_by_path =
            collect_snapshots(&project, &git_store, worktree_id, &events, &mut cx).await?;

        // 只保留在项目内的文件变更事件
        events.retain(|stored_event| {
            let zeta_prompt::Event::BufferChange { path, .. } = stored_event.event.as_ref();
            let relative_path = strip_root_name(path, &root_name);
            snapshots_by_path.contains_key(relative_path)
        });

        // 获取当前语言的行注释前缀
        let line_comment_prefix = snapshot
            .language()
            .and_then(|lang| lang.config().line_comments.first())
            .map(|s| s.to_string())
            .unwrap_or_default();

        // 计算光标所在的代码片段
        let (cursor_excerpt, cursor_offset_in_excerpt, cursor_excerpt_range) = cx
            .background_executor()
            .spawn(async move { compute_cursor_excerpt(&snapshot, cursor_anchor) })
            .await;

        // 计算所有未提交的 Git 差异
        let uncommitted_diff = cx
            .background_executor()
            .spawn(async move { compute_uncommitted_diff(snapshots_by_path) })
            .await;

        // 生成编辑历史文本
        let mut edit_history = String::new();
        for stored_event in &events {
            write_event_with_relative_paths(&mut edit_history, &stored_event.event, &root_name);
            if !edit_history.ends_with('\n') {
                edit_history.push('\n');
            }
        }

        // 生成空的预期补丁模板（方便手动编写预期结果）
        let mut expected_patches = Vec::new();
        let mut rejected_patch = None;
        if populate_expected_patch {
            let mut empty_patch = String::new();
            let start_row = cursor_excerpt_range.start.row + 1;
            let row_count = cursor_excerpt_range.end.row - cursor_excerpt_range.start.row + 1;
            writeln!(&mut empty_patch, "--- a/{}", cursor_path.display()).ok();
            writeln!(&mut empty_patch, "+++ b/{}", cursor_path.display()).ok();
            writeln!(
                &mut empty_patch,
                "@@ -{},{} +{},{} @@",
                start_row, row_count, start_row, row_count,
            )
            .ok();
            for line in cursor_excerpt.lines() {
                writeln!(&mut empty_patch, " {}", line).ok();
            }

            expected_patches.push(empty_patch.clone());
            rejected_patch = Some(empty_patch);
        }

        // 构建最终的示例规格
        let mut spec = ExampleSpec {
            name: generate_timestamp_name(),
            repository_url,
            revision,
            tags: Vec::new(),
            reasoning: None,
            uncommitted_diff,
            cursor_path,
            cursor_position: String::new(),
            edit_history,
            expected_patches,
            rejected_patch,
            telemetry: None,
            human_feedback: Vec::new(),
            rating: None,
        };
        spec.set_cursor_excerpt(
            &cursor_excerpt,
            cursor_offset_in_excerpt,
            &line_comment_prefix,
        );
        Ok(spec)
    }))
}

/// 移除路径前缀中的工作区根目录名称，获取相对路径
fn strip_root_name<'a>(path: &'a Path, root_name: &str) -> &'a Path {
    path.strip_prefix(root_name).unwrap_or(path)
}

/// 将事件写入编辑历史，使用相对路径
fn write_event_with_relative_paths(
    output: &mut String,
    event: &zeta_prompt::Event,
    root_name: &str,
) {
    fn write_relative_path(output: &mut String, path: &Path, root_name: &str) {
        for component in strip_root_name(path, root_name).components() {
            output.push('/');
            write!(output, "{}", component.as_os_str().to_string_lossy()).ok();
        }
    }

    let zeta_prompt::Event::BufferChange {
        path,
        old_path,
        diff,
        ..
    } = event;

    output.push_str("--- a");
    write_relative_path(output, old_path.as_ref(), root_name);
    output.push_str("\n+++ b");
    write_relative_path(output, path.as_ref(), root_name);
    output.push('\n');
    output.push_str(diff);
}

/// 计算光标所在的代码片段、偏移量和范围
fn compute_cursor_excerpt(
    snapshot: &language::BufferSnapshot,
    cursor_anchor: language::Anchor,
) -> (String, usize, Range<Point>) {
    use text::ToOffset as _;
    use text::ToPoint as _;

    let cursor_offset = cursor_anchor.to_offset(snapshot);
    let (excerpt_point_range, excerpt_offset_range, cursor_offset_in_excerpt) =
        crate::cursor_excerpt::compute_cursor_excerpt(snapshot, cursor_offset);
    let syntax_ranges = crate::cursor_excerpt::compute_syntax_ranges(
        snapshot,
        cursor_offset,
        &excerpt_offset_range,
    );
    let excerpt_text: String = snapshot.text_for_range(excerpt_point_range).collect();
    let (_, context_range) = zeta_prompt::compute_editable_and_context_ranges(
        &excerpt_text,
        cursor_offset_in_excerpt,
        &syntax_ranges,
        100,
        50,
    );
    let context_text = excerpt_text[context_range.clone()].to_string();
    let cursor_in_context = cursor_offset_in_excerpt.saturating_sub(context_range.start);
    let context_buffer_start =
        (excerpt_offset_range.start + context_range.start).to_point(snapshot);
    let context_buffer_end = (excerpt_offset_range.start + context_range.end).to_point(snapshot);
    (
        context_text,
        cursor_in_context,
        context_buffer_start..context_buffer_end,
    )
}

/// 收集所有变更文件的缓冲区快照与差异快照
async fn collect_snapshots(
    project: &Entity<Project>,
    git_store: &Entity<project::git_store::GitStore>,
    worktree_id: WorktreeId,
    events: &[StoredEvent],
    cx: &mut gpui::AsyncApp,
) -> Result<HashMap<Arc<Path>, (TextBufferSnapshot, BufferDiffSnapshot)>> {
    let mut snapshots_by_path = HashMap::default();
    for stored_event in events {
        let zeta_prompt::Event::BufferChange { path, .. } = stored_event.event.as_ref();
        if let Some((project_path, relative_path)) = project.read_with(cx, |project, cx| {
            let project_path = project
                .find_project_path(path, cx)
                .filter(|path| path.worktree_id == worktree_id)?;
            let relative_path: Arc<Path> = project_path.path.as_std_path().into();
            Some((project_path, relative_path))
        }) {
            if let hash_map::Entry::Vacant(entry) = snapshots_by_path.entry(relative_path) {
                let buffer = project
                    .update(cx, |project, cx| {
                        project.open_buffer(project_path.clone(), cx)
                    })
                    .await?;
                let diff = git_store
                    .update(cx, |git_store, cx| {
                        git_store.open_uncommitted_diff(buffer.clone(), cx)
                    })
                    .await?;
                let diff_snapshot = diff.update(cx, |diff, cx| diff.snapshot(cx));
                entry.insert((stored_event.old_snapshot.clone(), diff_snapshot));
            }
        }
    }
    Ok(snapshots_by_path)
}

/// 生成所有文件的未提交统一差异（unified diff）
fn compute_uncommitted_diff(
    snapshots_by_path: HashMap<Arc<Path>, (TextBufferSnapshot, BufferDiffSnapshot)>,
) -> String {
    let mut uncommitted_diff = String::new();
    for (relative_path, (before_text, diff_snapshot)) in snapshots_by_path {
        if let Some(head_text) = &diff_snapshot.base_text_string() {
            let file_diff = language::unified_diff(head_text, &before_text.text());
            if !file_diff.is_empty() {
                let path_str = relative_path.to_string_lossy();
                writeln!(uncommitted_diff, "--- a/{path_str}").ok();
                writeln!(uncommitted_diff, "+++ b/{path_str}").ok();
                uncommitted_diff.push_str(&file_diff);
                if !uncommitted_diff.ends_with('\n') {
                    uncommitted_diff.push('\n');
                }
            }
        }
    }
    uncommitted_diff
}

/// 生成带时间戳的示例名称
fn generate_timestamp_name() -> String {
    let format = time::format_description::parse("[year]-[month]-[day] [hour]:[minute]:[second]");
    match format {
        Ok(format) => {
            let now = time::OffsetDateTime::now_local()
                .unwrap_or_else(|_| time::OffsetDateTime::now_utc());
            now.format(&format)
                .unwrap_or_else(|_| "unknown-time".to_string())
        }
        Err(_) => "unknown-time".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EditPredictionStore;
    use client::RefreshLlmTokenListener;
    use client::{Client, UserStore};
    use clock::FakeSystemClock;
    use gpui::{AppContext as _, TestAppContext, http_client::FakeHttpClient};
    use indoc::indoc;
    use language::{Anchor, Point};
    use project::{FakeFs, Project};
    use serde_json::json;
    use settings::SettingsStore;
    use std::path::Path;

    /// 测试示例捕获功能：验证生成的示例数据是否正确
    #[gpui::test]
    async fn test_capture_example(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());

        // Git 中已提交的代码
        let committed_contents = indoc! {"
            fn main() {
                one();
                two();
                three();
                four();
                five();
                six();
                seven();
                eight();
                nine();
            }
        "};

        // 磁盘上的当前代码
        let disk_contents = indoc! {"
            fn main() {
                // comment 1
                one();
                two();
                three();
                four();
                five();
                six();
                seven();
                eight();
                // comment 2
                nine();
            }
        "};

        // 构建测试文件系统
        fs.insert_tree(
            "/project",
            json!({
                ".git": {},
                "src": {
                    "main.rs": disk_contents,
                }
            }),
        )
        .await;

        // 创建项目外的外部文件
        fs.insert_tree(
            "/external",
            json!({
                "external.rs": "fn external() {}\n",
            }),
        )
        .await;

        // 设置 Git 仓库信息
        fs.set_head_for_repo(
            Path::new("/project/.git"),
            &[("src/main.rs", committed_contents.to_string())],
            "abc123def456",
        );
        fs.set_remote_for_repo(
            Path::new("/project/.git"),
            "origin",
            "https://github.com/test/repo.git",
        );

        let project = Project::test(fs.clone(), ["/project".as_ref()], cx).await;

        // 打开并编辑缓冲区
        let buffer = project
            .update(cx, |project, cx| {
                project.open_local_buffer("/project/src/main.rs", cx)
            })
            .await
            .unwrap();

        let ep_store = cx.read(|cx| EditPredictionStore::try_global(cx).unwrap());
        ep_store.update(cx, |ep_store, cx| {
            ep_store.register_buffer(&buffer, &project, cx)
        });
        cx.run_until_parked();

        // 执行编辑操作
        buffer.update(cx, |buffer, cx| {
            let point = Point::new(6, 0);
            buffer.edit([(point..point, "    // comment 3\n")], None, cx);
            let point = Point::new(4, 0);
            buffer.edit([(point..point, "    // comment 4\n")], None, cx);

            pretty_assertions::assert_eq!(
                buffer.text(),
                indoc! {"
                    fn main() {
                        // comment 1
                        one();
                        two();
                        // comment 4
                        three();
                        four();
                        // comment 3
                        five();
                        six();
                        seven();
                        eight();
                        // comment 2
                        nine();
                    }
                "}
            );
        });
        cx.run_until_parked();

        // 编辑外部文件（项目外）
        let external_buffer = project
            .update(cx, |project, cx| {
                project.open_local_buffer("/external/external.rs", cx)
            })
            .await
            .unwrap();
        ep_store.update(cx, |ep_store, cx| {
            ep_store.register_buffer(&external_buffer, &project, cx)
        });
        cx.run_until_parked();
        external_buffer.update(cx, |buffer, cx| {
            let point = Point::new(0, 0);
            buffer.edit([(point..point, "// external edit\n")], None, cx);
        });
        cx.run_until_parked();

        // 验证事件包含外部文件编辑
        let events = ep_store.update(cx, |store, cx| store.edit_history_for_project(&project, cx));
        assert!(
            matches!(
                events
                    .last()
                    .unwrap()
                    .event
                    .as_ref(),
                zeta_prompt::Event::BufferChange { path, .. } if path.as_ref() == "/external/external.rs"
            ),
            "外部文件编辑应记录在事件中"
        );

        // 捕获示例
        let mut example = cx
            .update(|cx| {
                capture_example(
                    project.clone(),
                    buffer.clone(),
                    Anchor::min_for_buffer(buffer.read(cx).remote_id()),
                    events,
                    true,
                    cx,
                )
                .unwrap()
            })
            .await
            .unwrap();
        example.name = "test".to_string();

        // 验证最终生成的示例是否符合预期
        pretty_assertions::assert_eq!(
            example,
            ExampleSpec {
                name: "test".to_string(),
                repository_url: "https://github.com/test/repo.git".to_string(),
                revision: "abc123def456".to_string(),
                tags: Vec::new(),
                reasoning: None,
                uncommitted_diff: indoc! {"
                    --- a/src/main.rs
                    +++ b/src/main.rs
                    @@ -1,4 +1,5 @@
                     fn main() {
                    +    // comment 1
                         one();
                         two();
                         three();
                    @@ -7,5 +8,6 @@
                         six();
                         seven();
                         eight();
                    +    // comment 2
                         nine();
                     }
                "}
                .to_string(),
                cursor_path: Path::new("src/main.rs").into(),
                cursor_position: indoc! {"
                    fn main() {
                    ^[CURSOR_POSITION]
                        // comment 1
                        one();
                        two();
                        // comment 4
                        three();
                        four();
                        // comment 3
                        five();
                        six();
                        seven();
                        eight();
                        // comment 2
                        nine();
                    }
                "}
                .to_string(),
                edit_history: indoc! {"
                    --- a/src/main.rs
                    +++ b/src/main.rs
                    @@ -2,8 +2,10 @@
                         // comment 1
                         one();
                         two();
                    +    // comment 4
                         three();
                         four();
                    +    // comment 3
                         five();
                         six();
                         seven();
                "}
                .to_string(),
                expected_patches: vec![
                    indoc! {"
                        --- a/src/main.rs
                        +++ b/src/main.rs
                        @@ -1,16 +1,16 @@
                         fn main() {
                             // comment 1
                             one();
                             two();
                             // comment 4
                             three();
                             four();
                             // comment 3
                             five();
                             six();
                             seven();
                             eight();
                             // comment 2
                             nine();
                         }
                    "}
                    .to_string()
                ],
                rejected_patch: Some(
                    indoc! {"
                        --- a/src/main.rs
                        +++ b/src/main.rs
                        @@ -1,16 +1,16 @@
                         fn main() {
                             // comment 1
                             one();
                             two();
                             // comment 4
                             three();
                             four();
                             // comment 3
                             five();
                             six();
                             seven();
                             eight();
                             // comment 2
                             nine();
                         }
                    "}
                    .to_string()
                ),
                telemetry: None,
                human_feedback: Vec::new(),
                rating: None,
            }
        );
    }

    /// 初始化测试环境
    fn init_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let settings_store = SettingsStore::test(cx);
            cx.set_global(settings_store);
            zlog::init_test();
            let http_client = FakeHttpClient::with_404_response();
            let client = Client::new(Arc::new(FakeSystemClock::new()), http_client, cx);
            let user_store = cx.new(|cx| UserStore::new(client.clone(), cx));
            language_model::init(cx);
            RefreshLlmTokenListener::register(client.clone(), user_store.clone(), cx);
            EditPredictionStore::global(&client, &user_store, cx);
        })
    }
}