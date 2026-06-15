/// .tui 文件解析器
///
/// 将 .tui 文件（类 Vue SFC 格式）解析为 AST。
/// 使用自定义 lexer 而非 syn::Parse，原因：
/// 1. `<template>` 被 Rust tokenizer 解析为比较运算符
/// 2. `{{ }}` 中的 `}}` 触发 syn delimiter 匹配错误
/// 3. 自定义 lexer 直接操作 Vec<char>，无此问题
///
/// 支持的语法：
/// - `<template>` / `<style>` / `<script lang="rust">` 三段式
/// - HTML-like 标签：`<Text class="x">`, `<Statusbar>`, `<Popup>` 等
/// - 模板语法：`{{ expr }}`, `v-if`, `{{ for x in iter }}`
/// - 样式选择器：`.class { fg: auto, bg: red }`, `.class:pseudo { ... }`
/// - 事件绑定：`@click="handler"`, `@scroll="handler"`

use proc_macro2::TokenStream;
use quote::quote;

// ─── Types ──────────────────────────────────────────────────

pub struct TuiFile {
    pub template: TemplateBlock,
    pub style: Option<StyleBlock>,
    pub script: Option<ScriptBlock>,
}

pub struct TemplateBlock {
    pub nodes: Vec<TemplateNode>,
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
    pub props: Vec<Prop>,
    pub events: Vec<EventBinding>,
    pub children: Vec<TemplateNode>,
    pub self_closing: bool,
}

pub struct Prop {
    pub name: String,
    pub value: String,
}

pub struct EventBinding {
    pub name: String,
    pub handler: String,
}

pub struct StyleBlock {
    pub rules: Vec<StyleRule>,
}

pub struct StyleRule {
    pub selector: String,
    pub properties: Vec<StyleProp>,
}

pub struct StyleProp {
    pub name: String,
    pub value: String,
}

pub struct ScriptBlock {
    pub lang: Option<String>,
    pub body: String,
}

pub struct ComponentRef {
    pub tag: String,
}

// ─── 字符级 Lexer ──────────────────────────────────────────
/// 自定义 lexer：逐字符扫描 .tui 文件内容
///
/// 不依赖 syn 的 ParseStream（因为 < > { } 等 delimiter
/// 在 ParseStream 上有自动匹配行为，会导致 {{ }} 解析失败）。
///
/// 核心设计：
/// - chars: Vec<char> — 将输入转为字符数组
/// - pos: usize — 当前扫描位置
/// - peek/peek2 — 查看当前/下一个字符（不消费）
/// - advance — 消费一个字符并返回
/// - read_xxx — 读取特定格式的 token（标识符、字符串、花括号内容等）

struct Lexer {
    chars: Vec<char>,
    pos: usize,
}

impl Lexer {
    fn new(s: &str) -> Self {
        Self { chars: s.chars().collect(), pos: 0 }
    }

    fn is_empty(&self) -> bool { self.pos >= self.chars.len() }

    fn peek(&self) -> Option<char> { self.chars.get(self.pos).copied() }

    fn peek2(&self) -> Option<char> { self.chars.get(self.pos + 1).copied() }

    fn advance(&mut self) -> Option<char> {
        let c = self.chars.get(self.pos).copied();
        if c.is_some() { self.pos += 1; }
        c
    }

    fn skip_whitespace(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_whitespace() { self.advance(); } else { break; }
        }
    }

    fn read_until(&mut self, ch: char) -> String {
        let mut s = String::new();
        while let Some(c) = self.advance() {
            if c == ch { break; }
            s.push(c);
        }
        s
    }

    fn read_ident(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' || c == '-' || c == '.' {
                s.push(c);
                self.advance();
            } else {
                break;
            }
        }
        s
    }

    fn read_string_literal(&mut self) -> String {
        if self.peek() == Some('"') {
            self.advance(); // "
            let s = self.read_until('"');
            s
        } else {
            self.read_ident()
        }
    }

    fn read_until_str(&mut self, target: &str) -> String {
        let mut s = String::new();
        let target_chars: Vec<char> = target.chars().collect();
        while !self.is_empty() {
            if self.peek() == Some(target_chars[0]) {
                let save = self.pos;
                let mut matched = true;
                for &tc in &target_chars[1..] {
                    self.advance();
                    if self.peek() != Some(tc) {
                        matched = false;
                        break;
                    }
                }
                if matched {
                    self.pos = save + target_chars.len();
                    return s;
                }
                self.pos = save;
            }
            s.push(self.advance().unwrap());
        }
        s
    }

    fn read_brace_content(&mut self) -> String {
        // 假设当前 pos 在 { 之后
        let mut depth = 1u32;
        let mut s = String::new();
        while let Some(c) = self.advance() {
            match c {
                '{' => { depth += 1; s.push(c); }
                '}' => {
                    depth -= 1;
                    if depth == 0 { break; }
                    s.push(c);
                }
                _ => s.push(c),
            }
        }
        s
    }

    fn read_angle_content(&mut self) -> String {
        // 读取 < > 之间的内容（不含标签本身）
        let mut depth = 1u32;
        let mut s = String::new();
        while let Some(c) = self.advance() {
            match c {
                '<' => {
                    if self.peek() == Some('/') {
                        depth -= 1;
                        if depth == 0 {
                            // 跳过 </Tag>
                            self.advance(); // /
                            while let Some(tc) = self.peek() {
                                if tc == '>' { self.advance(); break; }
                                self.advance();
                            }
                            break;
                        }
                    }
                    depth += 1;
                    s.push(c);
                }
                '>' => {
                    s.push(c);
                }
                _ => s.push(c),
            }
        }
        s
    }
}

