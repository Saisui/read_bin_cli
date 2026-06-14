# read_bin

Terminal TUI hex viewer / editor written in Rust.

**Dependencies**: ratatui (TUI framework), crossterm (terminal control), memmap2 (memory-mapped files), arboard (clipboard)

## Usage

```bash
# TUI mode
cargo run --release -- <file>

# Plain text hex dump mode
cargo run --release -- <file> --dump
```

## Display Modes

Click `[ASCII]` in the status bar to open the mode dropdown, or press `m` to cycle.

| Mode | Description |
|------|-------------|
| ASCII | Printable chars shown as-is, non-printable as hex |
| HEX | All bytes shown as 2-digit hex |
| UTF-8 | Decode and display UTF-8 characters |

### 256-Color Mode

Toggle with `n` key or the `[256]` checkbox in the mode dropdown. Each byte is rendered with its terminal palette color as background, with auto black/white foreground for readability.

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

| Key | Action |
|-----|--------|
| `↑` / `↓` | Navigate matches within current view |
| `←` / `→` | Jump to global prev/next match |
| `H` / `L` | Jump ±16 packs, find first match |
| `O` / `P` | Jump ±1MB, find next match |
| `HOME` | Jump to first match in file |
| `g` | Go to pack and first match there |

## Edit Mode

Press `i` to enter edit mode. Press `ESC` to exit.

| Key | Action |
|-----|--------|
| `←` `→` `↑` `↓` | Move cursor (cross-page) |
| `0`-`9` `a`-`f` | Edit nibble (HEX mode) |
| Any character | Edit byte (ASCII/UTF8 mode) |
| `Enter` | Insert newline (`\n`) in ASCII mode |
| `Tab` | Insert tab (`\t`) in ASCII mode |
| `Ctrl+Z` | Undo |
| `Ctrl+Y` | Redo |

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
| `@00000042` | Goto byte address |
| `pack 2/5` | Goto page number |
| `Ctrl+H:help` | Open help |

When 256-color is enabled, the mode label renders with a per-character gradient (blue → purple → pink).

## Save & Quit

| Key | Action |
|-----|--------|
| `Ctrl+S` | Save file |
| `q` | Quit (prompts to save if modified) |
| `Ctrl+Q` | Quit (prompts to save if modified) |

## Help

| Key | Action |
|-----|--------|
| `?` / `Ctrl+H` | Show help |

## Color Configuration

Colors can be customized via `color.yaml` in the working directory. Supports named colors (`red`, `green`, `blue`, etc.) and RGB format `[r, g, b]`.

Special features:
- `fg: auto` - Automatically choose black or white foreground based on background brightness
- Terminal palette sync - Reads `~/.termux/colors.properties` for consistent theming

See `rust/color.yaml` for an example configuration.
