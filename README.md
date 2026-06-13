# read_bin

Terminal TUI hex viewer / editor written in Rust.

**Dependencies**: ratatui (TUI framework), crossterm (terminal control), memmap2 (memory-mapped files)

## Usage

```bash
# TUI mode
cargo run --release -- <file>

# Plain text hex dump mode
cargo run --release -- <file> --dump
```

## Display Modes

Press `m` to cycle through: ASCII → HEX → UTF-8

| Mode | Description |
|------|-------------|
| ASCII | Printable chars shown as-is, non-printable as hex |
| HEX | All bytes shown as 2-digit hex |
| UTF-8 | Decode and display UTF-8 characters |

## Navigation

| Key | Action |
|-----|--------|
| `↑` `↓` / `j` `k` | Scroll one line |
| `←` `→` / `h` `l` | Previous/Next pack |
| `J` / `K` | Scroll one full screen |
| `H` / `L` | Jump ±16 packs |
| `PGUP` / `PGDN` | Scroll half screen |
| `O` / `P` | Jump ±1MB |
| `HOME` | Go to first pack |
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
| `↑` / `↓` | Navigate matches within current pack |
| `←` / `→` | Jump to global prev/next match |
| `H` / `L` | Jump ±16 packs, find first match |
| `O` / `P` | Jump ±1MB, find next match |
| `HOME` | Jump to first match in file |
| `g` | Go to pack and first match there |

## Edit Mode

Press `i` to enter edit mode. Press `ESC` to exit.

| Key | Action |
|-----|--------|
| `←` `→` `↑` `↓` | Move cursor |
| `0`-`9` `a`-`f` | Edit nibble (HEX mode) |
| Any character | Edit byte (ASCII/UTF8 mode) |
| `Enter` | Insert newline (`\n`) in ASCII mode |
| `Tab` | Insert tab (`\t`) in ASCII mode |
| `Ctrl+Z` | Undo |
| `Ctrl+Y` | Redo |

## Selection

| Key | Action |
|-----|--------|
| `Alt+J` | Set selection start (bright highlight) |
| `Alt+K` | Set selection end |

## Mouse Support

- **Click**: Move cursor to clicked byte
- **Scroll wheel**: Scroll up/down

## Save & Quit

| Key | Action |
|-----|--------|
| `Ctrl+S` | Save file |
| `q` | Quit (prompts to save if modified) |
| `Ctrl+Q` | Quit (prompts to save if modified) |

## Other

| Key | Action |
|-----|--------|
| `?` / `Ctrl+H` | Show help |
| `Alt+M` | Toggle display mode |
| `m` | Cycle display mode (ASCII/HEX/UTF8) |

## Color Configuration

Colors can be customized via `color.yaml` in the working directory. Supports named colors (`red`, `green`, `blue`, etc.) and RGB format `[r, g, b]`.

Special features:
- `fg: auto` - Automatically choose black or white foreground based on background brightness
- Terminal palette sync - Reads `~/.termux/colors.properties` for consistent theming

See `rust/color.yaml` for an example configuration.