// ─── 顶层解析 ──────────────────────────────────────────────
/// 解析 .tui 文件字符串为 TuiFile AST
///
/// 支持两种 section 语法：
/// 1. `<template>` ... `</template>`（HTML 风格）
/// 2. `template { ... }`（Rust 风格）
///
/// 返回包含 template（必需）、style（可选）、script（可选）的 TuiFile

pub fn parse_tui_str(content: &str) -> Result<TuiFile, String> {
    let mut lx = Lexer::new(content);
    let mut template = None;
    let mut style = None;
    let mut script = None;

    while !lx.is_empty() {
        lx.skip_whitespace();
        if lx.is_empty() { break; }

        // 支持 <template> 和 template 两种语法
        let section = if lx.peek() == Some('<') && lx.peek2().map_or(false, |c| c.is_alphabetic()) {
            lx.advance(); // <
            let name = lx.read_ident();
            lx.skip_whitespace();
            // 跳过属性
            while !lx.is_empty() && lx.peek() != Some('>') && lx.peek() != Some('/') {
                lx.advance();
            }
            if lx.peek() == Some('/') { lx.advance(); }
            if lx.peek() == Some('>') { lx.advance(); }
            name
        } else {
            lx.read_ident()
        };

        lx.skip_whitespace();

        match section.as_str() {
            "template" => {
                lx.skip_whitespace();
                if lx.peek() == Some('{') {
                    lx.advance();
                    let body = lx.read_brace_content();
                    let mut inner_lx = Lexer::new(&body);
                    template = Some(parse_template_block(&mut inner_lx)?);
                } else if lx.peek() == Some('<') {
                    lx.advance();
                    while let Some(c) = lx.advance() {
                        if c == '>' { break; }
                    }
                    let body = lx.read_angle_content();
                    let mut inner_lx = Lexer::new(&body);
                    template = Some(parse_template_block(&mut inner_lx)?);
                }
            }
            "style" => {
                lx.skip_whitespace();
                if lx.peek() == Some('{') {
                    lx.advance();
                    let body = lx.read_brace_content();
                    let mut inner_lx = Lexer::new(&body);
                    style = Some(parse_style_block(&mut inner_lx)?);
                } else if lx.peek() == Some('<') {
                    lx.advance();
                    while let Some(c) = lx.advance() {
                        if c == '>' { break; }
                    }
                    let body = lx.read_angle_content();
                    let mut inner_lx = Lexer::new(&body);
                    style = Some(parse_style_block(&mut inner_lx)?);
                }
            }
            "script" => {
                lx.skip_whitespace();
                // 可选 [lang="rust"] 或 lang="rust"
                let lang = if lx.peek() == Some('[') {
                    lx.advance();
                    let _name = lx.read_ident();
                    lx.skip_whitespace();
                    if lx.peek() == Some('=') { lx.advance(); }
                    lx.skip_whitespace();
                    let val = lx.read_string_literal();
                    lx.skip_whitespace();
                    if lx.peek() == Some(']') { lx.advance(); }
                    Some(val)
                } else {
                    None
                };
                lx.skip_whitespace();
                if lx.peek() == Some('{') {
                    lx.advance();
                    let body = lx.read_brace_content();
                    script = Some(ScriptBlock { lang, body });
                } else if lx.peek() == Some('<') {
                    lx.advance();
                    while let Some(c) = lx.advance() {
                        if c == '>' { break; }
                    }
                    let body = lx.read_angle_content();
                    script = Some(ScriptBlock { lang, body });
                }
            }
            "" => break,
            _ => return Err(format!("Expected 'template', 'style', or 'script', got '{}'", section)),
        }
    }

    Ok(TuiFile {
        template: template.ok_or("Missing <template> block")?,
        style,
        script,
    })
}

