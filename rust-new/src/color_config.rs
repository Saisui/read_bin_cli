use ratatui::style::{Color, Style};
use std::collections::HashMap;
use std::fs;
use std::sync::OnceLock;

type Rgb = [u8; 3];

/// fg: auto 解析后的哨兵值，标记需要在渲染时实时计算
pub const AUTO_FG_SENTINEL: Color = Color::Rgb(1, 1, 1);

pub fn luminance(c: Color) -> f64 {
    let (r, g, b) = color_rgb(c);
    0.299 * r as f64 + 0.587 * g as f64 + 0.114 * b as f64
}

// ─── terminal palette ────────────────────────────────────────
static TERM_PALETTE: OnceLock<HashMap<usize, [u8; 3]>> = OnceLock::new();

pub fn init_terminal_palette(home_dir: &std::path::Path) {
    let mut map = HashMap::new();
    let path = home_dir.join(".termux/colors.properties");
    if let Ok(content) = fs::read_to_string(&path) {
        for line in content.lines() {
            let line = line.trim();
            if let Some((key, val)) = line.split_once('=') {
                let val = val.trim().trim_start_matches('#');
                if let Some(idx) = key.trim().strip_prefix("color")
                    .and_then(|s| s.trim().parse::<usize>().ok())
                {
                    if let Some(rgb) = parse_hex3(val) {
                        map.insert(idx, rgb);
                    }
                }
            }
        }
    }
    TERM_PALETTE.set(map).ok();
}

fn parse_hex3(s: &str) -> Option<[u8; 3]> {
    if s.len() >= 6 {
        Some([
            u8::from_str_radix(&s[0..2], 16).ok()?,
            u8::from_str_radix(&s[2..4], 16).ok()?,
            u8::from_str_radix(&s[4..6], 16).ok()?,
        ])
    } else { None }
}

/// Resolve a named Color to actual (r,g,b), using terminal palette if available
pub fn color_rgb(c: Color) -> (u8, u8, u8) {
    if let Color::Rgb(r, g, b) = c { return (r, g, b); }
    let idx = match c {
        Color::Black => 0,
        Color::Red => 1,
        Color::Green => 2,
        Color::Yellow => 3,
        Color::Blue => 4,
        Color::Magenta => 5,
        Color::Cyan => 6,
        Color::White => 7,
        Color::DarkGray => 8,
        _ => return (128, 128, 128),
    };
    if let Some(pal) = TERM_PALETTE.get() {
        if let Some(&[r, g, b]) = pal.get(&idx) { return (r, g, b); }
    }
    // fallback XTerm-like
    match idx {
        0 => (0, 0, 0),
        1 => (205, 0, 0),
        2 => (0, 205, 0),
        3 => (205, 205, 0),
        4 => (0, 0, 238),
        5 => (139, 0, 139),
        6 => (0, 205, 205),
        7 => (229, 229, 229),
        8 => (80, 80, 80),
        _ => (128, 128, 128),
    }
}

// ─── embedded defaults ───────────────────────────────────────
const DEFAULT_YAML: &str = r#"
null:      { fg: auto, bg: red }
control:   { fg: auto, bg: blue }
blank:     { fg: auto, bg: cyan }
ascii:     { fg: auto }
hex:       { fg: auto, bg: green }
head2:     { fg: auto, bg: blue }
head3:     { fg: auto, bg: magenta }
head4:     { fg: auto, bg: red }
tail:      { fg: auto, bg: yellow }
unknown:   { fg: auto, bg: red }
cursor:    { fg: auto, bg: yellow }
selection: { fg: auto, bg: green }
found:     { fg: auto, bg: yellow }
focused_button: { fg: auto, bg: white }

dim:
  global: 0.8
"#;

// ─── minimal YAML parser ─────────────────────────────────────
fn yaml_parse_style(block: &str, key: &str) -> Option<Style> {
    let mut start = 0;
    loop {
        let pos = block[start..].find(key)?;
        let key_start = start + pos;
        let rest = &block[key_start..];
        // Must be at line start (after optional whitespace)
        if key_start > 0 && !block[..key_start].ends_with('\n') {
            start = key_start + key.len();
            continue;
        }
        // skip key + ":"
        let after = rest.trim_start_matches(key).trim_start().strip_prefix(':')?;
        let val = after.trim_start();
        return parse_style_val(val);
    }
}

fn parse_style_val(val: &str) -> Option<Style> {
    let val = val.trim();
    if val == "{}" { return Some(Style::default()); }
    // find matching closing brace
    let brace_start = val.find('{')?;
    let mut depth = 0u8;
    let mut brace_end = brace_start;
    for (i, ch) in val[brace_start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => { depth -= 1; if depth == 0 { brace_end = brace_start + i; break; } }
            _ => {}
        }
    }
    let inner = val[brace_start + 1..brace_end].trim();
    let mut s = Style::default();
    let mut fg_auto = false;
    for part in split_top_level(inner, ',') {
        if let Some((k, v)) = part.split_once(':') {
            let k = k.trim();
            let v = v.trim();
            if k == "fg" && v == "auto" {
                fg_auto = true;
            } else if let Some(c) = parse_color(v) {
                match k {
                    "fg" => s = s.fg(c),
                    "bg" => s = s.bg(c),
                    _ => {}
                }
            }
        }
    }
    if fg_auto {
        if s.bg.is_some() {
            s = s.fg(AUTO_FG_SENTINEL);
        }
    }
    Some(s)
}

