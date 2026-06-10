use std::collections::HashSet;

pub const FIND_CHUNK: usize = 1024 * 1024;
const CHUNK: usize = 1024 * 1024;

// ─── byte-level pattern matcher ──────────────────────────────
#[derive(Clone, Copy)]
pub struct BytePat { mask: u8, val: u8 }

impl BytePat {
    fn exact(b: u8) -> Self { Self { mask: 0xff, val: b } }
    fn hi_nibble(hi: u8) -> Self { Self { mask: 0xf0, val: hi << 4 } }
    fn lo_nibble(lo: u8) -> Self { Self { mask: 0x0f, val: lo } }
    fn any() -> Self { Self { mask: 0x00, val: 0 } }
    fn matches(&self, b: u8) -> bool { b & self.mask == self.val }
}

pub struct BytesPattern { pub pats: Vec<BytePat> }

impl BytesPattern {
    fn find_in(&self, data: &[u8], start: usize) -> Option<usize> {
        let nlen = self.pats.len();
        if nlen == 0 || start + nlen > data.len() { return None; }
        for i in start..=data.len() - nlen {
            if self.pats.iter().enumerate().all(|(j, p)| p.matches(data[i + j])) {
                return Some(i);
            }
        }
        None
    }
}

fn compile_bytes_pattern(s: &str) -> Option<BytesPattern> {
    let clean: String = s.to_lowercase().chars()
        .filter(|c| c.is_ascii_hexdigit() || *c == 'x' || *c == 'z')
        .collect();
    if clean.is_empty() { return None; }
    let ch: Vec<char> = clean.chars().collect();
    let mut pats = Vec::new();
    let mut i = 0;
    while i < ch.len() {
        if ch[i] == 'z' { pats.push(BytePat::any()); i += 1; }
        else if i + 1 >= ch.len() { return None; }
        else {
            let (a, b) = (ch[i], ch[i + 1]);
            if a != 'x' && b != 'x' { pats.push(BytePat::exact(hv(a)? * 16 + hv(b)?)); }
            else if a != 'x' && b == 'x' { pats.push(BytePat::hi_nibble(hv(a)?)); }
            else if a == 'x' && b != 'x' { pats.push(BytePat::lo_nibble(hv(b)?)); }
            else { pats.push(BytePat::any()); }
            i += 2;
        }
    }
    Some(BytesPattern { pats })
}

fn compile_fallback_pat(s: &str) -> Option<BytesPattern> {
    let ch: Vec<char> = s.chars().collect();
    let mut pats = Vec::new();
    let mut i = 0;
    while i < ch.len() {
        if i + 1 < ch.len() && ch[i].is_ascii_hexdigit() && ch[i + 1].is_ascii_hexdigit()
            && (i == 0 || ch[i - 1] != '\\')
        {
            pats.push(BytePat::exact(hv(ch[i]).unwrap_or(0) * 16 + hv(ch[i + 1]).unwrap_or(0)));
            i += 2;
        } else {
            pats.push(BytePat::exact(ch[i] as u8));
            i += 1;
        }
    }
    if pats.is_empty() { None } else { Some(BytesPattern { pats }) }
}

// ─── search engine ───────────────────────────────────────────
pub struct Search {
    needle: Needle,
    pack_size: usize,
    file_size: usize,
    pub ranges: Vec<(usize, usize)>,
    pub set: HashSet<usize>,
    scanned: usize,
    pub label: String,
}

enum Needle {
    Lit(Vec<u8>),
    Pat(BytesPattern),
    Re(regex_lite::Regex),
}

