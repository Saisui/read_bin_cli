/// 搜索模块
///
/// 支持精确 hex 字节搜索和 nibble 模式匹配（如 `4x`、`[0-3]f`、`z`）。
/// 使用三级位图索引（L0=pack, L1=1MB, L2=1GB）加速跳过无匹配区域。
/// 后台线程增量搜索，通过 channel 回传结果。
use std::sync::mpsc;
use std::thread;

/// 搜索跳转步长（1MB），用于 O/P 键 ±1MB 区域跳转
pub const FIND_CHUNK: usize = 1024 * 1024;

/// 后台搜索每次扫描的数据块大小（1MB）
const CHUNK: usize = 1024 * 1024;

/// 位图索引 L1 层粒度（1MB = 1 个 L1 位）
const L1_SIZE: usize = 1024 * 1024;

/// 位图索引 L2 层粒度（1GB = 1 个 L2 位）
const L2_SIZE: usize = 1024 * 1024 * 1024;

/// 后台搜索事件：一批匹配结果或搜索完成
pub enum SearchEvent {
    Chunk { matches: Vec<(usize, usize)> },
    Done,
}

/// 启动后台搜索线程
///
/// 按 CHUNK 分块扫描整个文件，每块找到的匹配通过 channel 发送。
/// 返回 receiver 供主线程通过 `drain_search_rx()` 消费。
pub fn start_bg_search(
    needle_bytes: Vec<u8>,
    file_size: usize,
    mmap: Vec<u8>,
) -> mpsc::Receiver<SearchEvent> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let nlen = needle_bytes.len();
        if nlen == 0 {
            let _ = tx.send(SearchEvent::Done);
            return;
        }
        let mut pos = 0;
        while pos + nlen <= file_size {
            let chunk_end = (pos + CHUNK).min(file_size);
            let mut matches = Vec::new();
            let mut p = pos;
            while p + nlen <= chunk_end {
                if mmap[p..p + nlen] == needle_bytes[..] {
                    matches.push((p, p + nlen));
                }
                p += 1;
            }
            if !matches.is_empty() {
                let _ = tx.send(SearchEvent::Chunk { matches });
            }
            pos = chunk_end;
        }
        let _ = tx.send(SearchEvent::Done);
    });
    rx
}

/// 搜索状态
///
/// 存储搜索模式（精确/nibble）、匹配结果列表、三级位图索引。
/// 支持增量搜索（extend）和跨 pack 查询（pack_matches）。
pub struct Search {
    needle: Needle,
    pack_size: usize,
    file_size: usize,
    pub ranges: Vec<(usize, usize)>,
    has_match_l0: Vec<u8>,
    has_match_l1: Vec<u8>,
    has_match_l2: Vec<u8>,
    pub match_count: usize,
    scanned: usize,
    pub label: String,
}

/// 搜索模式：精确字节序列或 nibble 模式
enum Needle {
    Lit(Vec<u8>),
    Pat(Vec<NibAtom>),
}

/// Nibble 级匹配原子
///
/// 两个 NibAtom 组成一个字节约束：高 4 位 + 低 4 位。
/// - Exact(n): 精确匹配 nibble 值 n
/// - Range(lo, hi): nibble 值在 [lo, hi] 范围内
/// - Any: 任意值（搜索语法中的 `x` 或 `z` 的一半）
#[derive(Clone)]
pub enum NibAtom {
    Exact(u8),
    Range(u8, u8),
    Any,
}

impl Search {
    /// 创建精确 hex 字节搜索
    pub fn new_hex(data: Vec<u8>, ps: usize, fs: usize, label: String) -> Self {
        let l0 = (fs + ps - 1) / ps;
        let l1 = (fs + L1_SIZE - 1) / L1_SIZE;
        let l2 = (fs + L2_SIZE - 1) / L2_SIZE;
        Self { needle: Needle::Lit(data), pack_size: ps, file_size: fs, ranges: Vec::new(),
            has_match_l0: vec![0u8; (l0 + 7) / 8],
            has_match_l1: vec![0u8; (l1 + 7) / 8],
            has_match_l2: vec![0u8; (l2 + 7) / 8],
            match_count: 0, scanned: 0, label }
    }
    /// 创建 nibble 模式搜索
    pub fn new_pat(pat: Vec<NibAtom>, ps: usize, fs: usize, label: String) -> Self {
        let l0 = (fs + ps - 1) / ps;
        let l1 = (fs + L1_SIZE - 1) / L1_SIZE;
        let l2 = (fs + L2_SIZE - 1) / L2_SIZE;
        Self { needle: Needle::Pat(pat), pack_size: ps, file_size: fs, ranges: Vec::new(),
            has_match_l0: vec![0u8; (l0 + 7) / 8],
            has_match_l1: vec![0u8; (l1 + 7) / 8],
            has_match_l2: vec![0u8; (l2 + 7) / 8],
            match_count: 0, scanned: 0, label }
    }
    /// 是否还有未扫描的数据
    pub fn has_more(&self) -> bool { self.scanned < self.file_size }

