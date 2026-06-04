#!/usr/bin/env python3
"""
Windows binary file viewer/editor using ANSI escape sequences and msvcrt.
All features from read_bin.py adapted for native Windows console.
"""
import sys
import os
import re
import mmap
import msvcrt
import ctypes
import time

FIND_CHUNK_SIZE = 1024 * 1024

# ═══════════════════════════════════════════════════════════════════
# Enable ANSI escape processing in Windows console
# ═══════════════════════════════════════════════════════════════════
def _enable_vt():
    kernel32 = ctypes.windll.kernel32
    h = kernel32.GetStdHandle(-11)
    m = ctypes.c_uint32()
    if kernel32.GetConsoleMode(h, ctypes.byref(m)):
        kernel32.SetConsoleMode(h, m.value | 0x0004)

_enable_vt()

# ═══════════════════════════════════════════════════════════════════
# ANSI helpers
# ═══════════════════════════════════════════════════════════════════
def _w(s=''):
    sys.stdout.write(s)
    sys.stdout.flush()

def _goto(r, c):
    _w(f'\033[{r};{c}H')

def _cursor_hide():
    _w('\033[?25l')

def _cursor_show():
    _w('\033[?25h')

RST   = '\033[0m'
BOLD  = '\033[1m'
REV   = '\033[7m'
# foreground
F_RED     = '\033[31m'
F_GREEN   = '\033[32m'
F_YELLOW  = '\033[33m'
F_BLUE    = '\033[34m'
F_MAGENTA = '\033[35m'
F_CYAN    = '\033[36m'
F_WHITE   = '\033[37m'
F_BLACK   = '\033[30m'
# background
B_RED     = '\033[41m'
B_GREEN   = '\033[42m'
B_YELLOW  = '\033[43m'
B_BLUE    = '\033[44m'
B_MAGENTA = '\033[45m'
B_CYAN    = '\033[46m'
B_WHITE   = '\033[47m'

# Combined color codes matching curses color pairs
C_NULL      = F_RED                                    # null byte
C_RETURN    = F_WHITE + B_BLUE                         # \r
C_YELLOW_BG = F_BLACK + B_YELLOW                       # pair 3
C_CONTROL   = F_BLACK + B_RED                          # control chars
C_NORMAL    = F_WHITE                                  # printable
C_CYAN      = F_BLACK + B_CYAN                         # 0x80-0xbf first nibble
C_BLUE_BG   = F_BLACK + B_BLUE                         # 0x80-0xbf second nibble
C_MAGENTA1  = F_BLACK + B_MAGENTA                      # high bytes
C_MAGENTA2  = F_BLACK + B_MAGENTA
C_GREEN_HEX = F_BLACK + B_GREEN                        # hex printable
C_GREEN_HEX2= F_BLACK + B_GREEN
C_MATCH     = F_BLACK + B_YELLOW                       # search match
C_MATCH_SEL = F_RED + B_YELLOW                         # current match
C_CURSOR    = F_WHITE + B_RED                          # edit cursor nibble

# ═══════════════════════════════════════════════════════════════════
# Key constants
# ═══════════════════════════════════════════════════════════════════
K_UP    = '__UP__'
K_DOWN  = '__DOWN__'
K_LEFT  = '__LEFT__'
K_RIGHT = '__RIGHT__'
K_HOME  = '__HOME__'
K_PGUP  = '__PGUP__'
K_PGDN  = '__PGDN__'
K_END   = '__END__'
K_ESC   = '__ESC__'
K_TAB   = '__TAB__'
K_ENTER = '__ENTER__'
K_RESIZE = '__RESIZE__'

_EXT_MAP = {
    (0xe0, 72): K_UP,
    (0xe0, 80): K_DOWN,
    (0xe0, 75): K_LEFT,
    (0xe0, 77): K_RIGHT,
    (0xe0, 71): K_HOME,
    (0xe0, 73): K_PGUP,
    (0xe0, 81): K_PGDN,
    (0xe0, 79): K_END,
}

# Extended key codes indexed by scan code only (works for both \xe0 and \x00 prefixes)
_EXT_SCAN_MAP = {
    72: K_UP, 80: K_DOWN, 75: K_LEFT, 77: K_RIGHT,
    71: K_HOME, 73: K_PGUP, 81: K_PGDN, 79: K_END,
}
# ANSI CSI sequences: \x1b[<code>
_ANSI_MAP = {
    65: K_UP, 66: K_DOWN, 67: K_RIGHT, 68: K_LEFT,
    72: K_HOME, 70: K_END, 53: K_PGUP, 54: K_PGDN,
}

def read_key():
    """Read a single key event. Returns a single-char string, a K_* constant, or None."""
    ch = msvcrt.getch()
    if ch in (b'\xe0', b'\x00'):
        ch2 = msvcrt.getch()
        return _EXT_SCAN_MAP.get(ch2[0])
    c = ch[0]
    if c == 27:
        if msvcrt.kbhit():
            ch2 = msvcrt.getch()
            c2 = ch2[0]
            if c2 == 91 or c2 == 79:  # '[' or 'O' (application mode)
                if msvcrt.kbhit():
                    ch3 = msvcrt.getch()
                    c3 = ch3[0]
                    if c3 == 53 or c3 == 54:
                        if msvcrt.kbhit():
                            msvcrt.getch()
                    return _ANSI_MAP.get(c3, K_ESC)
            return K_ESC
        return K_ESC
    if c == 9:
        return K_TAB
    if c in (10, 13):
        return K_ENTER
    if 32 <= c <= 126:
        return chr(c)
    if c == 8 or c == 127:
        return '\x08'
    return None

