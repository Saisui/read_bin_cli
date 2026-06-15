/// 简易 .tui 文件解析器
///
/// 解析 <template> + <style> + <script> 块，生成 Token 结构。
/// 不依赖 syn/quote，纯字符串操作。
pub struct TuiFile {
    pub template: Vec<TemplateNode>,
    pub style_rules: Vec<StyleRule>,
    pub script_body: Option<String>,
}

pub enum TemplateNode {
    Element(Element),
    Text(String),
    Interpolation(String),
    ForLoop { var: String, iter: String, body: Vec<TemplateNode> },
    IfCond { expr: String, body: Vec<TemplateNode> },
}

pub struct Element {
    pub tag: String,
    pub classes: Vec<String>,
    pub self_closing: bool,
    pub children: Vec<TemplateNode>,
}

pub struct StyleRule {
    pub selector: String,
    pub properties: Vec<(String, String)>,
}

/// 解析 .tui 文件内容
pub fn parse(content: &str) -> Result<TuiFile, String> {
    let mut template = Vec::new();
    let mut style_rules = Vec::new();
    let mut script_body = None;

    // 提取 <template>...</template>
    if let Some(t_start) = content.find("<template>") {
        let t_end = content.find("</template>").ok_or("Missing </template>")?;
        let t_body = &content[t_start + 10..t_end];
        template = parse_template_content(t_body)?;
    }

    // 提取 <style>...</style>
    if let Some(s_start) = content.find("<style>") {
        let s_end = content.find("</style>").ok_or("Missing </style>")?;
        let s_body = &content[s_start + 7..s_end];
        style_rules = parse_style_content(s_body);
    }

    // 提取 <script>...</script>
    if let Some(sc_start) = content.find("<script") {
        let sc_tag_end = content[sc_start..].find('>').ok_or("Missing > in <script>")?;
        let sc_body_start = sc_start + sc_tag_end + 1;
        let sc_end = content[sc_body_start..].find("</script>").ok_or("Missing </script>")?;
        script_body = Some(content[sc_body_start..sc_body_start + sc_end].to_string());
    }

    Ok(TuiFile { template, style_rules, script_body })
}

/// 解析模板内容为节点列表
fn parse_template_content(body: &str) -> Result<Vec<TemplateNode>, String> {
    let mut nodes = Vec::new();
    let chars: Vec<char> = body.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        // 跳过空白
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }

        // {{ ... }} 插值
        if i + 1 < chars.len() && chars[i] == '{' && chars[i + 1] == '{' {
            i += 2;
            let start = i;
            while i + 1 < chars.len() && !(chars[i] == '}' && chars[i + 1] == '}') {
                i += 1;
            }
            let expr = chars[start..i].iter().collect::<String>().trim().to_string();
            i += 2; // 跳过 }}
            nodes.push(TemplateNode::Interpolation(expr));
            continue;
        }

        // <Tag ...> 或 <Tag .../>
        if chars[i] == '<' && i + 1 < chars.len() && chars[i + 1] != '/' {
            i += 1;
            let tag_start = i;
            while i < chars.len() && !chars[i].is_whitespace() && chars[i] != '>' && chars[i] != '/' {
                i += 1;
            }
            let tag = chars[tag_start..i].iter().collect::<String>();

            // 解析属性
            let mut classes = Vec::new();
            let mut self_closing = false;
            while i < chars.len() && chars[i] != '>' {
                if chars[i] == '/' && i + 1 < chars.len() && chars[i + 1] == '>' {
                    self_closing = true;
                    i += 2;
                    break;
                }
                if chars[i].is_whitespace() {
                    i += 1;
                    continue;
                }
                // class="..."
                if i + 5 < chars.len() && chars[i..i+6].iter().collect::<String>() == "class=" {
                    i += 7; // 跳过 class="
                    let cls_start = i;
                    while i < chars.len() && chars[i] != '"' {
                        i += 1;
                    }
                    let cls = chars[cls_start..i].iter().collect::<String>();
                    classes.extend(cls.split_whitespace().map(String::from));
                    i += 1; // 跳过 "
                    continue;
                }
                // 跳过其他属性
                i += 1;
            }

            if !self_closing && i < chars.len() && chars[i] == '>' {
                i += 1; // 跳过 >
            }

            // 解析子节点直到 </Tag>
            let mut children = Vec::new();
            if !self_closing {
                let end_tag = format!("</{}>", tag);
                while i < chars.len() {
                    if i + end_tag.len() <= chars.len()
                        && chars[i..i + end_tag.len()].iter().collect::<String>() == end_tag
                    {
                        i += end_tag.len();
                        break;
                    }
                    // 递归解析子节点
                    if chars[i].is_whitespace() {
                        i += 1;
                        continue;
                    }
                    if i + 1 < chars.len() && chars[i] == '{' && chars[i + 1] == '{' {
                        i += 2;
                        let start = i;
                        while i + 1 < chars.len() && !(chars[i] == '}' && chars[i + 1] == '}') {
                            i += 1;
                        }
                        let expr = chars[start..i].iter().collect::<String>().trim().to_string();
                        i += 2;
                        children.push(TemplateNode::Interpolation(expr));
                    } else if chars[i] == '<' && i + 1 < chars.len() && chars[i + 1] != '/' {
                        // 嵌套元素
                        let (child, new_i) = parse_element(&chars, i)?;
                        children.push(child);
                        i = new_i;
                    } else {
                        // 文本
                        let text_start = i;
                        while i < chars.len() && chars[i] != '<' && !(i + 1 < chars.len() && chars[i] == '{' && chars[i + 1] == '{') {
                            i += 1;
                        }
                        let text: String = chars[text_start..i].iter().collect();
                        let text = text.trim().to_string();
                        if !text.is_empty() {
                            children.push(TemplateNode::Text(text));
                        }
                    }
                }
            }

            nodes.push(TemplateNode::Element(Element {
                tag,
                classes,
                self_closing,
                children,
            }));
            continue;
        }

        // 文本
        let text_start = i;
        while i < chars.len() && chars[i] != '<' && !(i + 1 < chars.len() && chars[i] == '{' && chars[i + 1] == '{') {
            i += 1;
        }
        let text: String = chars[text_start..i].iter().collect();
        let text = text.trim().to_string();
        if !text.is_empty() {
            nodes.push(TemplateNode::Text(text));
        }
    }

    Ok(nodes)
}