    /// 增量搜索：从 scanned 位置继续扫描，至少扫描到 min_off + CHUNK
    ///
    /// 返回 true 表示找到了新的匹配。
    pub fn extend(&mut self, mmap: &[u8], min_off: usize) -> bool {
        if min_off < self.scanned { return false; }
        let mut start = (self.scanned / CHUNK) * CHUNK;
        let mut found = false;
        while start < self.file_size && (start < min_off + CHUNK || self.ranges.is_empty()) {
            let end = (start + CHUNK).min(self.file_size);
            let new_matches: Vec<(usize, usize)> = match &self.needle {
                Needle::Lit(needle) => {
                    let nlen = needle.len();
                    if nlen == 0 { break; }
                    let mut out = Vec::new();
                    let mut pos = start;
                    loop {
                        pos = match mmap[pos..end].windows(nlen).position(|w| w == needle.as_slice()) {
                            Some(p) => pos + p, None => break,
                        };
                        let me = pos + nlen;
                        if self.ranges.last().map_or(true, |(_, e)| *e <= pos) {
                            out.push((pos, me));
                        }
                        pos += 1;
                    }
                    out
                }
                Needle::Pat(atoms) => {
                    let nbytes = atoms.len() / 2;
                    if nbytes == 0 { break; }
                    let mut out = Vec::new();
                    let mut pos = start;
                    while pos + nbytes <= end {
                        if atoms_match(atoms, mmap, pos) {
                            let me = pos + nbytes;
                            if self.ranges.last().map_or(true, |(_, e)| *e <= pos) {
                                out.push((pos, me));
                            }
                        }
                        pos += 1;
                    }
                    out
                }
            };
            for &(s, e) in &new_matches {
                self.ranges.push((s, e));
                self.match_count += 1;
                self.mark_all(s, e);
            }
            if !self.ranges.is_empty() && !found { found = true; }
            self.scanned = end;
            start = end;
        }
        found
    }
    /// 查找 min 位置之后的第一个匹配，必要时触发增量搜索
    pub fn find_after(&mut self, mmap: &[u8], min: usize) -> Option<usize> {
        if let Some(i) = self.ranges.iter().position(|(s, _)| *s >= min) { return Some(i); }
        self.extend(mmap, min);
        self.ranges.iter().position(|(s, _)| *s >= min)
    }
    /// 获取指定 pack 内的所有匹配范围（裁剪到 pack 边界）
    pub fn pack_matches(&self, pi: usize) -> Vec<(usize, usize)> {
        let base = pi * self.pack_size;
        let end = (base + self.pack_size).min(self.file_size);
        let mut ranges = Vec::new();
        for &(s, e) in &self.ranges {
            if s >= end { break; }
            if e > base {
                ranges.push((s.max(base), e.min(end)));
            }
        }
        ranges
    }
    /// 查找包含指定偏移的匹配索引
    pub fn idx_for_offset(&self, off: usize) -> Option<usize> {
        self.ranges.iter().position(|(s, e)| *s <= off && off < *e)
    }

    /// 在三级位图中标记 [start, end) 范围内有匹配
    ///
    /// 分别设置 L0（pack 粒度）、L1（1MB 粒度）、L2（1GB 粒度）的位。
    pub fn mark_all(&mut self, start: usize, end: usize) {
        let ep = (end - 1).max(start);
        for p in (start / self.pack_size)..=(ep / self.pack_size) {
            self.has_match_l0[p / 8] |= 1 << (p % 8);
        }
        for p in (start / L1_SIZE)..=(ep / L1_SIZE) {
            self.has_match_l1[p / 8] |= 1 << (p % 8);
        }
        for p in (start / L2_SIZE)..=(ep / L2_SIZE) {
            self.has_match_l2[p / 8] |= 1 << (p % 8);
        }
    }
}

