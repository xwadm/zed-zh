use super::tool_permissions::{
    SensitiveSettingsKind, authorize_symlink_access, canonicalize_worktree_roots,
    detect_symlink_escape, sensitive_settings_kind,
};
use agent_client_protocol::schema as acp;
use agent_settings::AgentSettings;
use futures::FutureExt as _;
use gpui::{App, Entity, SharedString, Task};
use project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use settings::Settings;
use std::sync::Arc;
use util::markdown::MarkdownInlineCode;

use crate::{
    AgentTool, ToolCallEventStream, ToolInput, ToolPermissionDecision, decide_permission_for_path,
};
use std::path::Path;

/// 在项目内的指定路径创建新目录。返回目录创建成功的确认信息。
///
/// 该工具会创建目录及所有必要的父目录。需要在项目内创建新目录时均应使用此工具。
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CreateDirectoryToolInput {
    /// 新目录的路径
    ///
    /// <示例>
    /// 若项目具有以下结构：
    ///
    /// - directory1/
    /// - directory2/
    ///
    /// 可通过传入路径 "directory1/new_directory" 创建新目录
    /// </示例>
    pub path: String,
}

pub struct CreateDirectoryTool {
    project: Entity<Project>,
}

impl CreateDirectoryTool {
    pub fn new(project: Entity<Project>) -> Self {
        Self { project }
    }
}

impl AgentTool for CreateDirectoryTool {
    type Input = CreateDirectoryToolInput;
    type Output = String;

