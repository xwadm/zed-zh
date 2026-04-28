use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    LitInt, Token, parse::Parse, parse::ParseStream, parse_macro_input, punctuated::Punctuated,
};

/// 动态间距输入结构体，解析宏参数
struct DynamicSpacingInput {
    values: Punctuated<DynamicSpacingValue, Token![,]>,
}

/// 动态间距值类型
///
/// 宏输入为一组数值列表
///
/// 当输入单个数值时，使用标准间距公式生成对应的间距值
///
/// 当输入三元组数值时，直接使用这三个数值作为间距值
enum DynamicSpacingValue {
    /// 单个数值（自动计算紧凑/默认/舒适间距）
    Single(LitInt),
    /// 三元组数值（紧凑/默认/舒适）
    Tuple(LitInt, LitInt, LitInt),
}

impl Parse for DynamicSpacingInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        Ok(DynamicSpacingInput {
            values: input.parse_terminated(DynamicSpacingValue::parse, Token![,])?,
        })
    }
}

impl Parse for DynamicSpacingValue {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        if input.peek(syn::token::Paren) {
            let content;
            syn::parenthesized!(content in input);
            let a: LitInt = content.parse()?;
            content.parse::<Token![,]>()?;
            let b: LitInt = content.parse()?;
            content.parse::<Token![,]>()?;
            let c: LitInt = content.parse()?;
            Ok(DynamicSpacingValue::Tuple(a, b, c))
        } else {
            Ok(DynamicSpacingValue::Single(input.parse()?))
        }
    }
}

/// 为 DynamicSpacing 枚举自动生成间距方法
pub fn derive_spacing(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DynamicSpacingInput);

    let spacing_ratios: Vec<_> = input
        .values
        .iter()
        .map(|v| {
            let variant = match v {
                DynamicSpacingValue::Single(n) => {
                    format_ident!("Base{:02}", n.base10_parse::<u32>().unwrap())
                }
                DynamicSpacingValue::Tuple(_, b, _) => {
                    format_ident!("Base{:02}", b.base10_parse::<u32>().unwrap())
                }
            };
            match v {
                DynamicSpacingValue::Single(n) => {
                    let n = n.base10_parse::<f32>().unwrap();
                    quote! {
                        DynamicSpacing::#variant => match ::theme::theme_settings(cx).ui_density(cx) {
                            ::theme::UiDensity::Compact => (#n - 4.0).max(0.0) / BASE_REM_SIZE_IN_PX,
                            ::theme::UiDensity::Default => #n / BASE_REM_SIZE_IN_PX,
                            ::theme::UiDensity::Comfortable => (#n + 4.0) / BASE_REM_SIZE_IN_PX,
                        }
                    }
                }
                DynamicSpacingValue::Tuple(a, b, c) => {
                    let a = a.base10_parse::<f32>().unwrap();
                    let b = b.base10_parse::<f32>().unwrap();
                    let c = c.base10_parse::<f32>().unwrap();
                    quote! {
                        DynamicSpacing::#variant => match ::theme::theme_settings(cx).ui_density(cx) {
                            ::theme::UiDensity::Compact => #a / BASE_REM_SIZE_IN_PX,
                            ::theme::UiDensity::Default => #b / BASE_REM_SIZE_IN_PX,
                            ::theme::UiDensity::Comfortable => #c / BASE_REM_SIZE_IN_PX,
                        }
                    }
                }
            }
        })
        .collect();

    let (variant_names, doc_strings): (Vec<_>, Vec<_>) = input
        .values
        .iter()
        .map(|v| {
            let variant = match v {
                DynamicSpacingValue::Single(n) => {
                    format_ident!("Base{:02}", n.base10_parse::<u32>().unwrap())
                }
                DynamicSpacingValue::Tuple(_, b, _) => {
                    format_ident!("Base{:02}", b.base10_parse::<u32>().unwrap())
                }
            };
            let doc_string = match v {
                DynamicSpacingValue::Single(n) => {
                    let n = n.base10_parse::<f32>().unwrap();
                    let compact = (n - 4.0).max(0.0);
                    let comfortable = n + 4.0;
                    format!(
                        "`{}px`|`{}px`|`{}px (以16px/rem为基准)` - 随用户的rem尺寸自动缩放",
                        compact, n, comfortable
                    )
                }
                DynamicSpacingValue::Tuple(a, b, c) => {
                    let a = a.base10_parse::<f32>().unwrap();
                    let b = b.base10_parse::<f32>().unwrap();
                    let c = c.base10_parse::<f32>().unwrap();
                    format!(
                        "`{}px`|`{}px`|`{}px (以16px/rem为基准)` - 随用户的rem尺寸自动缩放",
                        a, b, c
                    )
                }
            };
            (quote!(#variant), quote!(#doc_string))
        })
        .unzip();

    let expanded = quote! {
        /// 动态间距系统：根据界面密度（紧凑/默认/舒适）自动调整间距大小
        ///
        /// "Base" 后的数字表示默认 rem 尺寸和间距设置下的基础像素大小
        ///
        /// 在需要动态间距的场景，优先使用 DynamicSpacing，而非手动或固定间距值
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum DynamicSpacing {
            #(
                #[doc = #doc_strings]
                #variant_names,
            )*
        }

        impl DynamicSpacing {
            /// 获取间距比例，仅内部使用
            fn spacing_ratio(&self, cx: &App) -> f32 {
                const BASE_REM_SIZE_IN_PX: f32 = 16.0;
                match self {
                    #(#spacing_ratios,)*
                }
            }

            /// 获取 rem 单位的间距值
            pub fn rems(&self, cx: &App) -> Rems {
                rems(self.spacing_ratio(cx))
            }

            /// 获取像素单位的间距值
            pub fn px(&self, cx: &App) -> Pixels {
                let ui_font_size_f32: f32 = ::theme::theme_settings(cx).ui_font_size(cx).into();
                px(ui_font_size_f32 * self.spacing_ratio(cx))
            }
        }
    };

    TokenStream::from(expanded)
}