/// 解析单个元素（用于嵌套）
fn parse_element(chars: &[char], start: usize) -> Result<(TemplateNode, usize), String> {
    let mut i = start + 1; // 跳过 <
    let tag_start = i;
    while i < chars.len() && !chars[i].is_whitespace() && chars[i] != '>' && chars[i] != '/' {
        i += 1;
    }
    let tag = chars[tag_start..i].iter().collect::<String>();

    let mut classes = Vec::new();
    let mut self_closing = false;
    while i < chars.len() && chars[i] != '>' {
        if chars[i] == '/' && i + 1 < chars.len() && chars[i + 1] == '>' {
            self_closing = true;
            i += 2;
            break;
        }
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        if i + 5 < chars.len() && chars[i..i+6].iter().collect::<String>() == "class=" {
            i += 7;
            let cls_start = i;
            while i < chars.len() && chars[i] != '"' {
                i += 1;
            }
            let cls = chars[cls_start..i].iter().collect::<String>();
            classes.extend(cls.split_whitespace().map(String::from));
            i += 1;
            continue;
        }
        i += 1;
    }

    if !self_closing && i < chars.len() && chars[i] == '>' {
        i += 1;
    }

    let mut children = Vec::new();
    if !self_closing {
        let end_tag = format!("</{}>", tag);
        while i < chars.len() {
            if i + end_tag.len() <= chars.len()
                && chars[i..i + end_tag.len()].iter().collect::<String>() == end_tag
            {
                i += end_tag.len();
                break;
            }
            if chars[i].is_whitespace() {
                i += 1;
                continue;
            }
            if i + 1 < chars.len() && chars[i] == '{' && chars[i + 1] == '{' {
                i += 2;
                let expr_start = i;
                while i + 1 < chars.len() && !(chars[i] == '}' && chars[i + 1] == '}') {
                    i += 1;
                }
                let expr = chars[expr_start..i].iter().collect::<String>().trim().to_string();
                i += 2;
                children.push(TemplateNode::Interpolation(expr));
            } else {
                let text_start = i;
                while i < chars.len() && chars[i] != '<' && !(i + 1 < chars.len() && chars[i] == '{' && chars[i + 1] == '{') {
                    i += 1;
                }
                let text: String = chars[text_start..i].iter().collect();
                let text = text.trim().to_string();
                if !text.is_empty() {
                    children.push(TemplateNode::Text(text));
                }
            }
        }
    }

    Ok((TemplateNode::Element(Element { tag, classes, self_closing, children }), i))
}

