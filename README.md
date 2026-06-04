# read_bin

Terminal-based binary file viewer / editor with TUI (curses).

## Usage

```bash
python read_bin.py <file>
```

## Display Modes

| Key | Mode     | Description                  |
|-----|----------|------------------------------|
| `m` | ASCII    | Printable chars shown, else hex |
| `m` | HEX      | All bytes shown as hex       |

## Navigation

| Key                | Action                        |
|--------------------|-------------------------------|
| `h` `j` `k` `l`   | Scroll left/down/up/right     |
| `←` `↓` `↑` `→`   | Same as hjkl                  |
| `H` / `L`          | Jump ±16 packs (64K)          |
| `J` / `K`          | Scroll one full screen        |
| `PGUP` / `PGDN`    | Scroll half screen            |
| `O` / `P`          | Jump ±1MB                     |
| `HOME`             | Go to first pack              |
| `g`                | Go to pack (hex input)        |

## Search

| Key | Description                                  |
|-----|----------------------------------------------|
| `f` | Search: hex bytes / regex / advanced pattern |
| `F` | Search plain UTF-8 string                    |
| `ESC` | Clear search highlight                     |

### Search Input Formats

- **Hex bytes**: `48656c6c6f`
- **Regex**: `/[\x20-\x7e]{4,}/`
- **Advanced hex**: `7x` (7 with any nibble), `zx` (any byte then any nibble), `zz` (any two bytes)

### Search Navigation

| Key          | Action                                |
|--------------|---------------------------------------|
| `↑` / `↓`    | Navigate matches within current pack  |
| `←` / `→`    | Jump to global prev/next match        |
| `H` / `L`    | Jump ±16 packs, find first match      |
| `O` / `P`    | Jump ±1MB, find next match            |
| `HOME`       | Jump to first match in file           |
| `g`          | Go to pack and first match there      |

## Edit Mode

Press `i` to enter edit mode. Press `ESC` to exit.

| Key              | Action                          |
|------------------|---------------------------------|
| `←` `→` `↑` `↓` | Move cursor                     |
| `0`-`9` `a`-`f` | Edit nibble (HEX mode)          |
| Any character    | Insert byte (ASCII mode)        |
| `Enter`          | Insert newline (`\n`)           |
| `Tab`            | Insert tab (`\t`)               |

## Quit

| Key | Action                          |
|-----|---------------------------------|
| `q` | Quit (prompts to save if modified) |

- Choose **Yes** to flush changes to disk.
- Choose **No** to undo all changes and quit.

## Other

| Key | Action     |
|-----|------------|
| `?` | Show help  |
