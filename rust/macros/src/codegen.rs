use quote::quote;
use proc_macro2::TokenStream;
use crate::parser::*;

pub fn gen_tui_file(tui: &TuiFile) -> TokenStream {
    let mut tokens = TokenStream::new();

    if let Some(style) = &tui.style {
        tokens.extend(gen_style_block(style));
    }

    tokens.extend(gen_template_block(&tui.template));

    if let Some(script) = &tui.script {
        let body_str = &script.body;
        let body_tokens: proc_macro2::TokenStream = body_str.parse()
            .unwrap_or_else(|e| {
                eprintln!("Warning: failed to parse script body: {}", e);
                quote! {}
            });
        tokens.extend(body_tokens);
    }

    tokens
}

fn gen_style_block(style: &StyleBlock) -> TokenStream {
    let mut match_arms = Vec::new();
    for rule in &style.rules {
        let selector = &rule.selector;
        let props = &rule.properties;
        let (class, pseudo) = if let Some(colon_pos) = selector.find(':') {
            (&selector[1..colon_pos], Some(&selector[colon_pos + 1..]))
        } else {
            (&selector[1..], None)
        };
        let style_expr = gen_style_expr(props);
        match pseudo {
            Some(p) => match_arms.push(quote! { (#class, Some(#p)) => #style_expr, }),
            None => match_arms.push(quote! { (#class, _) => #style_expr, }),
        }
    }
    quote! {
        fn style_resolve(class: &str, pseudo: Option<&str>) -> ratatui::style::Style {
            match (class, pseudo) {
                #(#match_arms)*
                _ => ratatui::style::Style::default(),
            }
        }
    }
}

fn gen_style_expr(props: &[StyleProp]) -> TokenStream {
    let mut fg = None;
    let mut bg = None;
    let mut bold = false;
    let mut dim = false;
    for prop in props {
        match prop.name.as_str() {
            "fg" => fg = Some(parse_color_value(&prop.value)),
            "bg" => bg = Some(parse_color_value(&prop.value)),
            "bold" => bold = prop.value == "true",
            "dim" => dim = prop.value == "true",
            _ => {}
        }
    }
    let mut s = quote! { ratatui::style::Style::default() };
    if let Some(fg_expr) = fg { s = quote! { #s.fg(#fg_expr) }; }
    if let Some(bg_expr) = bg { s = quote! { #s.bg(#bg_expr) }; }
    if bold { s = quote! { #s.add_modifier(ratatui::style::Modifier::BOLD) }; }
    if dim { s = quote! { #s.add_modifier(ratatui::style::Modifier::DIM) }; }
    s
}

fn parse_color_value(val: &str) -> TokenStream {
    match val {
        "auto" => quote! { crate::color_config::AUTO_FG_SENTINEL },
        "black" => quote! { ratatui::style::Color::Black },
        "red" => quote! { ratatui::style::Color::Red },
        "green" => quote! { ratatui::style::Color::Green },
        "yellow" => quote! { ratatui::style::Color::Yellow },
        "blue" => quote! { ratatui::style::Color::Blue },
        "magenta" => quote! { ratatui::style::Color::Magenta },
        "cyan" => quote! { ratatui::style::Color::Cyan },
        "white" => quote! { ratatui::style::Color::White },
        "darkgray" | "dark_gray" => quote! { ratatui::style::Color::DarkGray },
        _ => {
            if val.starts_with('[') && val.ends_with(']') {
                let inner = &val[1..val.len() - 1];
                let parts: Vec<u8> = inner.split(',').filter_map(|s| s.trim().parse().ok()).collect();
                if parts.len() == 3 {
                    let (r, g, b) = (parts[0], parts[1], parts[2]);
                    return quote! { ratatui::style::Color::Rgb(#r, #g, #b) };
                }
            }
            quote! { ratatui::style::Color::White }
        }
    }
}

fn gen_template_block(template: &TemplateBlock) -> TokenStream {
    let body = gen_nodes(&template.nodes);
    quote! {
        pub fn render(f: &mut ratatui::Frame, app: &crate::app::App, data: &[u8], area: ratatui::layout::Rect) {
            #body
        }
    }
}

fn gen_nodes(nodes: &[TemplateNode]) -> TokenStream {
    let stmts: Vec<_> = nodes.iter().map(gen_node).collect();
    quote! { #(#stmts)* }
}

fn gen_node(node: &TemplateNode) -> TokenStream {
    match node {
        TemplateNode::Element(elem) => gen_element(elem),
        TemplateNode::Text(text) => {
            // 静态文本节点：在 TUI 中通常由父组件（如 Text）处理
            // 这里生成空语句，避免编译警告
            let t = text.as_str();
            if t.is_empty() {
                quote! {}
            } else {
                quote! { let _text_node: &str = #t; }
            }
        }
        TemplateNode::Interpolation(expr) => {
            // 插值表达式：直接作为 Rust 代码透传
            // 例如 {{ app.mode.label() }} → app.mode.label()
            let expr_ts: proc_macro2::TokenStream = expr.parse()
                .expect("Invalid interpolation expression");
            quote! { { let _ = &#expr_ts; } }
        }
        TemplateNode::ForLoop { var, iter, body } => {
            let v: proc_macro2::TokenStream = var.parse().unwrap();
            let it: proc_macro2::TokenStream = iter.parse().unwrap();
            let b = gen_nodes(body);
            quote! { for #v in #it { #b } }
        }
        TemplateNode::IfCond { expr, body } => {
            let e: proc_macro2::TokenStream = expr.parse().unwrap();
            let b = gen_nodes(body);
            quote! { if #e { #b } }
        }
    }
}

fn gen_element(elem: &Element) -> TokenStream {
    match elem.tag.as_str() {
        "Text" => gen_text_element(elem),
        "Statusbar" => gen_statusbar(elem),
        "Popup" => gen_popup(elem),
        "Row" => gen_row(elem),
        "Byte" => gen_byte(elem),
        "Scrollable" | "Scrollbar" => quote! {},
        _ => {
            let children = gen_nodes(&elem.children);
            quote! { #children }
        }
    }
}

fn gen_text_element(elem: &Element) -> TokenStream {
    let class = elem.classes.first().map(|c| c.as_str()).unwrap_or("");
    let content = gen_interpolation_content(&elem.children);
    quote! {
        {
            let sty = style_resolve(#class, None);
            let text = #content;
            spans.push(ratatui::text::Span::styled(text, sty));
        }
    }
}

fn gen_statusbar(elem: &Element) -> TokenStream {
    let children = gen_nodes(&elem.children);
    quote! {
        {
            f.render_widget(ratatui::widgets::Clear, ratatui::layout::Rect::new(0, area.height - 1, area.width, 1));
            let mut spans: Vec<ratatui::text::Span> = Vec::new();
            #children
            f.render_widget(
                ratatui::widgets::Paragraph::new(ratatui::text::Line::from(spans)),
                ratatui::layout::Rect::new(0, area.height - 1, area.width, 1),
            );
        }
    }
}

fn gen_popup(elem: &Element) -> TokenStream {
    let title = get_prop_value(elem, "title").unwrap_or_default();
    let title_tokens: proc_macro2::TokenStream = title.parse().unwrap_or_else(|_| quote! { "" });
    let children = gen_nodes(&elem.children);
    quote! {
        {
            let h = (area.height * 8 / 10).max(10).min(40);
            let w = (area.width * 9 / 10).max(30).min(80);
            let y = (area.height.saturating_sub(h)) / 2;
            let x = (area.width.saturating_sub(w)) / 2;
            let popup = ratatui::layout::Rect::new(x, y, w, h);
            f.render_widget(ratatui::widgets::Clear, popup);
            f.render_widget(
                ratatui::widgets::Block::default()
                    .borders(ratatui::widgets::Borders::ALL)
                    .border_style(ratatui::style::Style::default().fg(ratatui::style::Color::Gray).bg(ratatui::style::Color::Rgb(20, 20, 40)))
                    .title(#title_tokens)
                    .style(ratatui::style::Style::default().bg(ratatui::style::Color::Rgb(20, 20, 40))),
                popup,
            );
            #children
        }
    }
}

fn gen_row(elem: &Element) -> TokenStream {
    let children = gen_nodes(&elem.children);
    quote! {
        {
            #children
            lines.push(ratatui::text::Line::from(spans));
            spans = Vec::new();
        }
    }
}

fn gen_byte(elem: &Element) -> TokenStream {
    let offset = get_prop_value(elem, "offset")
        .map(|v| v.parse::<proc_macro2::TokenStream>().unwrap_or_else(|_| quote! { 0 }))
        .unwrap_or_else(|| quote! { 0 });
    quote! {
        {
            let byte_offset = #offset;
            if byte_offset < data.len() {
                let b = data[byte_offset];
                let sty = crate::byte_style(b, app.mode);
                let disp = crate::byte_disp(b, app.mode);
                spans.push(ratatui::text::Span::styled(disp, sty));
            }
        }
    }
}

fn gen_interpolation_content(nodes: &[TemplateNode]) -> TokenStream {
    let mut parts = Vec::new();
    for node in nodes {
        match node {
            TemplateNode::Text(t) => {
                let s = t.as_str();
                parts.push(quote! { #s });
            }
            TemplateNode::Interpolation(expr) => {
                let e: proc_macro2::TokenStream = expr.parse().unwrap();
                parts.push(quote! { &#e });
            }
            _ => {}
        }
    }
    if parts.is_empty() {
        quote! { String::new() }
    } else if parts.len() == 1 {
        parts[0].clone()
    } else {
        quote! { format!(#(#parts),*) }
    }
}

fn get_prop_value(elem: &Element, name: &str) -> Option<String> {
    elem.props.iter().find(|p| p.name == name).map(|p| p.value.clone())
}

pub fn gen_component_call(component: &ComponentRef) -> TokenStream {
    let tag = &component.tag;
    let tag_ident: proc_macro2::TokenStream = tag.parse().unwrap();
    quote! {
        {
            crate::components::#tag_ident::render(f, app, data, area);
        }
    }
}
