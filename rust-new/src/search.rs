use regex::bytes::Regex;
use std::collections::HashSet;

pub const FIND_CHUNK: usize = 1024 * 1024;
const CHUNK: usize = 1024 * 1024;

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
    Re(Regex),
}

impl Search {
    pub fn new_hex(data: Vec<u8>, ps: usize, fs: usize, label: String) -> Self {
        Self { needle: Needle::Lit(data), pack_size: ps, file_size: fs, ranges: Vec::new(), set: HashSet::new(), scanned: 0, label }
    }
    pub fn new_re(re: Regex, ps: usize, fs: usize, label: String) -> Self {
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
                Needle::Re(re) => {
                    let chunk = &mmap[start..end];
                    let mut p = 0;
                    while let Some(m) = re.find_at(chunk, p) {
                        let ms = start + m.start();
                        let me = start + m.end();
                        if self.ranges.last().map_or(true, |(_, e)| *e <= ms) {
                            self.ranges.push((ms, me));
                            for o in ms..me { self.set.insert(o); }
                        }
                        p = m.end();
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

pub fn parse_input(s: &str) -> Option<(String, bool, Vec<u8>, Option<Regex>)> {
    let t = s.trim();
    if t.is_empty() { return None; }
    // /regex/
    if t.starts_with('/') && t.ends_with('/') && t.len() >= 2 {
        let pat = &t[1..t.len()-1];
        if let Ok(re) = Regex::new(pat) { return Some((t.into(), true, vec![], Some(re))); }
        return None;
    }
    // pure hex
    let hex: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    if !hex.is_empty() && hex.len() % 2 == 0 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        if let Some(bytes) = decode_hex(&hex) { return Some((t.into(), false, bytes, None)); }
    }
    // advanced hex with x/z
    let lo = t.to_lowercase();
    if lo.contains('x') || lo.contains('z') {
        if let Some(re) = compile_adv(t) { return Some((t.into(), true, vec![], Some(re))); }
        return None;
    }
    // fallback regex
    let conv = convert_hex_pat(t);
    Regex::new(&conv).ok().map(|re| (t.into(), true, vec![], Some(re)))
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

fn compile_adv(s: &str) -> Option<Regex> {
    let clean: String = s.to_lowercase().chars().filter(|c| c.is_ascii_hexdigit() || *c == 'x' || *c == 'z').collect();
    if clean.is_empty() { return None; }
    let ch: Vec<char> = clean.chars().collect();
    let mut pat = String::new();
    let mut i = 0;
    while i < ch.len() {
        if ch[i] == 'z' { pat.push_str("[\\x00-\\xff]"); i += 1; }
        else if i + 1 >= ch.len() { return None; }
        else {
            let (a, b) = (ch[i], ch[i+1]);
            if a != 'x' && b != 'x' { pat.push_str(&format!("\\x{:02x}", hv(a)?*16+hv(b)?)); }
            else if a != 'x' && b == 'x' { pat.push_str(&format!("[\\x{:0}0-\\x{:0}f]", hv(a)?, hv(a)?)); }
            else if a == 'x' && b != 'x' {
                let v = hv(b)?;
                let alts: Vec<String> = (0..16u8).map(|hi| format!("\\x{:02x}", hi*16+v)).collect();
                pat.push_str(&format!("({})", alts.join("|")));
            } else { pat.push_str("[\\x00-\\xff]"); }
            i += 2;
        }
    }
    Regex::new(&pat).ok()
}

fn convert_hex_pat(s: &str) -> String {
    let ch: Vec<char> = s.chars().collect();
    let mut r = String::new();
    let mut i = 0;
    while i < ch.len() {
        if i+1 < ch.len() && ch[i].is_ascii_hexdigit() && ch[i+1].is_ascii_hexdigit()
            && (i == 0 || ch[i-1] != '\\') {
            r.push_str(&format!("\\x{:02x}", hv(ch[i]).unwrap_or(0)*16+hv(ch[i+1]).unwrap_or(0)));
            i += 2;
        } else { r.push(ch[i]); i += 1; }
    }
    r
}