/// split on `sep` but skip when inside `[...]`
fn split_top_level<'a>(s: &'a str, sep: char) -> Vec<&'a str> {
    let mut parts = Vec::new();
    let mut depth = 0u8;
    let mut last = 0;
    for (i, ch) in s.char_indices() {
        match ch {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            c if c == sep && depth == 0 => {
                parts.push(s[last..i].trim());
                last = i + 1;
            }
            _ => {}
        }
    }
    parts.push(s[last..].trim());
    parts
}

fn parse_color(v: &str) -> Option<Color> {
    match v {
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "white" => Some(Color::White),
        "darkgray" | "dark_gray" => Some(Color::DarkGray),
        "auto" => None,
        _ => parse_rgb(v),
    }
}

fn parse_rgb(v: &str) -> Option<Color> {
    let inner = v.strip_prefix('[')?.strip_suffix(']')?;
    let mut nums = [0u8; 3];
    for (i, p) in inner.split(',').enumerate() {
        if i >= 3 { return None; }
        nums[i] = p.trim().parse().ok()?;
    }
    Some(Color::Rgb(nums[0], nums[1], nums[2]))
}

fn yaml_parse_dim(yaml: &str) -> (f64, HashMap<Rgb, f64>) {
    let dim_start = yaml.find("\ndim:").unwrap_or(yaml.len());
    let dim_block = &yaml[dim_start..];
    let global = dim_block
        .lines()
        .find(|l| l.trim().starts_with("global:"))
        .and_then(|l| l.split(':').nth(1))
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0.8);

    let mut overrides = HashMap::new();
    let over_block = dim_block.find("overrides:").map(|i| &dim_block[i..]).unwrap_or("");
    for line in over_block.lines().skip(1) {
        let line = line.trim();
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim().trim_matches('"').trim();
            if let Some(v) = v.trim().parse().ok() {
                if let Some(c) = parse_rgb(k) {
                    if let Color::Rgb(r, g, b) = c {
                        overrides.insert([r, g, b], v);
                    }
                }
            }
        }
    }

    (global, overrides)
}

// ─── ColorConfig ─────────────────────────────────────────────
#[derive(Debug)]
pub struct ColorConfig {
    pub sp_null: Style,
    pub sp_control: Style,
    pub sp_blank: Style,
    pub sp_ascii: Style,
    pub sp_hex: Style,
    pub sp_head2: Style,
    pub sp_head3: Style,
    pub sp_head4: Style,
    pub sp_tail: Style,
    pub sp_unknown: Style,
    pub sp_cursor: Style,
    pub sp_found: Style,
    pub sp_selection: Style,
    pub sp_focused_button: Style,
    dim_global: f64,
    dim_overrides: HashMap<Rgb, f64>,
}

impl ColorConfig {
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let yaml = if path.exists() {
            fs::read_to_string(path).map_err(|e| format!("read color.yaml: {e}"))?
        } else {
            DEFAULT_YAML.to_string()
        };
        Self::parse(&yaml)
    }

    fn parse(yaml: &str) -> Result<Self, String> {
        let d = Style::default().fg(Color::Rgb(220, 220, 220));

        let (dim_global, dim_overrides) = yaml_parse_dim(yaml);

        Ok(ColorConfig {
            sp_null:    yaml_parse_style(yaml, "null").unwrap_or(d),
            sp_control: yaml_parse_style(yaml, "control").unwrap_or(d),
            sp_blank:   yaml_parse_style(yaml, "blank").unwrap_or(d),
            sp_ascii:   yaml_parse_style(yaml, "ascii").unwrap_or(d),
            sp_hex:     yaml_parse_style(yaml, "hex").unwrap_or(d),
            sp_head2:   yaml_parse_style(yaml, "head2").unwrap_or_default(),
            sp_head3:   yaml_parse_style(yaml, "head3").unwrap_or_default(),
            sp_head4:   yaml_parse_style(yaml, "head4").unwrap_or_default(),
            sp_tail:    yaml_parse_style(yaml, "tail").unwrap_or_default(),
            sp_unknown: yaml_parse_style(yaml, "unknown").unwrap_or_default(),
            sp_cursor: yaml_parse_style(yaml, "cursor").unwrap_or_default(),
            sp_found: yaml_parse_style(yaml, "found").unwrap_or_default(),
            sp_selection: yaml_parse_style(yaml, "selection").unwrap_or_default(),
            sp_focused_button: yaml_parse_style(yaml, "focused_button").unwrap_or_default(),
            dim_global,
            dim_overrides,
        })
    }

    pub fn dim_bg(&self, s: Style) -> Style {
        if let Some(bg) = s.bg {
            let mult = match bg {
                Color::Rgb(r, g, b) => self.dim_overrides.get(&[r, g, b]).copied().unwrap_or(self.dim_global),
                _ => self.dim_global,
            };
            let (num, den) = ((mult * 100.0) as u16, 100);
            s.bg(scale_color(bg, num, den))
        } else {
            s
        }
    }
}

fn scale_color(c: Color, num: u16, den: u16) -> Color {
    let (r, g, b) = color_rgb(c);
    Color::Rgb(
        (r as u16 * num / den) as u8,
        (g as u16 * num / den) as u8,
        (b as u16 * num / den) as u8,
    )
}