    const NAME: &'static str = "create_directory";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Read
    }

    fn initial_title(
        &self,
        input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        if let Ok(input) = input {
            format!("创建目录 {}", MarkdownInlineCode(&input.path)).into()
        } else {
            "创建目录".into()
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
            let decision = cx.update(|cx| {
                decide_permission_for_path(Self::NAME, &input.path, AgentSettings::get_global(cx))
            });

            if let ToolPermissionDecision::Deny(reason) = decision {
                return Err(reason);
            }

            let destination_path: Arc<str> = input.path.as_str().into();

            let fs = project.read_with(cx, |project, _cx| project.fs().clone());
            let canonical_roots = canonicalize_worktree_roots(&project, &fs, cx).await;

            let symlink_escape_target = project.read_with(cx, |project, cx| {
                detect_symlink_escape(project, &input.path, &canonical_roots, cx)
                    .map(|(_, target)| target)
            });

            let sensitive_kind = sensitive_settings_kind(Path::new(&input.path), fs.as_ref()).await;

            let decision =
                if matches!(decision, ToolPermissionDecision::Allow) && sensitive_kind.is_some() {
                    ToolPermissionDecision::Confirm
                } else {
                    decision
                };

            let authorize = if let Some(canonical_target) = symlink_escape_target {
                // 符号链接越权授权会替换（而非补充）常规的工具权限提示
                // 符号链接提示已要求用户显式批准并显示规范目标路径
                // 其安全性比通用确认提示更严格
                Some(cx.update(|cx| {
                    authorize_symlink_access(
                        Self::NAME,
                        &input.path,
                        &canonical_target,
                        &event_stream,
                        cx,
                    )
                }))
            } else {
                match decision {
                    ToolPermissionDecision::Allow => None,
                    ToolPermissionDecision::Confirm => Some(cx.update(|cx| {
                        let title = format!("创建目录 {}", MarkdownInlineCode(&input.path));
                        let title = match &sensitive_kind {
                            Some(SensitiveSettingsKind::Local) => {
                                format!("{title}（本地设置）")
                            }
                            Some(SensitiveSettingsKind::Global) => format!("{title}（设置）"),
                            None => title,
                        };
                        let context =
                            crate::ToolPermissionContext::new(Self::NAME, vec![input.path.clone()]);
                        event_stream.authorize(title, context, cx)
                    })),
                    ToolPermissionDecision::Deny(_) => None,
                }
            };

            if let Some(authorize) = authorize {
                authorize.await.map_err(|e| e.to_string())?;
            }

            let create_entry = project.update(cx, |project, cx| {
                match project.find_project_path(&input.path, cx) {
                    Some(project_path) => Ok(project.create_entry(project_path, true, cx)),
                    None => Err("待创建路径超出项目范围".to_string()),
                }
            })?;

            futures::select! {
                result = create_entry.fuse() => {
                    result.map_err(|e| format!("创建目录 {destination_path} 失败：{e}"))?;
                }
                _ = event_stream.cancelled_by_user().fuse() => {
                    return Err("创建目录已被用户取消".to_string());
                }
            }

            Ok(format!("已创建目录 {destination_path}"))
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

    use crate::ToolCallEventStream;

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

    /// 测试创建目录时符号链接越权会请求授权
    #[gpui::test]
    async fn test_create_directory_symlink_escape_requests_authorization(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        fs.insert_tree(
            path!("/root"),
            json!({
                "project": {
                    "src": { "main.rs": "fn main() {}" }
                },
                "external": {
                    "data": { "file.txt": "content" }
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

        let tool = Arc::new(CreateDirectoryTool::new(project));

        let (event_stream, mut event_rx) = ToolCallEventStream::test();
        let task = cx.update(|cx| {
            tool.run(
                ToolInput::resolved(CreateDirectoryToolInput {
                    path: "project/link_to_external".into(),
                }),
                event_stream,
                cx,
            )
        });

        let auth = event_rx.expect_authorization().await;
        let title = auth.tool_call.fields.title.as_deref().unwrap_or("");
        assert!(
            title.contains("points outside the project") || title.contains("symlink"),
            "授权标题应提示符号链接越权，实际为：{title}",
        );

        auth.response
            .send(acp_thread::SelectedPermissionOutcome::new(
                acp::PermissionOptionId::new("allow"),
                acp::PermissionOptionKind::AllowOnce,
            ))
            .unwrap();

        let result = task.await;
        assert!(
            result.is_ok(),
            "授权后工具应执行成功：{result:?}"
        );
    }

    /// 测试创建目录时符号链接越权被拒绝
    #[gpui::test]
    async fn test_create_directory_symlink_escape_denied(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        fs.insert_tree(
            path!("/root"),
            json!({
                "project": {
                    "src": { "main.rs": "fn main() {}" }
                },
                "external": {
                    "data": { "file.txt": "content" }
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

        let tool = Arc::new(CreateDirectoryTool::new(project));

        let (event_stream, mut event_rx) = ToolCallEventStream::test();
        let task = cx.update(|cx| {
            tool.run(
                ToolInput::resolved(CreateDirectoryToolInput {
                    path: "project/link_to_external".into(),
                }),
                event_stream,
                cx,
            )
        });

        let auth = event_rx.expect_authorization().await;

        drop(auth);

        let result = task.await;
        assert!(
            result.is_err(),
            "授权被拒绝时工具应执行失败"
        );
    }

    /// 测试创建目录时符号链接越权确认仅需单次批准
    #[gpui::test]
    async fn test_create_directory_symlink_escape_confirm_requires_single_approval(
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
                    "src": { "main.rs": "fn main() {}" }
                },
                "external": {
                    "data": { "file.txt": "content" }
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

        let tool = Arc::new(CreateDirectoryTool::new(project));

        let (event_stream, mut event_rx) = ToolCallEventStream::test();
        let task = cx.update(|cx| {
            tool.run(
                ToolInput::resolved(CreateDirectoryToolInput {
                    path: "project/link_to_external".into(),
                }),
                event_stream,
                cx,
            )
        });

        let auth = event_rx.expect_authorization().await;
        let title = auth.tool_call.fields.title.as_deref().unwrap_or("");
        assert!(
            title.contains("points outside the project") || title.contains("symlink"),
            "授权标题应提示符号链接越权，实际为：{title}",
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
            "单次授权后工具应执行成功：{result:?}"
        );
    }

    /// 测试创建目录时符号链接越权遵循拒绝策略
    #[gpui::test]
    async fn test_create_directory_symlink_escape_honors_deny_policy(cx: &mut TestAppContext) {
        init_test(cx);
        cx.update(|cx| {
            let mut settings = AgentSettings::get_global(cx).clone();
            settings.tool_permissions.tools.insert(
                "create_directory".into(),
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
                    "src": { "main.rs": "fn main() {}" }
                },
                "external": {
                    "data": { "file.txt": "content" }
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

        let tool = Arc::new(CreateDirectoryTool::new(project));

        let (event_stream, mut event_rx) = ToolCallEventStream::test();
        let result = cx
            .update(|cx| {
                tool.run(
                    ToolInput::resolved(CreateDirectoryToolInput {
                        path: "project/link_to_external".into(),
                    }),
                    event_stream,
                    cx,
                )
            })
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