# ═══════════════════════════════════════════════════════════════════
# Utility
# ═══════════════════════════════════════════════════════════════════
def format_size(size):
    for unit in ['B', 'KB', 'MB', 'GB']:
        if size < 1024.0:
            return f"{size:.1f}{unit}" if unit != 'B' else f"{size}{unit}"
        size /= 1024.0
    return f"{size:.1f}TB"

def get_byte_display(b, mode):
    """Return plain 2-char string for a byte (no ANSI codes)."""
    if mode == 'ascii':
        if b == 0:
            return ". "
        if b == 0x0d:
            return "\\r"
        if b == 10:
            return "⏎ "
        if b == 0x1b:
            return "\\e"
        if 0x01 <= b <= 0x1f:
            return f"{b:02x}"
        if b == 0x20:
            return "· "
        if 0x21 <= b <= 0x7e:
            return f"{b:c} "
        if 0x80 <= b <= 0xbf:
            return f"{b:02x}"
        return f"{b:02x}"
    else:
        if b == 0:
            return ". "
        if 0x20 <= b <= 0x7e:
            return f"{b:02x}"
        return f"{b:02x}"

def ansi_byte(b, mode):
    """Return ANSI-colored 2-char string for a byte."""
    if mode == 'ascii':
        return _ansi_byte_ascii(b)
    else:
        return _ansi_byte_hex(b)

def _ansi_byte_ascii(b):
    if b == 0:
        return f'{C_NULL}. {RST}'
    if b == 0x0d:
        return f'{C_RETURN}\\r{RST}'
    if b == 10:
        return f'{C_NORMAL}⏎ {RST}'
    if b == 0x1b:
        return f'{C_CONTROL}\\e{RST}'
    if 0x01 <= b <= 0x1f:
        return f'{C_CONTROL}{b:02x}{RST}'
    if b == 0x20:
        return f'{C_NORMAL}· {RST}'
    if 0x21 <= b <= 0x7e:
        return f'{C_NORMAL}{b:c} {RST}'
    if 0x80 <= b <= 0xbf:
        s = f'{b:02x}'
        return f'{C_CYAN}{s[0]}{C_BLUE_BG}{s[1]}{RST}'
    s = f'{b:02x}'
    return f'{C_MAGENTA1}{s[0]}{C_MAGENTA2}{s[1]}{RST}'

def _ansi_byte_hex(b):
    if b == 0:
        return f'{C_NULL}. {RST}'
    s = f'{b:02x}'
    if 0x20 <= b <= 0x7e:
        return f'{C_GREEN_HEX}{s[0]}{C_GREEN_HEX2}{s[1]}{RST}'
    if b == 0x0d:
        return f'{C_RETURN}{s[0]}{C_RETURN}{s[1]}{RST}'
    if b == 10:
        return f'{C_NORMAL}{s[0]}{s[1]}{RST}'
    if b == 0x1b:
        return f'{C_CONTROL}{s[0]}{C_CONTROL}{s[1]}{RST}'
    if 0x01 <= b <= 0x1f:
        return f'{C_CONTROL}{s[0]}{C_CONTROL}{s[1]}{RST}'
    if 0x80 <= b <= 0xbf:
        return f'{C_CYAN}{s[0]}{C_BLUE_BG}{s[1]}{RST}'
    return f'{C_MAGENTA1}{s[0]}{C_MAGENTA2}{s[1]}{RST}'

def init_gradient_colors():
    """Return list of ANSI gradient background strings for header."""
    start_r, start_g, start_b = 102, 102, 255
    end_r, end_g, end_b = 102, 255, 102
    steps = 16
    colors = []
    for i in range(steps):
        r = start_r + (end_r - start_r) * i // max(1, steps - 1)
        g = start_g + (end_g - start_g) * i // max(1, steps - 1)
        b = start_b + (end_b - start_b) * i // max(1, steps - 1)
        colors.append(f'\033[48;2;{r};{g};{b}m{F_BLACK}')
    return colors

def draw_header_with_gradient(text, colors):
    """Return ANSI-colored header string with gradient."""
    if not colors:
        return f'{C_BLUE_BG}{text}{RST}'
    leading_spaces = 4
    grad_part = text[leading_spaces:]
    grad_len = len(grad_part)
    result = C_BLUE_BG + text[:leading_spaces]
    for idx, ch in enumerate(grad_part):
        ci = idx * (len(colors) - 1) // max(1, grad_len - 1)
        result += colors[ci] + ch
    result += RST
    return result

