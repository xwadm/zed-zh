use std::sync::Arc;

use fuzzy::StringMatch;
use gpui::{AnyElement, App, Context, DismissEvent, ReadGlobal, SharedString, Task, Window, px};
use picker::{Picker, PickerDelegate};
use settings::SettingsStore;
use ui::{ListItem, ListItemSpacing, PopoverMenu, prelude::*};
use util::ResultExt;

use crate::{
    SettingField, SettingsFieldMetadata, SettingsUiFile, render_picker_trigger_button,
    update_settings_file,
};

type OllamaModelPicker = Picker<OllamaModelPickerDelegate>;

/// Ollama模型选择器委托
struct OllamaModelPickerDelegate {
    /// 模型列表
    models: Vec<SharedString>,
    /// 过滤后的模型匹配结果
    filtered_models: Vec<StringMatch>,
    /// 当前选中的索引
    selected_index: usize,
    /// 模型变更回调
    on_model_changed: Arc<dyn Fn(SharedString, &mut Window, &mut App) + 'static>,
}

impl OllamaModelPickerDelegate {
    /// 创建模型选择器委托
    fn new(
        current_model: SharedString,
        on_model_changed: impl Fn(SharedString, &mut Window, &mut App) + 'static,
        cx: &mut Context<OllamaModelPicker>,
    ) -> Self {
        let mut models = edit_prediction::ollama::fetch_models(cx);

        // 如果当前模型不在列表中，添加到顶部
        let current_in_list = models.contains(&current_model);
        if !current_model.is_empty() && !current_in_list {
            models.insert(0, current_model.clone());
        }

        // 定位当前选中的模型
        let selected_index = models
            .iter()
            .position(|model| *model == current_model)
            .unwrap_or(0);

        // 初始化匹配结果
        let filtered_models = models
            .iter()
            .enumerate()
            .map(|(index, model)| StringMatch {
                candidate_id: index,
                string: model.to_string(),
                positions: Vec::new(),
                score: 0.0,
            })
            .collect();

        Self {
            models,
            filtered_models,
            selected_index,
            on_model_changed: Arc::new(on_model_changed),
        }
    }
}

impl PickerDelegate for OllamaModelPickerDelegate {
    type ListItem = AnyElement;

    /// 获取匹配结果数量
    fn match_count(&self) -> usize {
        self.filtered_models.len()
    }

    /// 获取当前选中索引
    fn selected_index(&self) -> usize {
        self.selected_index
    }

    /// 设置选中索引
    fn set_selected_index(
        &mut self,
        ix: usize,
        _: &mut Window,
        cx: &mut Context<OllamaModelPicker>,
    ) {
        self.selected_index = ix.min(self.filtered_models.len().saturating_sub(1));
        cx.notify();
    }

    /// 搜索框占位文本
    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        "搜索模型…".into()
    }

    /// 根据查询更新匹配结果
    fn update_matches(
        &mut self,
        query: String,
        _window: &mut Window,
        cx: &mut Context<OllamaModelPicker>,
    ) -> Task<()> {
        let query_lower = query.to_lowercase();

        self.filtered_models = self
            .models
            .iter()
            .enumerate()
            .filter(|(_, model)| query.is_empty() || model.to_lowercase().contains(&query_lower))
            .map(|(index, model)| StringMatch {
                candidate_id: index,
                string: model.to_string(),
                positions: Vec::new(),
                score: 0.0,
            })
            .collect();

        self.selected_index = 0;
        cx.notify();

        Task::ready(())
    }

    /// 确认选择模型
    fn confirm(
        &mut self,
        _secondary: bool,
        window: &mut Window,
        cx: &mut Context<OllamaModelPicker>,
    ) {
        let Some(model_match) = self.filtered_models.get(self.selected_index) else {
            return;
        };

        (self.on_model_changed)(model_match.string.clone().into(), window, cx);
        cx.emit(DismissEvent);
    }

    /// 关闭选择器
    fn dismissed(&mut self, window: &mut Window, cx: &mut Context<OllamaModelPicker>) {
        cx.defer_in(window, |picker, window, cx| {
            picker.set_query("", window, cx);
        });
        cx.emit(DismissEvent);
    }

    /// 渲染单个模型选项
    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _window: &mut Window,
        _cx: &mut Context<OllamaModelPicker>,
    ) -> Option<Self::ListItem> {
        let model_match = self.filtered_models.get(ix)?;

        Some(
            ListItem::new(ix)
                .inset(true)
                .spacing(ListItemSpacing::Sparse)
                .toggle_state(selected)
                .child(Label::new(model_match.string.clone()))
                .into_any_element(),
        )
    }
}

/// 渲染Ollama模型选择器
pub fn render_ollama_model_picker(
    field: SettingField<settings::OllamaModelName>,
    file: SettingsUiFile,
    _metadata: Option<&SettingsFieldMetadata>,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let (_, value) = SettingsStore::global(cx).get_value_from_file(file.to_settings(), field.pick);
    let current_value: SharedString = value
        .map(|m| m.0.clone().into())
        .unwrap_or_else(|| "".into());

    PopoverMenu::new("ollama-model-picker")
        .trigger(render_picker_trigger_button(
            "ollama_model_picker_trigger".into(),
            if current_value.is_empty() {
                "选择模型…".into()
            } else {
                current_value.clone()
            },
        ))
        .menu(move |window, cx| {
            Some(cx.new(|cx| {
                let file = file.clone();
                let current_value = current_value.clone();
                let delegate = OllamaModelPickerDelegate::new(
                    current_value,
                    move |model_name, window, cx| {
                        update_settings_file(
                            file.clone(),
                            field.json_path,
                            window,
                            cx,
                            move |settings, app| {
                                (field.write)(
                                    settings,
                                    Some(settings::OllamaModelName(model_name.to_string())),
                                    app,
                                );
                            },
                        )
                        .log_err();
                    },
                    cx,
                );

                Picker::uniform_list(delegate, window, cx)
                    .show_scrollbar(true)
                    .width(rems_from_px(210.))
                    .max_height(Some(rems(18.).into()))
            }))
        })
        .anchor(gpui::Anchor::TopLeft)
        .offset(gpui::Point {
            x: px(0.0),
            y: px(2.0),
        })
        .with_handle(ui::PopoverMenuHandle::default())
        .into_any_element()
}