// ─── Template parsing ───────────────────────────────────────

fn parse_template_block(lx: &mut Lexer) -> Result<TemplateBlock, String> {
    let mut nodes = Vec::new();
    while !lx.is_empty() {
        nodes.push(parse_template_node(lx)?);
    }
    Ok(TemplateBlock { nodes })
}

fn parse_template_node(lx: &mut Lexer) -> Result<TemplateNode, String> {
    lx.skip_whitespace();
    if lx.is_empty() {
        return Err("Unexpected EOF in template".into());
    }

    // {{ ... }}
    if lx.peek() == Some('{') && lx.peek2() == Some('{') {
        return parse_brace_block(lx);
    }

    // < 元素
    if lx.peek() == Some('<') && lx.peek2().map_or(false, |c| c.is_alphabetic()) {
        return parse_element(lx);
    }

    // 文本
    let mut text = String::new();
    while let Some(c) = lx.peek() {
        if c == '<' || (c == '{' && lx.peek2() == Some('{')) {
            break;
        }
        text.push(c);
        lx.advance();
    }
    Ok(TemplateNode::Text(text.trim().to_string()))
}

fn parse_element(lx: &mut Lexer) -> Result<TemplateNode, String> {
    lx.advance(); // <
    let tag = lx.read_ident();
    lx.skip_whitespace();

    let mut classes = Vec::new();
    let mut props = Vec::new();
    let mut events = Vec::new();

    loop {
        lx.skip_whitespace();
        let next = lx.peek().ok_or("Unexpected EOF in element")?;
        if next == '>' || next == '/' { break; }

        if next == '@' {
            lx.advance(); // @
            let event_name = lx.read_ident();
            lx.skip_whitespace();
            if lx.peek() == Some('=') { lx.advance(); }
            lx.skip_whitespace();
            let handler = lx.read_string_literal();
            events.push(EventBinding { name: event_name, handler });
        } else if next == ':' {
            lx.advance(); // :
            let prop_name = lx.read_ident();
            lx.skip_whitespace();
            if lx.peek() == Some('=') { lx.advance(); }
            lx.skip_whitespace();
            let value = lx.read_string_literal();
            props.push(Prop { name: prop_name, value });
        } else if next.is_alphabetic() {
            let attr_name = lx.read_ident();
            lx.skip_whitespace();
            if lx.peek() == Some('=') { lx.advance(); }
            lx.skip_whitespace();
            let value = lx.read_string_literal();
            if attr_name == "class" {
                classes.extend(value.split_whitespace().map(|s| s.to_string()));
            } else {
                props.push(Prop { name: attr_name, value });
            }
        } else {
            break;
        }
    }

    lx.skip_whitespace();

    // 自闭合
    if lx.peek() == Some('/') {
        lx.advance(); // /
        if lx.peek() == Some('>') { lx.advance(); }
        return Ok(TemplateNode::Element(Element {
            tag, classes, props, events,
            children: Vec::new(), self_closing: true,
        }));
    }

    if lx.peek() == Some('>') { lx.advance(); }

    // 子节点
    let mut children = Vec::new();
    loop {
        lx.skip_whitespace();
        if lx.is_empty() { return Err(format!("Unclosed <{}>", tag)); }

        // 检查 </Tag>
        if lx.peek() == Some('<') && lx.peek2() == Some('/') {
            break;
        }

        children.push(parse_template_node(lx)?);
    }

    // </Tag>
    lx.advance(); // <
    lx.advance(); // /
    let close_tag = lx.read_ident();
    if close_tag != tag {
        return Err(format!("Mismatched: expected </{}>, got </{}>", tag, close_tag));
    }
    lx.skip_whitespace();
    if lx.peek() == Some('>') { lx.advance(); }

    Ok(TemplateNode::Element(Element {
        tag, classes, props, events,
        children, self_closing: false,
    }))
}

