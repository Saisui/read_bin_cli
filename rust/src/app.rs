/// 应用状态管理模块
///
/// 管理文件分页、光标位置、显示模式、搜索状态、撤销/做栈。
/// 不直接处理渲染或输入事件，由 main.rs 驱动。
use std::path::PathBuf;
use std::sync::mpsc;
use crate::search::{Search, SearchEvent};

/// 显示模式：ASCII 字符 / HEX 十六进制 / UTF-8 解码
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayMode { Ascii, Hex, Utf8 }

impl DisplayMode {
    /// 切换到下一个显示模式（Ascii → Hex → Utf8 → Ascii）
    pub fn next(self) -> Self {
        match self {
            Self::Ascii => Self::Hex,
            Self::Hex => Self::Utf8,
            Self::Utf8 => Self::Ascii,
        }
    }
    /// 切换到上一个显示模式（Utf8 → Hex → Ascii → Utf8）
    pub fn prev(self) -> Self {
        match self {
            Self::Ascii => Self::Utf8,
            Self::Hex => Self::Ascii,
            Self::Utf8 => Self::Hex,
        }
    }
    /// 返回模式的显示标签（如 "[ASCII]"、"[HEX]"、"[UTF8]"）
    pub fn label(self) -> &'static str {
        match self { Self::Ascii => "[ASCII]", Self::Hex => "[HEX]", Self::Utf8 => "[UTF8]" }
    }
}

/// 输入状态机
///
/// Normal: 浏览模式（快捷键导航）
/// Edit: 字节编辑模式
/// SearchInput / StringSearchInput / GotoInput: 文本输入模式
/// SaveConfirm: 退出确认弹窗
/// Help: 帮助弹窗
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Edit,
    SearchInput,
    GotoInput,
    GotoByteInput,
    StringSearchInput,
    SaveConfirm,
    Help,
    ModeSelect,
    FileBrowser,
}

/// 撤销/重做条目：记录单字节修改
pub struct UndoEntry {
    /// 修改的字节偏移
    pub offset: usize,
    /// 修改前的值
    pub old: u8,
    /// 修改后的值
    pub new: u8,
}

/// 核心应用状态
///
/// - file_size / pack_size / total_packs: 文件分页信息（每 pack 4096 字节）
/// - current_pack / scroll_top: 当前可见 pack 和滚动偏移
/// - cursor_byte / cursor_nibble: 光标位置（字节偏移 + hex 模式的半字节）
/// - search / pack_ranges: 搜索状态和当前 pack 内的匹配范围
/// - undo_stack / redo_stack: 编辑历史
/// - sel_start / sel_end: 选区范围
pub struct App {
    pub file_size: usize,
    pub pack_size: usize,
    pub total_packs: usize,
    pub current_pack: usize,
    pub scroll_top: usize,
    pub mode: DisplayMode,
    pub input_mode: InputMode,
    pub filename: String,
    pub cursor_byte: usize,
    pub cursor_nibble: usize,
    pub dirty: bool,
    pub undo_stack: Vec<UndoEntry>,
    pub redo_stack: Vec<UndoEntry>,
    pub search: Option<Search>,
    pub search_active: bool,
    pub pack_ranges: Vec<(usize, usize)>,
    pub pack_match_idx: Option<usize>,
    pub global_match_idx: Option<usize>,
    pub input_buf: String,
    pub input_prompt: String,
    pub save_selected: bool,
    pub sel_start: Option<usize>,
    pub sel_end: Option<usize>,
    pub dragging: bool,
    pub help_scroll: usize,
    pub help_dragging: bool,
    pub help_rect: Option<(u16, u16, u16, u16)>, // x, y, w, h
    pub cursor_focused: bool,
    pub search_rx: Option<mpsc::Receiver<SearchEvent>>,
    pub is_color256: bool,
    pub is_rgb_bg: bool,
    pub is_hsl_bg: bool,
}