# ═══════════════════════════════════════════════════════════════════
# Frame drawing
# ═══════════════════════════════════════════════════════════════════
def draw_frame(mm, file_size, pack_size, pack_idx, scroll_top, rows, cols,
               mode, base_name, header_colors,
               matches_set=None, current_match_range=None,
               edit_mode=False, cursor_byte=None, cursor_nibble=0):
    """Build and write the entire frame to console."""
    lines = []
    base_offset = pack_idx * pack_size
    data = mm[base_offset:base_offset+pack_size]

    size_str = format_size(file_size)
    total_packs = (file_size + pack_size - 1) // pack_size
    pack_str = f"{pack_idx+1:x} / {total_packs:x}"
    mode_str = "[ASCII]" if mode == 'ascii' else "[HEX]"
    if edit_mode:
        mode_str += " [EDIT]"

    lines.append(f'{C_NORMAL}{base_name}  ({size_str}){RST}')
    lines.append(f'{C_NORMAL}pack: {pack_str}  {mode_str}{RST}')
    header_text = "    0 1 2 3 4 5 6 7 8 9 a b c d e f "
    lines.append(draw_header_with_gradient(header_text, header_colors))

    max_data_rows = max(0, rows - 4)
    total_rows = (len(data) + 15) // 16
    if scroll_top > total_rows - max_data_rows:
        scroll_top = max(0, total_rows - max_data_rows)
    start_row = scroll_top
    end_row = min(start_row + max_data_rows, total_rows)

    for r in range(start_row, end_row):
        offset = r * 16
        parts = [f'{F_CYAN}{r:02x}  {RST}']
        for i in range(16):
            byte_off = offset + i
            if byte_off < len(data):
                b = data[byte_off]
                global_off = base_offset + byte_off
                is_cursor = (edit_mode and cursor_byte == global_off)
                if is_cursor:
                    disp = get_byte_display(b, mode)
                    if mode == 'hex' and cursor_nibble == 0:
                        parts.append(f'{C_CURSOR}{disp[0]}{C_NORMAL}{disp[1]}{RST}')
                    elif mode == 'hex' and cursor_nibble == 1:
                        parts.append(f'{C_NORMAL}{disp[0]}{C_CURSOR}{disp[1]}{RST}')
                    else:
                        parts.append(f'{C_CURSOR}{disp[0]}{disp[1]}{RST}')
                    continue
                if current_match_range is not None:
                    start, end = current_match_range
                    if start <= global_off < end:
                        disp = get_byte_display(b, mode)
                        parts.append(f'{C_MATCH_SEL}{disp[0]}{disp[1]}{RST}')
                        continue
                if matches_set and global_off in matches_set:
                    disp = get_byte_display(b, mode)
                    parts.append(f'{C_MATCH}{disp[0]}{disp[1]}{RST}')
                    continue
                parts.append(ansi_byte(b, mode))
            else:
                parts.append('  ')
        lines.append(''.join(parts))

    _w('\033[H' + '\r\n'.join(lines) + '\033[J')
    return scroll_top

# ═══════════════════════════════════════════════════════════════════
# Line input
# ═══════════════════════════════════════════════════════════════════
def _line_input(prompt, rows):
    """Read a line of text at the bottom of console."""
    _goto(rows, 1)
    _w('\033[K' + prompt + ' ')
    _cursor_show()
    buf = []
    while True:
        ch = msvcrt.getwch()
        if ch == '\r' or ch == '\n':
            break
        elif ch == '\x1b':
            buf.clear()
            break
        elif ch == '\x08' or ch == '\x7f':
            if buf:
                buf.pop()
                _w('\b \b')
        elif ord(ch) >= 32:
            buf.append(ch)
            _w(ch)
    _cursor_hide()
    _goto(rows, 1)
    _w('\033[K')
    line = ''.join(buf)
    return line if line else None

def input_hex(prompt, rows):
    val = _line_input(prompt, rows)
    if val is None:
        return None
    val = val.strip()
    if val.lower().startswith('0x'):
        val = val[2:]
    val = re.sub(r'[^0-9a-fA-F]', '', val)
    if not val:
        return None
    try:
        return int(val, 16)
    except:
        return None

def input_string(prompt, rows):
    return _line_input(prompt, rows)

def show_message(msg, rows, delay=1):
    _goto(rows, 1)
    _w('\033[K' + msg)
    time.sleep(delay)
    _goto(rows, 1)
    _w('\033[K')

def compile_advanced_hex_pattern(s):
    s = s.lower()
    s = re.sub(r'[^0-9a-fxz]', '', s)
    if not s:
        return None
    parts = []
    i = 0
    n = len(s)
    while i < n:
        ch = s[i]
        if ch == 'z':
            parts.append(b'[\x00-\xff]')
            i += 1
        else:
            if i + 1 >= n:
                return None
            a = ch
            b = s[i+1]
            if a not in '0123456789abcdefx' or b not in '0123456789abcdefx':
                return None
            if a != 'x' and b != 'x':
                parts.append(f'\\x{a}{b}'.encode())
            elif a != 'x' and b == 'x':
                parts.append(f'[\\x{a}0-\\x{a}f]'.encode())
            elif a == 'x' and b != 'x':
                candidates = [f'\\x{i}{b}' for i in '0123456789abcdef']
                parts.append(f'({"|".join(candidates)})'.encode())
            else:
                parts.append(b'[\x00-\xff]')
            i += 2
    try:
        pattern = b''.join(parts)
        return re.compile(pattern)
    except re.error:
        return None