impl Search {
    pub fn new_hex(data: Vec<u8>, ps: usize, fs: usize, label: String) -> Self {
        Self { needle: Needle::Lit(data), pack_size: ps, file_size: fs, ranges: Vec::new(), set: HashSet::new(), scanned: 0, label }
    }
    pub fn new_pat(pat: BytesPattern, ps: usize, fs: usize, label: String) -> Self {
        Self { needle: Needle::Pat(pat), pack_size: ps, file_size: fs, ranges: Vec::new(), set: HashSet::new(), scanned: 0, label }
    }
    pub fn new_re(re: regex_lite::Regex, ps: usize, fs: usize, label: String) -> Self {
        Self { needle: Needle::Re(re), pack_size: ps, file_size: fs, ranges: Vec::new(), set: HashSet::new(), scanned: 0, label }
    }
    pub fn has_more(&self) -> bool { self.scanned < self.file_size }
    pub fn extend(&mut self, mmap: &[u8], min_off: usize) -> bool {
        if min_off < self.scanned { return false; }
        let mut start = (self.scanned / CHUNK) * CHUNK;
        let mut found = false;
        while start < self.file_size && (start < min_off + CHUNK || self.ranges.is_empty()) {
            let end = (start + CHUNK).min(self.file_size);
            match &self.needle {
                Needle::Lit(needle) => {
                    let nlen = needle.len();
                    let mut pos = start;
                    loop {
                        pos = match mmap[pos..end].windows(nlen).position(|w| w == needle.as_slice()) {
                            Some(p) => pos + p,
                            None => break,
                        };
                        let me = pos + nlen;
                        if self.ranges.last().map_or(true, |(_, e)| *e <= pos) {
                            self.ranges.push((pos, me));
                            for o in pos..me { self.set.insert(o); }
                        }
                        pos += 1;
                    }
                }
                Needle::Pat(pat) => {
                    let nlen = pat.pats.len();
                    let mut pos = start;
                    while let Some(p) = pat.find_in(mmap, pos.max(start)) {
                        let me = p + nlen;
                        if me > end { break; }
                        if self.ranges.last().map_or(true, |(_, e)| *e <= p) {
                            self.ranges.push((p, me));
                            for o in p..me { self.set.insert(o); }
                        }
                        pos = p + 1;
                    }
                }
                Needle::Re(re) => {
                    let chunk = &mmap[start..end];
                    let s = String::from_utf8_lossy(chunk);
                    let mut moff = 0;
                    while let Some(m) = re.find_at(&s, moff) {
                        let ms = start + m.start();
                        let me = start + m.end();
                        if self.ranges.last().map_or(true, |(_, e)| *e <= ms) {
                            self.ranges.push((ms, me));
                        }
                        moff = m.end();
                    }
                }
            }
            if !self.ranges.is_empty() && !found { found = true; }
            self.scanned = end;
            start = end;
        }
        found
    }
    pub fn find_after(&mut self, mmap: &[u8], min: usize) -> Option<usize> {
        if let Some(i) = self.ranges.iter().position(|(s, _)| *s >= min) { return Some(i); }
        self.extend(mmap, min);
        self.ranges.iter().position(|(s, _)| *s >= min)
    }
    pub fn pack_matches(&self, pi: usize) -> (Vec<(usize, usize)>, HashSet<usize>) {
        let base = pi * self.pack_size;
        let end = (base + self.pack_size).min(self.file_size);
        let mut ranges = Vec::new();
        let mut set = HashSet::new();
        for &(s, e) in &self.ranges {
            if s >= end { break; }
            if e > base {
                let rs = s.max(base);
                let re = e.min(end);
                ranges.push((rs, re));
                for o in rs..re { set.insert(o); }
            }
        }
        (ranges, set)
    }
    pub fn idx_for_offset(&self, off: usize) -> Option<usize> {
        self.ranges.iter().position(|(s, e)| *s <= off && off < *e)
    }
}

// ─── input parsing ───────────────────────────────────────────
pub enum SearchKind {
    Hex { bytes: Vec<u8>, label: String },
    Pat { pat: BytesPattern, label: String },
    Re { re: regex_lite::Regex, label: String },
    Str { bytes: Vec<u8>, label: String },
}

pub fn parse_input(s: &str) -> Option<SearchKind> {
    let t = s.trim();
    if t.is_empty() { return None; }
    // /regex/
    if t.starts_with('/') && t.ends_with('/') && t.len() >= 2 {
        let pat = &t[1..t.len()-1];
        if let Ok(re) = regex_lite::Regex::new(pat) {
            return Some(SearchKind::Re { re, label: t.into() });
        }
        return None;
    }
    // pure hex
    let hex: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    if !hex.is_empty() && hex.len() % 2 == 0 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        if let Some(bytes) = decode_hex(&hex) {
            return Some(SearchKind::Hex { bytes, label: t.into() });
        }
    }
    // advanced hex with x/z → manual byte pattern
    let lo = t.to_lowercase();
    if lo.contains('x') || lo.contains('z') {
        if let Some(pat) = compile_bytes_pattern(t) {
            return Some(SearchKind::Pat { pat, label: t.into() });
        }
        return None;
    }
    // fallback: convert hex-like pattern to bytes pattern
    if let Some(pat) = compile_fallback_pat(t) {
        return Some(SearchKind::Pat { pat, label: t.into() });
    }
    None
}

pub fn parse_str_input(s: &str) -> Option<(String, Vec<u8>)> {
    let t = s.trim();
    if t.is_empty() { return None; }
    Some((format!("\"{}\"", t), t.as_bytes().to_vec()))
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    (0..s.len()).step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i+2], 16).ok())
        .collect()
}

fn hv(c: char) -> Option<u8> { c.to_digit(16).map(|v| v as u8) }