impl App {
    /// 创建新应用实例，pack 大小固定为 4096 字节
    pub fn new(file_size: usize, filename: String) -> Self {
        let pack_size = 4096;
        Self {
            file_size,
            pack_size,
            total_packs: (file_size + pack_size - 1) / pack_size,
            current_pack: 0,
            scroll_top: 0,
            mode: DisplayMode::Ascii,
            input_mode: InputMode::Normal,
            filename,
            cursor_byte: 0,
            cursor_nibble: 0,
            dirty: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            search: None,
            search_active: false,
            pack_ranges: Vec::new(),
            pack_match_idx: None,
            global_match_idx: None,
            input_buf: String::new(),
            input_prompt: String::new(),
            save_selected: false,
            sel_start: None,
            sel_end: None,
            dragging: false,
            help_scroll: 0,
            help_dragging: false,
            help_rect: None,
            cursor_focused: true,
            search_rx: None,
            is_color256: false,
            is_rgb_bg: false,
            is_hsl_bg: false,
        }
    }

    /// 终端高度可显示的最大行数（减去 1 行顶栏 + 1 行列号头 + 1 行状态栏）
    pub fn max_rows(&self, h: u16) -> usize {
        (h as usize).saturating_sub(3)
    }

    /// 当前 pack 的实际数据长度（最后一 pack 可能不满 4096）
    pub fn data_len(&self) -> usize {
        self.pack_size.min(self.file_size - self.current_pack * self.pack_size)
    }

    /// 当前 pack 的总行数（每行 16 字节）
    pub fn total_rows(&self) -> usize {
        (self.data_len() + 15) / 16
    }

    /// 文件总行数（跨页全局行数）
    pub fn global_total_rows(&self) -> usize {
        (self.file_size + 15) / 16
    }

    /// 当前视口的全局起始行号 = current_pack * 每页行数 + scroll_top
    pub fn global_scroll_top(&self) -> usize {
        self.current_pack * (self.pack_size / 16) + self.scroll_top
    }

    /// 全局行号 → (页号, 页内行号)
    ///
    /// 用于 build_lines 跨页渲染：给定全局行号，计算它落在哪个 pack 以及 pack 内的行偏移。
    pub fn global_to_local(&self, grow: usize) -> (usize, usize) {
        let pack_idx = grow / (self.pack_size / 16);
        let row_in_pack = grow % (self.pack_size / 16);
        (pack_idx.min(self.total_packs.saturating_sub(1)), row_in_pack)
    }

    /// 设置全局滚动位置（自动计算 current_pack 和 scroll_top）
    pub fn set_global_scroll(&mut self, global_row: usize) {
        let rows_per_pack = self.pack_size / 16;
        let max_global = self.global_total_rows().saturating_sub(1);
        let g = global_row.min(max_global);
        self.current_pack = g / rows_per_pack;
        self.scroll_top = g % rows_per_pack;
    }

    /// 将字节数格式化为人类可读大小（如 "1.5KB"、"2.0MB"）
    pub fn format_size(size: usize) -> String {
        let mut s = size as f64;
        for u in &["B", "KB", "MB", "GB"] {
            if s < 1024.0 {
                return if *u == "B" { format!("{}B", s as usize) } else { format!("{:.1}{}", s, u) };
            }
            s /= 1024.0;
        }
        format!("{:.1}TB", s)
    }

    /// 获取当前搜索匹配的字节范围
    pub fn current_match_range(&self) -> Option<(usize, usize)> {
        if !self.search_active { return None; }
        let idx = self.global_match_idx?;
        self.search.as_ref()?.ranges.get(idx).copied()
    }

    /// 清除搜索状态
    pub fn clear_search(&mut self) {
        self.search_active = false;
        self.search = None;
        self.pack_ranges.clear();
        self.pack_match_idx = None;
        self.global_match_idx = None;
    }