def input_search(rows):
    s = _line_input("search hex:", rows)
    if s is None:
        return None, None, None
    s = s.strip()
    user_input = s
    if s.startswith('/') and s.endswith('/') and len(s) >= 2:
        pattern = s[1:-1]
        try:
            regex = re.compile(pattern.encode('latin-1'))
            return 'regex', regex, user_input
        except re.error:
            show_message("Invalid regex", rows, 1)
            return None, None, None
    hex_test = re.sub(r'\s', '', s)
    if re.fullmatch(r'[0-9a-fA-F]*', hex_test) and len(hex_test) % 2 == 0 and len(hex_test) > 0:
        try:
            return 'hex', bytes.fromhex(hex_test), user_input
        except:
            pass
    if 'x' in s.lower() or 'z' in s.lower():
        regex = compile_advanced_hex_pattern(s)
        if regex:
            return 'regex', regex, user_input
        else:
            show_message("Invalid advanced hex pattern", rows, 1)
            return None, None, None
    def hex_to_byte(m):
        return '\\x' + m.group(1)
    conv = re.sub(r'(?<!\\)([0-9a-fA-F]{2})', hex_to_byte, s)
    try:
        regex = re.compile(conv.encode('latin-1'))
        return 'regex', regex, user_input
    except re.error:
        show_message("Invalid regex", rows, 1)
        return None, None, None

