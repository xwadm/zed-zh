use project::search::SearchQuery;
use text::Rope;
use util::{
    paths::{PathMatcher, PathStyle},
    rel_path::RelPath,
};

#[test]
fn path_matcher_creation_for_valid_paths() {
    for valid_path in [
        "file",
        "Cargo.toml",
        ".DS_Store",
        "~/dir/another_dir/",
        "./dir/file",
        "dir/[a-z].txt",
    ] {
        let path_matcher = PathMatcher::new(&[valid_path.to_owned()], PathStyle::local())
            .unwrap_or_else(|e| panic!("有效路径 {valid_path} 应该被接受，但得到了错误: {e}"));
        assert!(
            path_matcher.is_match(&RelPath::new(valid_path.as_ref(), PathStyle::local()).unwrap()),
            "有效路径 {valid_path} 的路径匹配器应该匹配自身"
        )
    }
}

#[test]
fn path_matcher_creation_for_globs() {
    for invalid_glob in ["dir/[].txt", "dir/[a-z.txt", "dir/{file"] {
        match PathMatcher::new(&[invalid_glob.to_owned()], PathStyle::local()) {
            Ok(_) => panic!("无效的通配模式 {invalid_glob} 不应该被接受"),
            Err(_expected) => {}
        }
    }

    for valid_glob in [
        "dir/?ile",
        "dir/*.txt",
        "dir/**/file",
        "dir/[a-z].txt",
        "{dir,file}",
    ] {
        match PathMatcher::new(&[valid_glob.to_owned()], PathStyle::local()) {
            Ok(_expected) => {}
            Err(e) => panic!("有效的通配模式应该被接受，但得到了错误: {e}"),
        }
    }
}

#[test]
fn test_case_sensitive_pattern_items() {
    let case_sensitive = false;
    let search_query = SearchQuery::regex(
        "test\\C",
        false,
        case_sensitive,
        false,
        false,
        Default::default(),
        Default::default(),
        false,
        None,
    )
    .expect("应该能够创建正则 SearchQuery");

    assert_eq!(
        search_query.case_sensitive(),
        true,
        "当查询中存在 \\C 模式项时，应启用大小写敏感。"
    );

    let case_sensitive = true;
    let search_query = SearchQuery::regex(
        "test\\c",
        true,
        case_sensitive,
        false,
        false,
        Default::default(),
        Default::default(),
        false,
        None,
    )
    .expect("应该能够创建正则 SearchQuery");

    assert_eq!(
        search_query.case_sensitive(),
        false,
        "当存在 \\c 模式项时，即使初始设置为 true，也应禁用大小写敏感。"
    );

    let case_sensitive = false;
    let search_query = SearchQuery::regex(
        "test\\c\\C",
        false,
        case_sensitive,
        false,
        false,
        Default::default(),
        Default::default(),
        false,
        None,
    )
    .expect("应该能够创建正则 SearchQuery");

    assert_eq!(
        search_query.case_sensitive(),
        true,
        "当 \\C 是最后一个模式项时，即使前面有 \\c，也应启用大小写敏感。"
    );

    let case_sensitive = false;
    let search_query = SearchQuery::regex(
        "tests\\\\C",
        false,
        case_sensitive,
        false,
        false,
        Default::default(),
        Default::default(),
        false,
        None,
    )
    .expect("应该能够创建正则 SearchQuery");

    assert_eq!(
        search_query.case_sensitive(),
        false,
        "当 \\C 模式项前面有反斜杠时，不应启用大小写敏感。"
    );
}

#[gpui::test]
async fn test_multiline_regex(cx: &mut gpui::TestAppContext) {
    let search_query = SearchQuery::regex(
        "^hello$\n",
        false,
        false,
        false,
        false,
        Default::default(),
        Default::default(),
        false,
        None,
    )
    .expect("应该能够创建正则 SearchQuery");

    use language::Buffer;
    let text = Rope::from("hello\nworld\nhello\nworld");
    let snapshot = cx
        .update(|app| Buffer::build_snapshot(text, None, None, None, app))
        .await;

    let results = search_query.search(&snapshot, None).await;
    assert_eq!(results, vec![0..6, 12..18]);
}