    /// 刷新当前 pack 的匹配范围列表和高亮索引
    pub fn refresh_pack(&mut self) {
        if let Some(ref s) = self.search {
            self.pack_ranges = s.pack_matches(self.current_pack);
            self.pack_match_idx = self.global_match_idx
                .and_then(|gi| s.ranges.get(gi).map(|&(st, _)| st))
                .and_then(|st| self.pack_ranges.iter().position(|(s, e)| *s <= st && st < *e));
        }
    }

    /// 跳转到全局搜索的第 idx 个匹配
    pub fn jump_global(&mut self, idx: usize) -> bool {
        let s = match self.search.as_ref() { Some(s) => s, None => return false };
        if idx >= s.ranges.len() { return false; }
        let (start, _) = s.ranges[idx];
        self.global_match_idx = Some(idx);
        self.current_pack = start / self.pack_size;
        let row = (start % self.pack_size) / 16;
        self.scroll_top = row.saturating_sub(12);
        self.refresh_pack();
        true
    }

    /// 跳转到下一个全局匹配，必要时触发增量搜索
    pub fn next_global(&mut self, mmap: &[u8]) -> bool {
        let ni = self.global_match_idx.map_or(0, |i| i + 1);
        let len = self.search.as_ref().map_or(0, |s| s.ranges.len());
        if ni < len { return self.jump_global(ni); }
        let last = self.search.as_ref().and_then(|s| s.ranges.last().map(|(_, e)| *e));
        if let Some(e) = last {
            let s = self.search.as_mut().unwrap();
            if s.extend(mmap, e + 1) && ni < s.ranges.len() {
                return self.jump_global(ni);
            }
        }
        false
    }

    /// 跳转到上一个全局匹配
    pub fn prev_global(&mut self) -> bool {
        let cur = match self.global_match_idx { Some(i) => i, None => return false };
        if cur == 0 { return false; }
        self.jump_global(cur - 1)
    }

    /// 跳转到下一个 pack 的第一个匹配
    pub fn next_page_match(&mut self) -> bool {
        let cur_pack = self.current_pack;
        if let Some(s) = self.search.as_ref() {
            for idx in 0..s.ranges.len() {
                let (start, _) = s.ranges[idx];
                if start / self.pack_size > cur_pack {
                    return self.jump_global(idx);
                }
            }
        }
        false
    }

    /// 跳转到上一个 pack 的最后一个匹配
    pub fn prev_page_match(&mut self) -> bool {
        let cur_pack = self.current_pack;
        if let Some(s) = self.search.as_ref() {
            for idx in (0..s.ranges.len()).rev() {
                let (start, _) = s.ranges[idx];
                if start / self.pack_size < cur_pack {
                    return self.jump_global(idx);
                }
            }
        }
        false
    }

    /// 确保光标在可见区域内（跨页）
    ///
    /// 根据光标全局行号与当前视口全局起始行号比较，
    /// 必要时通过 set_global_scroll 调整视口位置。
    pub fn ensure_cursor_visible(&mut self, h: u16) {
        let rows_per_pack = self.pack_size / 16;
        let pk = self.cursor_byte / self.pack_size;
        let row_in_pack = (self.cursor_byte % self.pack_size) / 16;
        let global_row = pk * rows_per_pack + row_in_pack;
        let gscroll = self.global_scroll_top();
        let mr = self.max_rows(h);
        if global_row < gscroll {
            self.set_global_scroll(global_row);
        } else if global_row >= gscroll + mr {
            self.set_global_scroll(global_row.saturating_sub(mr - 1));
        }
    }

    /// 修改单字节并记录到撤销栈
    pub fn modify(&mut self, mmap: &mut [u8], off: usize, val: u8) {
        if off < self.file_size && mmap[off] != val {
            self.undo_stack.push(UndoEntry { offset: off, old: mmap[off], new: val });
            self.redo_stack.clear();
            mmap[off] = val;
            self.dirty = true;
        }
    }

