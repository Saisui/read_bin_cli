# read_bin

**English** | [中文](README_ZH.md)

Terminal TUI hex viewer / editor written in Rust (v0.1.11).

**Dependencies**: ratatui (TUI framework), crossterm (terminal control), memmap2 (memory-mapped files), arboard (clipboard)

### Architecture Highlights

- **mmap + overlay**: Files are memory-mapped (zero-copy, no `to_vec`). Edits are stored in an in-memory `HashMap<usize, u8>` overlay that shadows the mmap — only modified bytes are copied, keeping memory proportional to edit count, not file size.
- **Four-level bitmap search** (invented by Saisui, BitSearch): 804-byte fixed memory for search indexing, on-demand scanning
- **Sparse Hierarchical Bitmap** (invented by Saisui): tracks edited bytes with 4K→1MB→1GB→1TB hierarchy, O(1) query, memory proportional to edit count not file size
- Edited bytes render in **italics** for visual distinction

## CLI Usage

```
read-bin [file] [options]
```

| Flag | Description |
|------|-------------|
| `--dump` | Plain text hex dump to stdout (no TUI) |
| `--copy` | Snapshot via temp file — external changes invisible |
| `--track` | Poll file for external changes every 50ms |
| `--inotify` | Inotify event-driven tracking (Linux/Android) |
| `--immediate`, `--imm` | Write-through: flush every edit to disk immediately |
| `--lock none` | No file lock (default) |
| `--lock 4k` | fcntl range lock on current 4K page |
| `--lock full` | flock(LOCK_SH) full file lock |
| `--lock-4k` | Same as `--lock 4k` |
| `--lock-full` | Same as `--lock full` |
| `--unlock` | Same as `--lock none` |
| `-h`, `--help` | Show help |

### Examples

```bash
read-bin data.bin                   # Open in TUI
read-bin data.bin --dump            # Plain hex dump
read-bin data.bin --copy --lock 4k  # Snapshot + 4K lock
read-bin log.bin --inotify          # Inotify tracking
read-bin data.bin --immediate       # Edit writes to disk instantly
```

## Top Bar

The top bar format is:

```
[1.2MB-icT] *filename.ext
```

- **`[filesize]`** — current file size in human-readable units
- **`-mods`** — active mode flags (omitted when all defaults)
- **`*`** — present when file has unsaved modifications
- **filename** — truncated with `...` preserving extension if too long

### Mode Flags

| Flag | Meaning |
|------|---------|
| `i` | Immediate (write-through) mode |
| `f` | Full file lock (`flock`) |
| `4` | 4K page lock (`fcntl`) |
| `t` | Track mode (poll every 50ms) |
| `T` | Inotify tracking (event-driven) |
| `c` | Copy mode (temp file snapshot) |

Click **`[filesize]`** in the top bar or press **`M`** to open the mode menu.

## Mode Menu

The mode menu lets you toggle runtime modes without restarting:

| Item | Toggleable | Description |
|------|------------|-------------|
| Track | ✅ | Poll file changes every 50ms |
| Inotify | ✅ | Inotify event-driven tracking (mutually exclusive with Track) |
| Immediate | ✅ | Write-through to disk |
| Copy | ❌ (set at launch only) | Snapshot via temp file |
| ───── | — | Separator |
| Lock: none/4k/full | ✅ | Cycle lock mode: none → 4k → full → none |

All modes except **Copy** can be toggled at runtime via the mode menu or keyboard shortcuts (`M` to open menu, then click/arrow+Enter to toggle).

## Display Modes

Click `[ASCII]` in the status bar to open the mode dropdown, or press `m` to cycle.

| Mode | Description |
|------|-------------|
| ASCII | Printable chars shown as-is, non-printable as hex |
| HEX | All bytes shown as 2-digit hex |
| UTF-8 | Decode and display UTF-8 characters |

### Color Modes

Toggle with `n` key (cycles through all modes) or select in the mode dropdown.

| Mode | Description |
|------|-------------|
| off | No color background |
| 256 | Each byte background = terminal palette color (Indexed) |
| RGB | Background from neighbor bytes: R=prev, G=self, B=next |
| HSL | Background from neighbor bytes: H=prev, L=self, S=next |
| Gray | Background = grayscale (RGB(v, v, v), v = byte value) |
| Heat | Heatmap: black→blue→red→yellow→white |
| hsl | Bit-decomposed HSL: high 4 bits = hue, mid 2 = lightness, low 2 = saturation |
| rgb | Bit-decomposed RGB: RR_GGGG_BB (2:4:2 bits) |

All color modes use auto-adaptive foreground (black or white based on background luminance).

## Cross-Page Scrolling

Scrolling seamlessly crosses page boundaries — no stopping at page edges.

