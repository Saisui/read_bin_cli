# read-bin-macros

编译时 proc macro crate，为 read-bin 提供 Vue-like `.tui` 单文件组件系统。

## 功能

- `include_tui!("ui/xxx.tui")` — 编译时读取 .tui 文件，生成 ratatui 渲染代码
- `html!{ <Component /> }` — 展开为组件渲染调用

## .tui 文件格式

```html
<template>
  <Statusbar>
    <Text class="mode">{{ app.mode.label() }}</Text>
  </Statusbar>
</template>

<style>
  .mode { fg: auto, bg: green }
</style>

<script lang="rust">
  fn helper() -> String { "hello".into() }
</script>
```

## 依赖

- `proc-macro2` v1.0 — TokenStream 操作
- `quote` v1.0 — 代码生成
- `syn` v2.0 — 仅用于解析字符串字面量

## 架构

```
lib.rs          入口：include_tui! 和 html! 宏
parser.rs       自定义 lexer + .tui 解析器（不依赖 syn::Parse）
codegen.rs      AST → Rust TokenStream 代码生成
```

### 为什么用自定义 lexer？

1. `<template>` 被 Rust tokenizer 解析为 `<` + `template` + `>`（比较运算符）
2. `{{ }}` 中的 `}}` 触发 syn 的 delimiter 匹配错误
3. 自定义 lexer 直接操作 `Vec<char>`，无 delimiter 问题

## 编译

```bash
# 在电脑上编译（Termux 内存不足）
cargo build --release

# 首次编译 ~30秒（syn/quote），后续增量 ~5秒
# 二进制大小不变（proc macro 不进入二进制）
```

## 生成的代码

`include_tui!("ui/statusbar.tui")` 生成：

```rust
// 从 <style> 块生成
fn style_resolve(class: &str, pseudo: Option<&str>) -> ratatui::style::Style {
    match (class, pseudo) {
        ("mode", _) => ratatui::style::Style::default()
            .fg(crate::color_config::AUTO_FG_SENTINEL)
            .bg(ratatui::style::Color::Green),
        _ => ratatui::style::Style::default(),
    }
}

// 从 <template> 块生成
pub fn render(f: &mut ratatui::Frame, app: &crate::app::App, data: &[u8], area: ratatui::layout::Rect) {
    f.render_widget(ratatui::widgets::Clear, ratatui::layout::Rect::new(0, area.height - 1, area.width, 1));
    let mut spans: Vec<ratatui::text::Span> = Vec::new();
    {
        let sty = style_resolve("mode", None);
        let text = &app.mode.label();
        spans.push(ratatui::text::Span::styled(text, sty));
    }
    f.render_widget(
        ratatui::widgets::Paragraph::new(ratatui::text::Line::from(spans)),
        ratatui::layout::Rect::new(0, area.height - 1, area.width, 1),
    );
}

// <script lang="rust"> 中的函数直接透传
```