/// 检查 data[pos..] 是否匹配 nibble 模式
fn atoms_match(atoms: &[NibAtom], data: &[u8], pos: usize) -> bool {
    atoms.chunks(2).enumerate().all(|(i, pair)| {
        let b = data[pos + i];
        let hi_ok = nib_match(&pair[0], b >> 4);
        let lo_ok = if pair.len() > 1 { nib_match(&pair[1], b & 0x0f) } else { true };
        hi_ok && lo_ok
    })
}

/// 单个 nibble 匹配检查
fn nib_match(a: &NibAtom, val: u8) -> bool {
    match a {
        NibAtom::Exact(n) => val == *n,
        NibAtom::Range(lo, hi) => val >= *lo && val <= *hi,
        NibAtom::Any => true,
    }
}

/// 搜索输入解析结果：精确 hex 或 nibble 模式
pub enum SearchKind {
    Hex { bytes: Vec<u8>, label: String },
    Pat { pat: Vec<NibAtom>, label: String },
}

/// 解析用户输入的搜索表达式
///
/// 支持格式：纯 hex（如 `4f2a`）、nibble 模式（如 `4x`、`[0-3]f`、`z`）。
pub fn parse_input(s: &str) -> Option<SearchKind> {
    let t = s.trim();
    if t.is_empty() { return None; }
    // pure hex
    let hex: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    if !hex.is_empty() && hex.len() % 2 == 0 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        if let Some(bytes) = decode_hex(&hex) {
            return Some(SearchKind::Hex { bytes, label: t.into() });
        }
    }
    // x/z/[] nibble pattern
    if let Some(pat) = compile_pat(t) {
        return Some(SearchKind::Pat { pat, label: t.into() });
    }
    None
}

/// 解析纯字符串搜索输入，返回 (标签, UTF-8 字节)
pub fn parse_str_input(s: &str) -> Option<(String, Vec<u8>)> {
    let t = s.trim();
    if t.is_empty() { return None; }
    Some((format!("\"{}\"", t), t.as_bytes().to_vec()))
}

/// 十六进制字符串解码为字节序列
fn decode_hex(s: &str) -> Option<Vec<u8>> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i+2], 16).ok()).collect()
}

/// 单个十六进制字符转 nibble 值
fn hv(c: char) -> Option<u8> { c.to_digit(16).map(|v| v as u8) }

/// 将搜索语法字符串分词为 NibAtom 序列
///
/// 支持：hex 数字、`x`（任意 nibble）、`z`（任意字节 = 两个 Any）、`[lo-hi]`（范围）。
fn tokenize(s: &str) -> Option<Vec<NibAtom>> {
    let ch: Vec<char> = s.to_lowercase().chars().collect();
    let mut atoms = Vec::new();
    let mut i = 0;
    while i < ch.len() {
        if ch[i].is_ascii_whitespace() { i += 1; continue; }
        if ch[i] == 'z' {
            atoms.push(NibAtom::Any);
            atoms.push(NibAtom::Any);
            i += 1;
        } else if ch[i] == 'x' {
            atoms.push(NibAtom::Any);
            i += 1;
        } else if ch[i] == '[' {
            let start = i + 1;
            let end = ch[start..].iter().position(|&c| c == ']')? + start;
            let inner: String = ch[start..end].iter().collect();
            let (lo_s, hi_s) = inner.split_once('-')?;
            let lo = hv(lo_s.trim().chars().next()?)?;
            let hi = hv(hi_s.trim().chars().next()?)?;
            atoms.push(NibAtom::Range(lo.min(hi), lo.max(hi)));
            i = end + 1;
        } else if ch[i].is_ascii_hexdigit() {
            atoms.push(NibAtom::Exact(hv(ch[i])?));
            i += 1;
        } else {
            return None;
        }
    }
    Some(atoms)
}

/// 编译搜索模式：分词后验证 nibble 数量为偶数（每字节 = 2 nibbles）
fn compile_pat(s: &str) -> Option<Vec<NibAtom>> {
    let atoms = tokenize(s)?;
    if atoms.is_empty() { return None; }
    if atoms.len() % 2 != 0 { return None; }
    Some(atoms)
}
