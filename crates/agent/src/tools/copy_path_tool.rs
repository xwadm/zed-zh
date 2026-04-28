use super::tool_permissions::{
    SensitiveSettingsKind, authorize_symlink_escapes, canonicalize_worktree_roots,
    collect_symlink_escapes, sensitive_settings_kind,
};
use crate::{
    AgentTool, ToolCallEventStream, ToolInput, ToolPermissionDecision, decide_permission_for_paths,
};
use agent_client_protocol::schema as acp;
use agent_settings::AgentSettings;
use futures::FutureExt as _;
use gpui::{App, Entity, Task};
use project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use settings::Settings;
use std::path::Path;
use std::sync::Arc;
use util::markdown::MarkdownInlineCode;

/// 复制项目中的文件或目录，并返回复制成功的确认信息。
/// 目录内容将被递归复制。
///
/// 当需要创建文件或目录的副本且不修改原文件时，应使用此工具。
/// 相比分别读取再写入文件或目录内容的方式，此工具效率更高，因此只要目标是复制操作，就应优先使用此工具。
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CopyPathToolInput {
    /// 要复制的文件或目录的源路径。
    /// 如果指定目录，其内容将被递归复制。
    ///
    /// <示例>
    /// 如果项目包含以下文件：
    ///
    /// - directory1/a/something.txt
    /// - directory2/a/things.txt
    /// - directory3/a/other.txt
    ///
    /// 可以通过提供源路径 "directory1/a/something.txt" 来复制第一个文件
    /// </示例>
    pub source_path: String,
    /// 文件或目录要复制到的目标路径。
    ///
    /// <示例>
    /// 要将 "directory1/a/something.txt" 复制到 "directory2/b/copy.txt"，请提供目标路径 "directory2/b/copy.txt"
    /// </示例>
    pub destination_path: String,
}

pub struct CopyPathTool {
    project: Entity<Project>,
}

impl CopyPathTool {
    pub fn new(project: Entity<Project>) -> Self {
        Self { project }
    }
}

impl AgentTool for CopyPathTool {
    type Input = CopyPathToolInput;
    type Output = String;