# ═══════════════════════════════════════════════════════════════════
# SearchAccumulator (same logic as original)
# ═══════════════════════════════════════════════════════════════════
class SearchAccumulator:
    def __init__(self, mm, search_type, needle, pack_size, user_pattern):
        self.mm = mm
        self.search_type = search_type
        self.needle = needle
        self.pack_size = pack_size
        self.file_size = len(mm)
        self.match_ranges = []
        self.matches_set = set()
        self.scanned_until = 0
        self.user_pattern = user_pattern

    def extend_scan(self, min_offset):
        if min_offset < self.scanned_until:
            return False
        start = self.scanned_until
        start = (start // FIND_CHUNK_SIZE) * FIND_CHUNK_SIZE
        found = False
        while start < self.file_size and (start < min_offset + FIND_CHUNK_SIZE or not self.match_ranges):
            end = min(start + FIND_CHUNK_SIZE, self.file_size)
            if self.search_type == 'regex':
                chunk = self.mm[start:end]
                pos = 0
                while True:
                    m = self.needle.search(chunk, pos)
                    if not m:
                        break
                    match_start = start + m.start()
                    match_end = start + m.end()
                    if not self.match_ranges or self.match_ranges[-1][1] <= match_start:
                        self.match_ranges.append((match_start, match_end))
                        for off in range(match_start, match_end):
                            self.matches_set.add(off)
                    pos = m.end()
            else:
                needle = self.needle
                nlen = len(needle)
                pos = start
                while True:
                    pos = self.mm.find(needle, pos, end)
                    if pos == -1:
                        break
                    match_start = pos
                    match_end = pos + nlen
                    if not self.match_ranges or self.match_ranges[-1][1] <= match_start:
                        self.match_ranges.append((match_start, match_end))
                        for off in range(match_start, match_end):
                            self.matches_set.add(off)
                    pos += 1
            if self.match_ranges and not found:
                found = True
            self.scanned_until = end
            start = end
        return found

    def get_match_index_for_offset(self, offset):
        for i, (s, e) in enumerate(self.match_ranges):
            if s <= offset < e:
                return i
        return -1

    def get_current_pack_matches(self, pack_idx):
        base = pack_idx * self.pack_size
        end = min(base + self.pack_size, self.file_size)
        pack_ranges = []
        pack_set = set()
        for s, e in self.match_ranges:
            if s >= end:
                break
            if e > base:
                rs = max(s, base)
                re = min(e, end)
                pack_ranges.append((rs, re))
                for off in range(rs, re):
                    pack_set.add(off)
        return pack_ranges, pack_set

    def has_more(self):
        return self.scanned_until < self.file_size

    def find_next_match_after_offset(self, min_offset):
        for i, (s, e) in enumerate(self.match_ranges):
            if s >= min_offset:
                return i
        if self.extend_scan(min_offset):
            for i, (s, e) in enumerate(self.match_ranges):
                if s >= min_offset:
                    return i
        return -1

# ═══════════════════════════════════════════════════════════════════
# Help screen
# ═══════════════════════════════════════════════════════════════════
def show_help(rows, cols):
    help_lines = [
        "=== READ_BIN HELP ===",
        "",
        "Navigation (non-search mode):",
        "  hjkl / arrows            Move cursor / scroll",
        "  H / L                    Jump +-16 packs",
        "  J / K                    Scroll one screen",
        "  PGUP / PGDN              Scroll half screen",
        "  HOME                     Go to first pack",
        "  g                        Go to pack (hex input)",
        "",
        "Search mode (after pressing f/F):",
        "  up / down (or j/k)       Nav matches in pack",
        "  left / right (or h/l)    Jump global prev/next",
        "  O / P                    Jump +-1MB block",
        "  H / L                    Jump +-16 packs",
        "  HOME                     Jump to first match",
        "  g                        Jump to pack + match",
        "  ESC                      Clear search highlight",
        "",
        "Search input:",
        "  f                        hex / regex / adv hex",
        "  F                        plain UTF-8 string",
        "",
        "Edit mode (press i):",
        "  ESC                      Exit edit mode",
        "  arrows                   Move cursor",
        "  0-9a-fA-F                Edit nibble (hex mode)",
        "  Enter                    Insert newline",
        "  Tab                      Insert tab",
        "  any character            Insert byte (ASCII)",
        "",
        "Other:",
        "  m                        Toggle ASCII / HEX",
        "  q                        Quit (save prompt)",
        "  ?                        Show this help",
        "",
        "Press any key to close",
    ]
    _w('\033[2J\033[H')
    start = 0
    per_page = max(1, rows - 3)
    while True:
        _w('\033[H')
        for i in range(start, min(start + per_page, len(help_lines))):
            line = help_lines[i]
            _w(f'{F_WHITE}{line}{RST}\033[K\r\n')
        if start + per_page < len(help_lines):
            _w(f'{F_YELLOW}-- More ({start+per_page}/{len(help_lines)}) PgDn/Enter continue q quit --{RST}')
        else:
            _w(f'{F_YELLOW}-- Press any key to close --{RST}')
        k = msvcrt.getch()
        if start + per_page >= len(help_lines):
            break
        if k in (b'q', b'Q', b'\x1b'):
            break
        if k == b'\xe0':
            k2 = msvcrt.getch()
            if k2[0] == 81:
                start = min(start + per_page, len(help_lines) - 1)
        elif k in (b'\r', b'\n', b' '):
            start += per_page

# ═══════════════════════════════════════════════════════════════════
# Main
# ═══════════════════════════════════════════════════════════════════
def main():
    if len(sys.argv) < 2:
        raise SystemExit(f"Usage: {sys.argv[0]} <filename>")

    filename = sys.argv[1]
    fd = os.open(filename, os.O_RDWR | os.O_BINARY)
    mm = mmap.mmap(fd, 0, access=mmap.ACCESS_WRITE)
    os.close(fd)
    file_size = len(mm)
    base_name = os.path.basename(filename)
    pack_size = 4096
    total_packs = (file_size + pack_size - 1) // pack_size
    if total_packs == 0:
        return

    current_pack = 0
    scroll_top = 0
    mode = 'ascii'
    header_colors = init_gradient_colors()
    _cursor_hide()

    # search state
    search_accum = None
    search_active = False
    current_pack_ranges = []
    current_pack_set = set()
    current_pack_match_idx = -1
    current_global_match_idx = -1

    # edit state
    edit_mode = False
    cursor_byte = 0
    cursor_nibble = 0
    dirty = False
    undo_map = {}

    def get_term_size():
        s = os.get_terminal_size()
        return s.lines, s.columns

    def refresh_current_pack_display():
        nonlocal current_pack_ranges, current_pack_set, current_pack_match_idx
        if not search_accum:
            return
        ranges, sset = search_accum.get_current_pack_matches(current_pack)
        current_pack_ranges = ranges
        current_pack_set = sset
        if current_global_match_idx != -1 and current_global_match_idx < len(search_accum.match_ranges):
            global_start, _ = search_accum.match_ranges[current_global_match_idx]
            idx = -1
            for i, (s, e) in enumerate(current_pack_ranges):
                if s <= global_start < e:
                    idx = i
                    break
            current_pack_match_idx = idx
        else:
            current_pack_match_idx = -1

    def jump_to_global_match(idx):
        nonlocal current_pack, scroll_top, current_global_match_idx
        if idx < 0 or idx >= len(search_accum.match_ranges):
            return False
        start, _ = search_accum.match_ranges[idx]
        current_global_match_idx = idx
        new_pack = start // pack_size
        offset_in_pack = start % pack_size
        new_row = offset_in_pack // 16
        data_len = min(pack_size, file_size - new_pack * pack_size)
        total_rows = (data_len + 15) // 16
        rows, _ = get_term_size()
        max_data_rows = max(0, rows - 4)
        scroll_top = max(0, min(total_rows - max_data_rows, new_row - max_data_rows // 2))
        current_pack = new_pack
        refresh_current_pack_display()
        return True

    def jump_to_next_global_match():
        if not search_accum or not search_accum.match_ranges:
            return False
        new_idx = current_global_match_idx + 1
        if new_idx < len(search_accum.match_ranges):
            return jump_to_global_match(new_idx)
        last_match_end = search_accum.match_ranges[-1][1]
        if search_accum.extend_scan(last_match_end + 1):
            new_idx = current_global_match_idx + 1
            if new_idx < len(search_accum.match_ranges):
                return jump_to_global_match(new_idx)
        return False

    def jump_to_prev_global_match():
        if not search_accum or not search_accum.match_ranges:
            return False
        new_idx = current_global_match_idx - 1
        if new_idx >= 0:
            return jump_to_global_match(new_idx)
        return False

    def ensure_cursor_visible():
        nonlocal current_pack, scroll_top
        pack_of_cursor = cursor_byte // pack_size
        if pack_of_cursor != current_pack:
            current_pack = pack_of_cursor
        offset_in_pack = cursor_byte % pack_size
        row_of_cursor = offset_in_pack // 16
        rows, _ = get_term_size()
        max_data_rows = max(0, rows - 4)
        if row_of_cursor < scroll_top:
            scroll_top = row_of_cursor
        elif row_of_cursor >= scroll_top + max_data_rows:
            scroll_top = row_of_cursor - max_data_rows + 1
        data_len = min(pack_size, file_size - current_pack * pack_size)
        total_rows = (data_len + 15) // 16
        scroll_top = max(0, min(scroll_top, total_rows - max_data_rows))

    def modify_byte(byte_offset, new_value):
        nonlocal dirty
        old = mm[byte_offset]
        if old != new_value:
            if byte_offset not in undo_map:
                undo_map[byte_offset] = old
            mm[byte_offset] = new_value
            dirty = True

    def edit_hex_input(ch):
        nonlocal cursor_byte, cursor_nibble, dirty
        if '0' <= ch <= '9':
            nib = ord(ch) - ord('0')
        elif 'a' <= ch <= 'f':
            nib = ord(ch) - ord('a') + 10
        elif 'A' <= ch <= 'F':
            nib = ord(ch) - ord('A') + 10
        else:
            return
        if cursor_byte >= file_size:
            return
        cur = mm[cursor_byte]
        if cursor_nibble == 0:
            new_byte = (cur & 0x0f) | (nib << 4)
        else:
            new_byte = (cur & 0xf0) | nib
        modify_byte(cursor_byte, new_byte)
        if cursor_nibble == 0:
            cursor_nibble = 1
        else:
            cursor_nibble = 0
            cursor_byte += 1
            if cursor_byte >= file_size:
                cursor_byte = file_size - 1
                cursor_nibble = 1
        ensure_cursor_visible()

    def edit_ascii_input(ch):
        nonlocal cursor_byte, dirty
        if cursor_byte >= file_size:
            return
        new_byte = ord(ch) & 0xff
        modify_byte(cursor_byte, new_byte)
        cursor_byte += 1
        if cursor_byte >= file_size:
            cursor_byte = file_size - 1
        ensure_cursor_visible()

    prev_rows, prev_cols = get_term_size()

    # ── Main loop ──
    while True:
        rows, cols = get_term_size()
        if rows != prev_rows or cols != prev_cols:
            prev_rows, prev_cols = rows, cols
        max_data_rows = max(0, rows - 4)

        current_match_range = None
        if search_active and search_accum and current_global_match_idx != -1:
            if current_global_match_idx < len(search_accum.match_ranges):
                current_match_range = search_accum.match_ranges[current_global_match_idx]

        scroll_top = draw_frame(mm, file_size, pack_size, current_pack, scroll_top,
                                rows, cols, mode, base_name, header_colors,
                                current_pack_set if search_active else None,
                                current_match_range,
                                edit_mode, cursor_byte, cursor_nibble)

        status = ""
        if edit_mode:
            if mode == 'ascii':
                status = "[EDIT ASCII] Move: arrows, type char/Enter/Tab to edit, ESC to exit"
            else:
                status = "[EDIT HEX] Move: arrows, 0-9a-f to edit nibble, ESC to exit"
        elif search_active and search_accum:
            total = len(search_accum.match_ranges)
            plus = "+" if search_accum.has_more() else ""
            display = search_accum.user_pattern
            if len(display) > 24:
                display = display[:24] + "..."
            cur = current_global_match_idx + 1 if current_global_match_idx != -1 else 0
            status = f"Search: {display} [{cur}/{total}{plus}]  up/dn: in-pack | left/right: global | ESC clear"
        else:
            status = "hjkl/arrows: move | H/L: +-16p | J/K: +-scr | PGUP/PGDN: half-scr | O/P: +-1MB | HOME: first | g: goto | f: search | F: str | i: edit | m: mode | ?: help | q: quit"
            if dirty:
                status = "[MODIFIED] " + status

        _goto(rows, 1)
        _w('\033[K' + C_NORMAL + status[:cols-1] + RST if status else '')

        key = read_key()

        # ── Edit mode ──
        if edit_mode:
            if key == K_ESC:
                edit_mode = False
                _cursor_hide()
                _goto(rows, 1)
                _w('\033[K')
                continue
            elif key == K_LEFT:
                if mode == 'ascii':
                    if cursor_byte > 0:
                        cursor_byte -= 1
                else:
                    if cursor_nibble == 0:
                        if cursor_byte > 0:
                            cursor_byte -= 1
                            cursor_nibble = 1
                    else:
                        cursor_nibble = 0
                ensure_cursor_visible()
                continue
            elif key == K_RIGHT:
                if mode == 'ascii':
                    if cursor_byte + 1 < file_size:
                        cursor_byte += 1
                else:
                    if cursor_nibble == 0:
                        cursor_nibble = 1
                    else:
                        if cursor_byte + 1 < file_size:
                            cursor_byte += 1
                            cursor_nibble = 0
                ensure_cursor_visible()
                continue
            elif key == K_UP:
                new_byte = cursor_byte - 16
                if new_byte >= 0:
                    cursor_byte = new_byte
                ensure_cursor_visible()
                continue
            elif key == K_DOWN:
                new_byte = cursor_byte + 16
                if new_byte < file_size:
                    cursor_byte = new_byte
                ensure_cursor_visible()
                continue
            elif mode == 'hex':
                if isinstance(key, str) and len(key) == 1 and key in '0123456789abcdefABCDEF':
                    edit_hex_input(key)
                continue
            else:
                if key == K_ENTER:
                    edit_ascii_input('\n')
                elif key == K_TAB:
                    edit_ascii_input('\t')
                elif isinstance(key, str) and len(key) == 1 and '\x20' <= key <= '\x7e':
                    edit_ascii_input(key)
                continue

        # ── Help ──
        if key == '?':
            show_help(rows, cols)
            continue

        # ── Search navigation (within pack) ──
        if search_active and key in (K_UP, K_DOWN, 'k', 'j'):
            if current_pack_ranges:
                if key in (K_UP, 'k'):
                    new_idx = (current_pack_match_idx - 1) % len(current_pack_ranges)
                else:
                    new_idx = (current_pack_match_idx + 1) % len(current_pack_ranges)
                if new_idx != current_pack_match_idx:
                    current_pack_match_idx = new_idx
                    match_start, _ = current_pack_ranges[current_pack_match_idx]
                    gidx = search_accum.get_match_index_for_offset(match_start)
                    if gidx != -1:
                        current_global_match_idx = gidx
                        offset_in_pack = match_start % pack_size
                        new_row = offset_in_pack // 16
                        data_len = min(pack_size, file_size - current_pack * pack_size)
                        total_rows = (data_len + 15) // 16
                        rows, _ = get_term_size()
                        max_data_rows = max(0, rows - 4)
                        scroll_top = max(0, min(total_rows - max_data_rows, new_row - max_data_rows // 2))
            continue

        # ── Global match navigation ──
        if search_active and key in (K_LEFT, K_RIGHT, 'h', 'l'):
            if key in (K_RIGHT, 'l'):
                if not jump_to_next_global_match():
                    show_message("No more matches", rows, 1)
            else:
                if not jump_to_prev_global_match():
                    show_message("No previous matches", rows, 1)
            continue

        # ── ESC clear ──
        if key == K_ESC:
            if search_active:
                search_active = False
                search_accum = None
                current_pack_ranges = []
                current_pack_set = set()
                current_pack_match_idx = -1
                current_global_match_idx = -1
            continue

        # ── Normal commands ──
        if key == 'q':
            if dirty:
                # quit dialog
                selected = 0  # 0=Yes, 1=No
                while True:
                    _goto(rows, 1)
                    _w(f'\033[K{REV} Save changes before quitting? [ Yes ] [ No ]{RST} {REV}←→ select Enter confirm{RST}')
                    c = read_key()
                    if c == K_LEFT or c == 'h':
                        selected = 0
                        _goto(rows, 1)
                        _w(f'\033[K Save changes before quitting? {REV}[ Yes ]{RST} [ No ]          ')
                    elif c == K_RIGHT or c == 'l':
                        selected = 1
                        _goto(rows, 1)
                        _w(f'\033[K Save changes before quitting? [ Yes ] {REV}[ No ]{RST}          ')
                    elif c == 'y' or c == 'Y':
                        selected = 0
                        break
                    elif c == 'n' or c == 'N':
                        selected = 1
                        break
                    elif c == K_ENTER or c == ' ':
                        break
                    elif c == K_ESC:
                        selected = 1
                        break
                _goto(rows, 1)
                _w('\033[K')
                if selected == 0:
                    mm.flush()
                    show_message("Saved.", rows, 1)
                else:
                    for off, orig in undo_map.items():
                        mm[off] = orig
                    mm.flush()
                    undo_map.clear()
                    dirty = False
                break
            else:
                break
        elif key == K_UP or key == 'k':
            if scroll_top > 0:
                scroll_top -= 1
        elif key == K_DOWN or key == 'j':
            data_len = min(pack_size, file_size - current_pack * pack_size)
            total_rows = (data_len + 15) // 16
            if scroll_top + max_data_rows < total_rows:
                scroll_top += 1
        elif key == K_LEFT or key == 'h':
            if not search_active and current_pack > 0:
                current_pack -= 1
                scroll_top = 0
        elif key == K_RIGHT or key == 'l':
            if not search_active and current_pack + 1 < total_packs:
                current_pack += 1
                scroll_top = 0
        elif key == 'K':
            scroll_top = max(0, scroll_top - max_data_rows)
        elif key == 'J':
            data_len = min(pack_size, file_size - current_pack * pack_size)
            total_rows = (data_len + 15) // 16
            scroll_top = min(total_rows - max_data_rows, scroll_top + max_data_rows)
        elif key == 'H':
            target_pack = max(0, current_pack - 16)
            if search_active:
                target_offset = target_pack * pack_size
                idx = search_accum.find_next_match_after_offset(target_offset)
                if idx != -1:
                    jump_to_global_match(idx)
                else:
                    show_message("No match in previous 16 packs", rows, 1)
            else:
                current_pack = target_pack
                scroll_top = 0
        elif key == 'L':
            target_pack = min(total_packs - 1, current_pack + 16)
            if search_active:
                target_offset = target_pack * pack_size
                idx = search_accum.find_next_match_after_offset(target_offset)
                if idx != -1:
                    jump_to_global_match(idx)
                else:
                    show_message("No match in next 16 packs", rows, 1)
            else:
                current_pack = target_pack
                scroll_top = 0
        elif key == K_PGUP:
            step = max(1, max_data_rows // 2)
            scroll_top = max(0, scroll_top - step)
        elif key == K_PGDN:
            step = max(1, max_data_rows // 2)
            data_len = min(pack_size, file_size - current_pack * pack_size)
            total_rows = (data_len + 15) // 16
            scroll_top = min(total_rows - max_data_rows, scroll_top + step)
        elif key == 'O' or key == 'o':
            if search_active:
                current_offset = current_pack * pack_size + scroll_top * 16
                new_min = max(0, current_offset - FIND_CHUNK_SIZE)
                idx = search_accum.find_next_match_after_offset(new_min)
                if idx != -1 and (idx != current_global_match_idx or new_min <= current_offset):
                    if idx != current_global_match_idx:
                        jump_to_global_match(idx)
                    elif new_min <= search_accum.match_ranges[idx][0] < current_offset:
                        jump_to_global_match(idx)
                    else:
                        show_message("No previous match in 1MB block", rows, 1)
                else:
                    show_message("No previous match in 1MB block", rows, 1)
            else:
                current_pack = max(0, current_pack - 256)
                scroll_top = 0
        elif key == 'P' or key == 'p':
            if search_active:
                current_offset = current_pack * pack_size + scroll_top * 16
                new_min = current_offset + FIND_CHUNK_SIZE
                idx = search_accum.find_next_match_after_offset(new_min)
                if idx != -1:
                    jump_to_global_match(idx)
                else:
                    show_message("No match in next 1MB block", rows, 1)
            else:
                current_pack = min(total_packs - 1, current_pack + 256)
                scroll_top = 0
        elif key == K_HOME:
            if search_active:
                idx = search_accum.find_next_match_after_offset(0)
                if idx != -1:
                    jump_to_global_match(idx)
                else:
                    show_message("No match at beginning", rows, 1)
            else:
                current_pack = 0
                scroll_top = 0
        elif key == 'i':
            edit_mode = True
            _cursor_show()
            if cursor_byte == 0 and not dirty:
                cursor_byte = current_pack * pack_size + scroll_top * 16
            ensure_cursor_visible()
            continue
        elif key == 'm':
            mode = 'hex' if mode == 'ascii' else 'ascii'
        elif key == 'g':
            val = input_hex("Go to pack (hex):", rows)
            if val is not None:
                target = val - 1
                if 0 <= target < total_packs:
                    if search_active:
                        target_offset = target * pack_size
                        idx = search_accum.find_next_match_after_offset(target_offset)
                        if idx != -1:
                            jump_to_global_match(idx)
                        else:
                            show_message(f"No match in that pack", rows, 1)
                    else:
                        current_pack = target
                        scroll_top = 0
                else:
                    show_message(f"Invalid pack: {hex(val)} (max {hex(total_packs)})", rows, 1)
        elif key == 'f':
            typ, data, user_input = input_search(rows)
            if typ is None:
                continue
            acc = SearchAccumulator(mm, typ, data, pack_size, user_input)
            start_offset = current_pack * pack_size + scroll_top * 16
            acc.extend_scan(start_offset)
            if acc.match_ranges:
                search_active = True
                search_accum = acc
                current_global_match_idx = 0
                start, _ = acc.match_ranges[0]
                new_pack = start // pack_size
                offset_in_pack = start % pack_size
                new_row = offset_in_pack // 16
                data_len = min(pack_size, file_size - new_pack * pack_size)
                total_rows = (data_len + 15) // 16
                rows, _ = get_term_size()
                max_data_rows = max(0, rows - 4)
                scroll_top = max(0, min(total_rows - max_data_rows, new_row - max_data_rows // 2))
                current_pack = new_pack
                refresh_current_pack_display()
            else:
                show_message("No match found in first 1MB block", rows, 1)
        elif key == 'F':
            s = input_string("Search STR:", rows)
            if s:
                acc = SearchAccumulator(mm, 'hex', s.encode('utf-8'), pack_size, f'"{s}"')
                start_offset = current_pack * pack_size + scroll_top * 16
                acc.extend_scan(start_offset)
                if acc.match_ranges:
                    search_active = True
                    search_accum = acc
                    current_global_match_idx = 0
                    start, _ = acc.match_ranges[0]
                    new_pack = start // pack_size
                    offset_in_pack = start % pack_size
                    new_row = offset_in_pack // 16
                    data_len = min(pack_size, file_size - new_pack * pack_size)
                    total_rows = (data_len + 15) // 16
                    rows, _ = get_term_size()
                    max_data_rows = max(0, rows - 4)
                    scroll_top = max(0, min(total_rows - max_data_rows, new_row - max_data_rows // 2))
                    current_pack = new_pack
                    refresh_current_pack_display()
                else:
                    show_message("No match found in first 1MB block", rows, 1)

    mm.close()
    _cursor_show()
    _w('\033[2J\033[H')

if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        _cursor_show()
        _w('\033[2J\033[H')
        sys.exit(0)
    except Exception as e:
        _cursor_show()
        _w('\033[0m\033[2J\033[H')
        sys.stdout.flush()
        print(f"Error: {e}")
        import traceback
        traceback.print_exc()
        sys.exit(1)