    /// 撤销上一次修改
    pub fn undo(&mut self, mmap: &mut [u8]) {
        if let Some(e) = self.undo_stack.pop() {
            if e.offset < self.file_size {
                mmap[e.offset] = e.old;
                self.redo_stack.push(e);
                self.dirty = !self.undo_stack.is_empty();
            }
        }
    }

    /// 重做上一次撤销
    pub fn redo(&mut self, mmap: &mut [u8]) {
        if let Some(e) = self.redo_stack.pop() {
            if e.offset < self.file_size {
                mmap[e.offset] = e.new;
                self.undo_stack.push(e);
                self.dirty = true;
            }
        }
    }

    /// HEX 模式下编辑半字节（nibble）
    ///
    /// 输入一个 hex 字符，替换当前 nibble，然后自动前进到下一个 nibble/字节。
    pub fn edit_hex(&mut self, mmap: &mut [u8], ch: char) {
        let nib = match ch {
            '0'..='9' => ch as u8 - b'0',
            'a'..='f' => ch as u8 - b'a' + 10,
            'A'..='F' => ch as u8 - b'A' + 10,
            _ => return,
        };
        if self.cursor_byte >= self.file_size { return; }
        let cur = mmap[self.cursor_byte];
        let new = if self.cursor_nibble == 0 { (cur & 0x0f) | (nib << 4) } else { (cur & 0xf0) | nib };
        self.modify(mmap, self.cursor_byte, new);
        if self.cursor_nibble == 0 { self.cursor_nibble = 1; }
        else {
            self.cursor_nibble = 0;
            self.cursor_byte += 1;
            if self.cursor_byte >= self.file_size {
                self.cursor_byte = self.file_size - 1;
                self.cursor_nibble = 1;
            }
        }
    }

    /// ASCII/UTF8 模式下编辑字符
    ///
    /// 将字符编码为 UTF-8 字节序列，逐字节写入并推进光标。
    pub fn edit_char(&mut self, mmap: &mut [u8], ch: char) {
        let mut buf = [0u8; 4];
        let s = ch.encode_utf8(&mut buf);
        for &b in s.as_bytes() {
            if self.cursor_byte >= self.file_size { return; }
            self.modify(mmap, self.cursor_byte, b);
            self.cursor_byte += 1;
        }
        if self.cursor_byte >= self.file_size { self.cursor_byte = self.file_size - 1; }
    }

    /// Accept a pre-built Search, run it, position to first match
    pub fn apply_search(&mut self, mut acc: Search, mmap: &[u8], h: u16) -> bool {
        let off = self.current_pack * self.pack_size + self.scroll_top * 16;
        acc.extend(mmap, off);
        self.search_active = true;
        self.search = Some(acc);
        if !self.search.as_ref().unwrap().ranges.is_empty() {
            self.global_match_idx = Some(0);
            let (start, _) = self.search.as_ref().unwrap().ranges[0];
            self.current_pack = start / self.pack_size;
            let row = (start % self.pack_size) / 16;
            let mr = self.max_rows(h);
            let tr = self.total_rows();
            self.scroll_top = row.saturating_sub(mr / 2).min(tr.saturating_sub(mr));
        }
        self.refresh_pack();
        true
    }

    /// 启动后台增量搜索线程
    pub fn start_bg_search(&mut self, needle: Vec<u8>, data: Vec<u8>) {
        let file_size = self.file_size;
        let rx = crate::search::start_bg_search(needle, file_size, data);
        self.search_rx = Some(rx);
    }

    /// 消费后台搜索结果，将新匹配追加到 ranges 并更新位图
    pub fn drain_search_rx(&mut self) {
        let events: Vec<SearchEvent> = {
            let rx = match self.search_rx { Some(ref rx) => rx, None => return };
            let mut events = Vec::new();
            loop {
                match rx.try_recv() {
                    Ok(event) => events.push(event),
                    Err(_) => break,
                }
            }
            events
        };
        for event in events {
            match event {
                SearchEvent::Chunk { matches } => {
                    if let Some(ref mut s) = self.search {
                        for &(start, end) in &matches {
                            if s.ranges.last().map_or(true, |(_, e)| *e <= start) {
                                s.ranges.push((start, end));
                                s.match_count += 1;
                                s.mark_all(start, end);
                            }
                        }
                    }
                }
                SearchEvent::Done => {
                    self.search_rx = None;
                }
            }
        }
        self.refresh_pack();
    }
}