    const NAME: &'static str = "copy_path";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Move
    }

    fn initial_title(
        &self,
        input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> ui::SharedString {
        if let Ok(input) = input {
            let src = MarkdownInlineCode(&input.source_path);
            let dest = MarkdownInlineCode(&input.destination_path);
            format!("复制 {src} 到 {dest}").into()
        } else {
            "复制路径".into()
        }
    }

    fn run(
        self: Arc<Self>,
        input: ToolInput<Self::Input>,
        event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<Self::Output, Self::Output>> {
        let project = self.project.clone();
        cx.spawn(async move |cx| {
            let input = input
                .recv()
                .await
                .map_err(|e| format!("接收工具输入失败：{e}"))?;
            let paths = vec![input.source_path.clone(), input.destination_path.clone()];
            let decision = cx.update(|cx| {
                decide_permission_for_paths(Self::NAME, &paths, &AgentSettings::get_global(cx))
            });
            if let ToolPermissionDecision::Deny(reason) = decision {
                return Err(reason);
            }

            let fs = project.read_with(cx, |project, _cx| project.fs().clone());
            let canonical_roots = canonicalize_worktree_roots(&project, &fs, cx).await;

            let symlink_escapes: Vec<(&str, std::path::PathBuf)> =
                project.read_with(cx, |project, cx| {
                    collect_symlink_escapes(
                        project,
                        &input.source_path,
                        &input.destination_path,
                        &canonical_roots,
                        cx,
                    )
                });

            let sensitive_kind =
                sensitive_settings_kind(Path::new(&input.source_path), fs.as_ref())
                    .await
                    .or(
                        sensitive_settings_kind(Path::new(&input.destination_path), fs.as_ref())
                            .await,
                    );

            let needs_confirmation = matches!(decision, ToolPermissionDecision::Confirm)
                || (matches!(decision, ToolPermissionDecision::Allow) && sensitive_kind.is_some());

            let authorize = if !symlink_escapes.is_empty() {
                // 符号链接越权授权会替换（而非补充）
                // 常规的工具权限提示。符号链接提示已
                // 要求用户显式批准并显示规范目标，
                // 其安全性比通用确认提示更高。
                Some(cx.update(|cx| {
                    authorize_symlink_escapes(Self::NAME, &symlink_escapes, &event_stream, cx)
                }))
            } else if needs_confirmation {
                Some(cx.update(|cx| {
                    let src = MarkdownInlineCode(&input.source_path);
                    let dest = MarkdownInlineCode(&input.destination_path);
                    let context = crate::ToolPermissionContext::new(
                        Self::NAME,
                        vec![input.source_path.clone(), input.destination_path.clone()],
                    );
                    let title = format!("复制 {src} 到 {dest}");
                    let title = match sensitive_kind {
                        Some(SensitiveSettingsKind::Local) => format!("{title}（本地设置）"),
                        Some(SensitiveSettingsKind::Global) => format!("{title}（设置）"),
                        None => title,
                    };
                    event_stream.authorize(title, context, cx)
                }))
            } else {
                None
            };

            if let Some(authorize) = authorize {
                authorize.await.map_err(|e| e.to_string())?;
            }

            let copy_task = project.update(cx, |project, cx| {
                match project
                    .find_project_path(&input.source_path, cx)
                    .and_then(|project_path| project.entry_for_path(&project_path, cx))
                {
                    Some(entity) => match project.find_project_path(&input.destination_path, cx) {
                        Some(project_path) => Ok(project.copy_entry(entity.id, project_path, cx)),
                        None => Err(format!(
                            "目标路径 {} 超出项目范围。",
                            input.destination_path
                        )),
                    },
                    None => Err(format!(
                        "源路径 {} 在项目中未找到。",
                        input.source_path
                    )),
                }
            })?;

            let result = futures::select! {
                result = copy_task.fuse() => result,
                _ = event_stream.cancelled_by_user().fuse() => {
                    return Err("用户已取消复制操作".to_string());
                }
            };
            result.map_err(|e| {
                format!(
                    "复制 {} 到 {} 失败：{e}",
                    input.source_path, input.destination_path
                )
            })?;
            Ok(format!(
                "已复制 {} 到 {}",
                input.source_path, input.destination_path
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs::Fs as _;
    use gpui::TestAppContext;
    use project::{FakeFs, Project};
    use serde_json::json;
    use settings::SettingsStore;
    use std::path::PathBuf;
    use util::path;

    /// 初始化测试环境
    fn init_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let settings_store = SettingsStore::test(cx);
            cx.set_global(settings_store);
        });
        cx.update(|cx| {
            let mut settings = AgentSettings::get_global(cx).clone();
            settings.tool_permissions.default = settings::ToolPermissionMode::Allow;
            AgentSettings::override_global(settings, cx);
        });
    }

    #[gpui::test]
    async fn test_copy_path_symlink_escape_source_requests_authorization(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        fs.insert_tree(
            path!("/root"),
            json!({
                "project": {
                    "src": { "file.txt": "content" }
                },
                "external": {
                    "secret.txt": "SECRET"
                }
            }),
        )
        .await;

        fs.create_symlink(
            path!("/root/project/link_to_external").as_ref(),
            PathBuf::from("../external"),
        )
        .await
        .unwrap();

        let project = Project::test(fs.clone(), [path!("/root/project").as_ref()], cx).await;
        cx.executor().run_until_parked();

        let tool = Arc::new(CopyPathTool::new(project));

        let input = CopyPathToolInput {
            source_path: "project/link_to_external".into(),
            destination_path: "project/external_copy".into(),
        };

        let (event_stream, mut event_rx) = ToolCallEventStream::test();
        let task = cx.update(|cx| tool.run(ToolInput::resolved(input), event_stream, cx));

        let auth = event_rx.expect_authorization().await;
        let title = auth.tool_call.fields.title.as_deref().unwrap_or("");
        assert!(
            title.contains("points outside the project")
                || title.contains("symlinks outside project"),
            "授权标题应提示符号链接越界，实际为：{title}",
        );

        auth.response
            .send(acp_thread::SelectedPermissionOutcome::new(
                acp::PermissionOptionId::new("allow"),
                acp::PermissionOptionKind::AllowOnce,
            ))
            .unwrap();

        let result = task.await;
        assert!(result.is_ok(), "批准后应执行成功：{result:?}");
    }

    #[gpui::test]
    async fn test_copy_path_symlink_escape_denied(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        fs.insert_tree(
            path!("/root"),
            json!({
                "project": {
                    "src": { "file.txt": "content" }
                },
                "external": {
                    "secret.txt": "SECRET"
                }
            }),
        )
        .await;

        fs.create_symlink(
            path!("/root/project/link_to_external").as_ref(),
            PathBuf::from("../external"),
        )
        .await
        .unwrap();

        let project = Project::test(fs.clone(), [path!("/root/project").as_ref()], cx).await;
        cx.executor().run_until_parked();

        let tool = Arc::new(CopyPathTool::new(project));

        let input = CopyPathToolInput {
            source_path: "project/link_to_external".into(),
            destination_path: "project/external_copy".into(),
        };

        let (event_stream, mut event_rx) = ToolCallEventStream::test();
        let task = cx.update(|cx| tool.run(ToolInput::resolved(input), event_stream, cx));

        let auth = event_rx.expect_authorization().await;
        drop(auth);

        let result = task.await;
        assert!(result.is_err(), "拒绝授权时应执行失败");
    }

    #[gpui::test]
    async fn test_copy_path_symlink_escape_confirm_requires_single_approval(
        cx: &mut TestAppContext,
    ) {
        init_test(cx);
        cx.update(|cx| {
            let mut settings = AgentSettings::get_global(cx).clone();
            settings.tool_permissions.default = settings::ToolPermissionMode::Confirm;
            AgentSettings::override_global(settings, cx);
        });

        let fs = FakeFs::new(cx.executor());
        fs.insert_tree(
            path!("/root"),
            json!({
                "project": {
                    "src": { "file.txt": "content" }
                },
                "external": {
                    "secret.txt": "SECRET"
                }
            }),
        )
        .await;

        fs.create_symlink(
            path!("/root/project/link_to_external").as_ref(),
            PathBuf::from("../external"),
        )
        .await
        .unwrap();

        let project = Project::test(fs.clone(), [path!("/root/project").as_ref()], cx).await;
        cx.executor().run_until_parked();

        let tool = Arc::new(CopyPathTool::new(project));

        let input = CopyPathToolInput {
            source_path: "project/link_to_external".into(),
            destination_path: "project/external_copy".into(),
        };

        let (event_stream, mut event_rx) = ToolCallEventStream::test();
        let task = cx.update(|cx| tool.run(ToolInput::resolved(input), event_stream, cx));

        let auth = event_rx.expect_authorization().await;
        let title = auth.tool_call.fields.title.as_deref().unwrap_or("");
        assert!(
            title.contains("points outside the project")
                || title.contains("symlinks outside project"),
            "授权标题应提示符号链接越界，实际为：{title}",
        );

        auth.response
            .send(acp_thread::SelectedPermissionOutcome::new(
                acp::PermissionOptionId::new("allow"),
                acp::PermissionOptionKind::AllowOnce,
            ))
            .unwrap();

        assert!(
            !matches!(
                event_rx.try_recv(),
                Ok(Ok(crate::ThreadEvent::ToolCallAuthorization(_)))
            ),
            "预期仅弹出一次授权提示",
        );

        let result = task.await;
        assert!(
            result.is_ok(),
            "一次授权后工具应执行成功：{result:?}"
        );
    }

    #[gpui::test]
    async fn test_copy_path_symlink_escape_honors_deny_policy(cx: &mut TestAppContext) {
        init_test(cx);
        cx.update(|cx| {
            let mut settings = AgentSettings::get_global(cx).clone();
            settings.tool_permissions.tools.insert(
                "copy_path".into(),
                agent_settings::ToolRules {
                    default: Some(settings::ToolPermissionMode::Deny),
                    ..Default::default()
                },
            );
            AgentSettings::override_global(settings, cx);
        });

        let fs = FakeFs::new(cx.executor());
        fs.insert_tree(
            path!("/root"),
            json!({
                "project": {
                    "src": { "file.txt": "content" }
                },
                "external": {
                    "secret.txt": "SECRET"
                }
            }),
        )
        .await;

        fs.create_symlink(
            path!("/root/project/link_to_external").as_ref(),
            PathBuf::from("../external"),
        )
        .await
        .unwrap();

        let project = Project::test(fs.clone(), [path!("/root/project").as_ref()], cx).await;
        cx.executor().run_until_parked();

        let tool = Arc::new(CopyPathTool::new(project));

        let input = CopyPathToolInput {
            source_path: "project/link_to_external".into(),
            destination_path: "project/external_copy".into(),
        };

        let (event_stream, mut event_rx) = ToolCallEventStream::test();
        let result = cx
            .update(|cx| tool.run(ToolInput::resolved(input), event_stream, cx))
            .await;

        assert!(result.is_err(), "策略拒绝时工具应执行失败");
        assert!(
            !matches!(
                event_rx.try_recv(),
                Ok(Ok(crate::ThreadEvent::ToolCallAuthorization(_)))
            ),
            "拒绝策略不应弹出符号链接授权提示",
        );
    }
}