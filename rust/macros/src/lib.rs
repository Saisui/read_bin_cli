extern crate proc_macro;
use proc_macro::TokenStream;

mod parser;
mod codegen;

/// include_tui!("ui/statusbar.tui") — 编译时读取 .tui 文件并生成代码
#[proc_macro]
pub fn include_tui(input: TokenStream) -> TokenStream {
    // 手动解析字符串字面量，不依赖 syn
    let input_str = input.to_string();
    let path_str = input_str.trim().trim_matches('"').trim_matches('\'');
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR not set");
    let full_path = std::path::Path::new(&manifest_dir).join(path_str);

    let content = std::fs::read_to_string(&full_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", full_path.display(), e));

    let tui = parser::parse_tui_str(&content)
        .unwrap_or_else(|e| panic!("Failed to parse {}: {}", full_path.display(), e));

    let code = codegen::gen_tui_file(&tui);
    code.into()
}

/// html!{ <Component /> } — 编译时展开为组件调用
#[proc_macro]
pub fn html(input: TokenStream) -> TokenStream {
    let input_str = input.to_string();
    let component = parser::parse_component_ref_str(&input_str)
        .expect("Failed to parse component reference");
    let code = codegen::gen_component_call(&component);
    code.into()
}