/// 文件浏览器条目
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

/// 文件浏览器状态
///
/// 用于无参数启动或 Ctrl+P 打开新文件时的目录浏览。
/// 显示当前目录内容，支持上下导航、进入目录、返回上级。
pub struct FileBrowser {
    pub current_dir: PathBuf,
    pub entries: Vec<DirEntry>,
    pub cursor: usize,
    pub scroll_top: usize,
    pub last_click_idx: Option<usize>,
}

impl FileBrowser {
    /// 创建文件浏览器，从指定目录开始
    pub fn new(dir: PathBuf) -> Self {
        let mut fb = Self {
            current_dir: dir,
            entries: Vec::new(),
            cursor: 0,
            scroll_top: 0,
            last_click_idx: None,
        };
        fb.refresh_entries();
        fb
    }

    /// 刷新目录条目列表
    ///
    /// 读取当前目录，排序：../ 在最前，然后目录按名称排序，最后文件按名称排序。
    pub fn refresh_entries(&mut self) {
        self.entries.clear();
        self.cursor = 0;
        self.scroll_top = 0;

        // 添加 ../ （除非在根目录）
        if self.current_dir.parent().is_some() {
            self.entries.push(DirEntry {
                name: "..".to_string(),
                is_dir: true,
                size: 0,
            });
        }

        if let Ok(read_dir) = std::fs::read_dir(&self.current_dir) {
            let mut dirs = Vec::new();
            let mut files = Vec::new();
            for entry in read_dir.flatten() {
                let metadata = entry.metadata().ok();
                let name = entry.file_name().to_string_lossy().to_string();
                let is_dir = metadata.as_ref().map_or(false, |m| m.is_dir());
                let size = metadata.as_ref().map_or(0, |m| m.len());
                if is_dir {
                    dirs.push(DirEntry { name, is_dir: true, size: 0 });
                } else {
                    files.push(DirEntry { name, is_dir: false, size });
                }
            }
            dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            self.entries.extend(dirs);
            self.entries.extend(files);
        }
    }

    /// 进入指定光标位置的目录或返回文件路径
    ///
    /// 返回 Some(path) 表示选中了文件，None 表示进入了目录。
    pub fn enter(&mut self) -> Option<PathBuf> {
        if self.entries.is_empty() {
            return None;
        }
        let entry = &self.entries[self.cursor];
        if entry.is_dir {
            let new_dir = if entry.name == ".." {
                self.current_dir.parent().unwrap_or(&self.current_dir).to_path_buf()
            } else {
                self.current_dir.join(&entry.name)
            };
            self.current_dir = new_dir;
            self.refresh_entries();
            None
        } else {
            Some(self.current_dir.join(&entry.name))
        }
    }

    /// 光标上移
    pub fn move_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    /// 光标下移
    pub fn move_down(&mut self) {
        if self.cursor + 1 < self.entries.len() {
            self.cursor += 1;
        }
    }

    /// 光标翻页
    pub fn page_up(&mut self, page_size: usize) {
        self.cursor = self.cursor.saturating_sub(page_size);
    }

    /// 光标翻页
    pub fn page_down(&mut self, page_size: usize) {
        self.cursor = (self.cursor + page_size).min(self.entries.len().saturating_sub(1));
    }

    /// 确保光标在可见区域内
    pub fn ensure_visible(&mut self, max_rows: usize) {
        if self.cursor < self.scroll_top {
            self.scroll_top = self.cursor;
        } else if self.cursor >= self.scroll_top + max_rows {
            self.scroll_top = self.cursor - max_rows + 1;
        }
    }
}