/// 从 TuiFile 生成 Rust 代码
pub fn generate(tui: &TuiFile) -> String {
    let mut code = String::new();

    // 生成样式查询函数
    code.push_str("use ratatui::style::{Color, Style};\n\n");
    code.push_str("pub fn style_resolve(class: &str) -> Style {\n");
    code.push_str("    match class {\n");
    for rule in &tui.style_rules {
        let selector = rule.selector.trim_start_matches('.');
        let style_expr = gen_style_expr(&rule.properties);
        code.push_str(&format!("        \"{}\" => {},\n", selector, style_expr));
    }
    code.push_str("        _ => Style::default(),\n");
    code.push_str("    }\n}\n\n");

    // 生成 render 函数占位
    code.push_str("pub fn render(_f: &mut ratatui::Frame, _app: &crate::app::App, _data: &[u8], _area: ratatui::layout::Rect) {\n");
    code.push_str("    // TODO: 从模板生成渲染代码\n");
    code.push_str("}\n\n");

    // 透传 <script> 中的 Rust 代码
    if let Some(script) = &tui.script_body {
        code.push_str("// ─── script ──────────────────────────────────────────────\n");
        code.push_str("use crate::app::{App, InputMode};\n");
        code.push_str(script);
        code.push_str("\n");
    }

    code
}

/// 从样式属性生成 Style 表达式字符串
fn gen_style_expr(props: &[(String, String)]) -> String {
    let mut parts = vec!["Style::default()".to_string()];

    for (key, val) in props {
        match key.as_str() {
            "fg" => {
                let color = gen_color_expr(val);
                parts.push(format!(".fg({})", color));
            }
            "bg" => {
                let color = gen_color_expr(val);
                parts.push(format!(".bg({})", color));
            }
            "bold" => {
                parts.push(".add_modifier(ratatui::style::Modifier::BOLD)".to_string());
            }
            "dim" => {
                parts.push(".add_modifier(ratatui::style::Modifier::DIM)".to_string());
            }
            _ => {}
        }
    }

    parts.join("")
}

/// 从颜色值生成 Color 表达式字符串
fn gen_color_expr(val: &str) -> String {
    match val {
        "auto" => "crate::color_config::AUTO_FG_SENTINEL".to_string(),
        "black" => "Color::Black".to_string(),
        "red" => "Color::Red".to_string(),
        "green" => "Color::Green".to_string(),
        "yellow" => "Color::Yellow".to_string(),
        "blue" => "Color::Blue".to_string(),
        "magenta" => "Color::Magenta".to_string(),
        "cyan" => "Color::Cyan".to_string(),
        "white" => "Color::White".to_string(),
        "darkgray" | "dark_gray" => "Color::DarkGray".to_string(),
        _ => {
            if val.starts_with('[') && val.ends_with(']') {
                let inner = &val[1..val.len() - 1];
                let parts: Vec<&str> = inner.split(',').map(|s| s.trim()).collect();
                if parts.len() == 3 {
                    format!("Color::Rgb({}, {}, {})", parts[0], parts[1], parts[2])
                } else {
                    "Color::default()".to_string()
                }
            } else {
                "Color::default()".to_string()
            }
        }
    }
}

/// 解析样式内容
fn parse_style_content(body: &str) -> Vec<StyleRule> {
    let mut rules = Vec::new();
    let lines: Vec<&str> = body.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i].trim();
        if line.is_empty() || line.starts_with("//") {
            i += 1;
            continue;
        }

        // .selector { ... }
        if line.starts_with('.') {
            let selector = line.split('{').next().unwrap_or("").trim().to_string();
            i += 1;
            let mut properties = Vec::new();

            while i < lines.len() {
                let prop_line = lines[i].trim();
                if prop_line.starts_with('}') || prop_line.is_empty() {
                    i += 1;
                    break;
                }
                if let Some((key, val)) = prop_line.split_once(':') {
                    properties.push((key.trim().to_string(), val.trim().trim_end_matches(',').to_string()));
                }
                i += 1;
            }

            rules.push(StyleRule { selector, properties });
            continue;
        }

        i += 1;
    }

    rules
}

fn main() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let ui_dir = std::path::Path::new("ui");

    if !ui_dir.exists() {
        return;
    }

    let entries: Vec<_> = std::fs::read_dir(ui_dir)
        .expect("Failed to read ui/ directory")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map_or(false, |ext| ext == "tui"))
        .collect();

    for entry in &entries {
        let path = entry.path();
        let stem = path.file_stem().unwrap().to_str().unwrap();
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("Failed to read {}: {}", path.display(), e));

        let tui = parse(&content)
            .unwrap_or_else(|e| panic!("Failed to parse {}: {}", path.display(), e));

        let code = generate(&tui);

        let out_path = std::path::Path::new(&out_dir).join(format!("{}.rs", stem));
        std::fs::write(&out_path, &code)
            .unwrap_or_else(|e| panic!("Failed to write {}: {}", out_path.display(), e));

        println!("cargo:rerun-if-changed={}", path.display());
    }

    println!("cargo:rerun-if-changed=ui/");
}