fn parse_brace_block(lx: &mut Lexer) -> Result<TemplateNode, String> {
    lx.advance(); // {
    lx.advance(); // {

    // 收集内容直到 }}
    let mut inner = String::new();
    let mut depth = 0u32;
    loop {
        let c = lx.advance().ok_or("Unclosed {{")?;
        if c == '{' {
            depth += 1;
            inner.push(c);
        } else if c == '}' {
            if depth > 0 {
                depth -= 1;
                inner.push(c);
            } else {
                // 可能是 }}
                let c2 = lx.peek().ok_or("Unclosed {{")?;
                if c2 == '}' {
                    lx.advance();
                    break;
                }
                inner.push(c);
            }
        } else {
            inner.push(c);
        }
    }

    let inner = inner.trim().to_string();

    // {{ for x in iter }}
    if inner.starts_with("for ") {
        let rest = &inner[4..];
        if let Some(pos) = rest.find(" in ") {
            let var = rest[..pos].trim().to_string();
            let iter = rest[pos + 4..].trim().to_string();

            let mut body = Vec::new();
            while !lx.is_empty() {
                lx.skip_whitespace();
                if lx.peek() == Some('{') && lx.peek2() == Some('{') {
                    // 检查是否是 {{ /for }}
                    let save = lx.pos;
                    lx.advance(); lx.advance();
                    let mut check = String::new();
                    while !lx.is_empty() && lx.peek() != Some('}') {
                        check.push(lx.advance().unwrap());
                    }
                    lx.pos = save;
                    if check.trim() == "/for" {
                        // 跳过 {{ /for }}
                        lx.advance(); lx.advance();
                        while !lx.is_empty() && lx.peek() != Some('}') {
                            lx.advance();
                        }
                        lx.advance(); // }
                        lx.advance(); // }
                        break;
                    }
                }
                body.push(parse_template_node(lx)?);
            }
            return Ok(TemplateNode::ForLoop { var, iter, body });
        }
    }

    // {{ if expr }}
    if inner.starts_with("if ") {
        let expr = inner[3..].trim().to_string();
        let mut body = Vec::new();
        while !lx.is_empty() {
            lx.skip_whitespace();
            if lx.peek() == Some('{') && lx.peek2() == Some('{') {
                let save = lx.pos;
                lx.advance(); lx.advance();
                let mut check = String::new();
                while !lx.is_empty() && lx.peek() != Some('}') {
                    check.push(lx.advance().unwrap());
                }
                lx.pos = save;
                if check.trim() == "/if" {
                    lx.advance(); lx.advance();
                    while !lx.is_empty() && lx.peek() != Some('}') {
                        lx.advance();
                    }
                    lx.advance(); lx.advance();
                    break;
                }
            }
            body.push(parse_template_node(lx)?);
        }
        return Ok(TemplateNode::IfCond { expr, body });
    }

    // 普通插值 {{ expr }}
    Ok(TemplateNode::Interpolation(inner))
}

// ─── Style parsing ──────────────────────────────────────────

fn parse_style_block(lx: &mut Lexer) -> Result<StyleBlock, String> {
    let mut rules = Vec::new();
    while !lx.is_empty() {
        lx.skip_whitespace();
        if lx.is_empty() { break; }

        if lx.peek() != Some('.') {
            // 跳过非规则行
            lx.read_until('\n');
            continue;
        }

        lx.advance(); // .
        let selector_name = lx.read_ident();
        let mut sel_str = format!(".{}", selector_name);

        lx.skip_whitespace();
        if lx.peek() == Some(':') {
            lx.advance();
            let pseudo = lx.read_ident();
            sel_str.push_str(&format!(":{}", pseudo));
        }

        lx.skip_whitespace();
        if lx.peek() == Some('{') {
            lx.advance();
            let block = lx.read_brace_content();
            let mut inner_lx = Lexer::new(&block);
            let mut properties = Vec::new();

            while !inner_lx.is_empty() {
                inner_lx.skip_whitespace();
                if inner_lx.is_empty() { break; }
                let prop_name = inner_lx.read_ident();
                inner_lx.skip_whitespace();
                if inner_lx.peek() == Some(':') { inner_lx.advance(); }
                inner_lx.skip_whitespace();
                let mut val = String::new();
                while !inner_lx.is_empty() && inner_lx.peek() != Some(',') {
                    val.push(inner_lx.advance().unwrap());
                }
                properties.push(StyleProp {
                    name: prop_name,
                    value: val.trim().to_string(),
                });
                if inner_lx.peek() == Some(',') {
                    inner_lx.advance();
                }
            }

            rules.push(StyleRule { selector: sel_str, properties });
        }
    }
    Ok(StyleBlock { rules })
}

// ─── Component ref parsing (for html! macro) ────────────────

pub fn parse_component_ref_str(s: &str) -> Result<ComponentRef, String> {
    let mut lx = Lexer::new(s);
    lx.skip_whitespace();
    if lx.peek() == Some('<') { lx.advance(); }
    let tag = lx.read_ident();
    Ok(ComponentRef { tag })
}