| Key | Action |
|-----|--------|
| `↑` `↓` / `j` `k` | Scroll one line (cross-page) |
| `J` / `K` | Scroll one full screen |
| `PGUP` / `PGDN` | Scroll half screen |
| `HOME` | Go to first row |
| `O` / `P` | Jump ±1MB |

## Navigation

| Key | Action |
|-----|--------|
| `←` `→` / `h` `l` | Previous/Next pack |
| `H` / `L` | Jump ±16 packs |
| `g` / `Ctrl+G` | Go to offset (hex input) |

## Search

| Key | Description |
|-----|-------------|
| `f` | Hex / nibble pattern search |
| `F` | Plain UTF-8 string search |
| `ESC` | Clear search highlight |

### Search Input Formats

- **Exact hex**: `48656c6c6f`
- **Nibble wildcard**: `4x` (high nibble=4, low any)
- **Nibble range**: `[0-3]f` (high nibble 0-3, low=f)
- **Double range**: `[A-F][0-3]` (both nibbles in range)
- **Any byte**: `z` (= two any nibbles)

### Search Navigation

In search mode, all navigation keys keep their normal step size but jump to the first match on the target page.

| Key | Step | Action |
|-----|------|--------|
| `↑` / `↓` | Per match | Navigate to prev/next match |
| `←` / `→` | ±1 page | First match on target page |
| `J` / `K` | ±1 screen | First match on target page |
| `PGUP` / `PGDN` | ±½ screen | First match on target page |
| `H` / `L` | ±16 pages | First match on target page |
| `O` / `P` | ±256 pages | First match on target page |
| `HOME` | To start | First match in file |

If the target page has no match, scanning continues forward until one is found.

## Edit Mode

Press `i` to enter edit mode. Press `ESC` to exit.

| Key | Action |
|-----|--------|
| `←` `→` `↑` `↓` | Move cursor (cross-page) |
| `0`-`9` `a`-`f` | Edit nibble (HEX mode) |
| Any character | Edit byte (ASCII/UTF8 mode) |
| `Enter` | Insert newline (`\n`) in ASCII mode |
| `Tab` | Insert tab (`\t`) in ASCII mode |
| `Alt`+`↑` | Byte value +1 (at 0xFF: no change) |
| `Alt`+`↓` | Byte value -1 (at 0x00: no change) |
| `Ctrl+Z` | Undo |
| `Ctrl+Y` | Redo |
| `Ctrl+K, R` | Restore cursor byte to original value |

## Selection

### Keyboard

| Key | Action |
|-----|--------|
| `Alt+J` | Set selection start |
| `Alt+K` | Set selection end |

### Mouse

- **Click**: Move cursor to clicked byte
- **Click + Drag**: Select byte range
- **Scroll wheel**: Scroll up/down

## Clipboard

| Key | Action |
|-----|--------|
| `Ctrl+C` | Copy selection to clipboard |

Copy output matches display mode:
- **ASCII**: printable chars as-is, non-printable as `.`
- **HEX**: space-separated hex bytes (`48 65 6c 6c 6f`)
- **UTF-8**: decoded UTF-8 characters

## Status Bar

The bottom status bar has clickable regions:

| Region | Action |
|--------|--------|
| `[ASCII]` / `[HEX]` / `[UTF8]` | Open mode dropdown |
| `& 00000042` | Goto byte address |
| `pack 2/5` | Goto page number |
| `[MENU]` | Open menu — press `h` (Help), `s` (Sample), `a` (About) |

When searching, the status bar shows:
`Search: "4f2a" [3/5678+] @3/ff  ↑↓:next ESC:clear`

When 256-color is enabled, the mode label renders with a per-character gradient (blue → purple → pink).

## Save & Quit

| Key | Action |
|-----|--------|
| `Ctrl+S` | Save file |
| `q` | Quit (prompts to save if modified) |
| `Ctrl+Q` | Quit (prompts to save if modified) |

## Help & Menu

| Key | Action |
|-----|--------|
| `?` / `Ctrl+H` | Show help |
| Click `[MENU]` | Open menu dropdown |

Menu items (press underlined letter as shortcut):
- <ins>**H**</ins>elp: Show keybinding help
- <ins>**S**</ins>ample: Open a 256-byte sample file (0x00..0xFF) in memory
- <ins>**A**</ins>bout: Version, author, license info

## File Browser

When no file argument is given, a built-in file browser opens. It lists files in the current directory, sorted with `*sample` at the top — a built-in 256-byte sample (0x00..0xFF) that requires no file on disk.

## Color Configuration

Colors can be customized via `color.yaml` in the working directory. Supports named colors (`red`, `green`, `blue`, etc.) and RGB format `[r, g, b]`.

Special features:
- `fg: auto` - Automatically choose black or white foreground based on background brightness
- Terminal palette sync - Reads `~/.termux/colors.properties` for consistent theming

See `rust/color.yaml` for an example configuration.
