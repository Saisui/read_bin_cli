# Changelog

## v0.1.4 (2026-06-15)

### Cross-page seamless scrolling
- Scroll past page boundaries without stopping — pages connect seamlessly
- Global row numbering for seamless navigation across all packs
- Row numbers show within-page offset (0x00-0xff), resetting at each page boundary
- Up/Down/j/k, PageUp/PageDown, Home all work cross-page
- Mouse click and drag work across page boundaries
- O/P (±1MB jump) uses global rows

---

## v0.1.3 (2026-06-15)

### UI overhaul — Status bar
- Removed top 2 rows (filename+size, pack+mode), replaced with clickable status bar
- Status bar layout: `[ASCII]  @00000042  pack 1/a  Ctrl+H:help`
- Pack info displayed in hexadecimal format

### Mode dropdown menu
- Click `[ASCII]` to open dropdown: ASCII / HEX / UTF8 / [256] checkbox
- Keyboard: 1/2/3 to select mode, 4/n to toggle 256-color
- Current mode highlighted in dropdown

### 256-color gradient label
- When 256-color enabled, `[ASCII]` renders with per-character gradient background
- 7 characters × 7 colors: blue → indigo → purple → magenta → pink → light pink
- White foreground on gradient background

### Clickable status bar regions
- Click `@address` → goto byte address input
- Click `pack` → goto pack number input
- Click `Ctrl+H:help` → open help window
- Jumping sets cursor position and focus

### Mouse drag selection
- Click and drag to select byte range
- Selection highlighted with sp(15)/sp(17) styles
- Click outside clears selection
- Status bar protected from selection bleed (Clear widget)

### Ctrl+C clipboard copy
- Added `arboard` dependency for cross-platform clipboard
- Mode-aware copy output:
  - ASCII: printable chars as-is, non-printable as `.`
  - HEX: space-separated hex bytes (`48 65 6c 6c 6f`)
  - UTF8: decoded UTF-8 characters (multi-byte sequences properly decoded)

### Windows double-click fix
- Root cause: crossterm 0.28 on Windows generates both Press and Release events for each keypress; code never checked `key.kind`
- Fix: filter `KeyEventKind::Release` events (all platforms)
- Additional 40ms throttle for Windows auto-repeat (`#[cfg(target_os = "windows")]`)
- Removed old fragile 20ms debounce that only covered scroll keys
- No impact on Termux/Linux/macOS (Repeat events not generated on those platforms)

### Internal
- New `InputMode::ModeSelect` and `InputMode::GotoByteInput` variants
- New `DisplayMode::prev()` method
- New `App.dragging` state field
- `max_rows` changed from `saturating_sub(4)` to `saturating_sub(2)`

---

## v0.1.2 (2026-06-14)

### 256-color display mode
- New 256-color background display mode via `n` key toggle
- Each byte rendered with its terminal palette color as background
- Auto foreground: luminance-based black/white text for readability
- Applied across ASCII, HEX, and UTF8 display modes

### CI/CD
- Added cross-compilation for Linux and Windows
- Added aarch64 (ARM64) build for Termux/Raspberry Pi
- Binary trimmed to ~564K with `opt-level = "s"` + LTO + strip

---

## v0.1.1 (2026-06-14)

### Search engine overhaul
- Replaced regex-based search with custom nibble pattern matching
- New search syntax: exact hex (`4f2a`), nibble wildcards (`4x`), ranges (`[0-3]f`), any byte (`z`)
- Three-level bitmap index (L0=pack, L1=1MB, L2=1GB) for fast skip
- Background incremental search via channel
- Search input: `f` for hex/nibble, `F` for plain string

### Color configuration
- YAML-based color config (`color.yaml`)
- `fg: auto` — auto-select black/white foreground based on background luminance
- Terminal palette sync from `~/.termux/colors.properties`
- Named colors + RGB `[r, g, b]` format
- Dim alternating effect for same-type consecutive bytes

### Display improvements
- Gradient column header (blue → green)
- UTF-8 mode with cross-row multi-byte sequence handling
- Display width calculation for CJK characters (width=2)
- Byte classification: null, control, ASCII, hex, head2/3/4, tail

### Help popup
- Interactive help with `?` / `Ctrl+H`
- Scrollable help content
- Version and author display

### Navigation
- Full keyboard navigation: j/k/h/l, J/K/H/L, PgUp/PgDn, Home
- O/P for ±1MB area jump
- g/Ctrl+G for goto offset (hex input)
- Mouse wheel scroll support

### Edit mode
- `i` to enter edit mode, ESC to exit
- Hex nibble editing in HEX mode
- Character editing in ASCII/UTF8 mode
- Ctrl+Z undo, Ctrl+Y redo
- Tab inserts `\t`, Enter inserts `\n`

### Selection
- `Alt+J` to set selection start
- `Alt+K` to set selection end

### File operations
- `Ctrl+S` to save
- `q` / `Ctrl+Q` to quit with save prompt if modified
- Memory-mapped file I/O via `memmap2`

---

## v0.1.0 (2026-06-13)

### Initial release — Rust TUI hex viewer
- Terminal-based hex viewer/editor written in Rust
- Three display modes: ASCII, HEX, UTF-8
- Pack-based file browsing (4096 bytes per pack)
- Basic navigation: arrow keys, j/k/h/l
- Byte editing with cursor
- Save to file
- Color configuration support
- Dependencies: ratatui (TUI), crossterm (terminal), memmap2 (mmap)

### Original Python version
- `read_bin.py` — Python/curses hex viewer (original)
- `read_bin_new.py` — Python/curses hex viewer (enhanced)
- `read_bin_win.py` — Windows-specific Python version
- `read_bin_termux.py` — Termux-specific Python version

### Original Ruby version
- `read_bin.rb` — Ruby hex viewer (original)
