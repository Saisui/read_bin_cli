# 🎮 实时对战：共享文件游戏

> 用 `read-bin` 的 `--inotify --immediate` 模式，通过共享文件与另一个程序进行实时对战。

## 概念

两个程序通过一个共享二进制文件通信：

```
┌─────────────┐     game.bin      ┌──────────────────┐
│  游戏引擎    │ ──写入状态──▶    │  read-bin 玩家端  │
│  (Python)   │ ◀──读取操作──    │  (TUI 十六进制)   │
└─────────────┘                   └──────────────────┘
```

- **游戏引擎**（Python 脚本）：维护游戏逻辑，将棋盘状态写入 `game.bin`
- **玩家端**（read-bin）：用 `--inotify --immediate` 打开同一文件，实时看到棋盘变化，直接编辑字节落子

`--inotify` 让 read-bin 毫秒级感知外部修改；`--immediate` 让玩家的编辑立即落盘——两者结合，实现实时交互。

---

## 文件格式：井字棋

16 字节二进制文件 `game.bin`：

| 偏移 | 字段 | 值含义 |
|------|------|--------|
| `0x00` | 游戏状态 | `0`=进行中, `1`=X 胜, `2`=O 胜, `3`=平局 |
| `0x01` | 当前轮到 | `1`=X, `2`=O |
| `0x02` ~ `0x0A` | 棋盘 3×3 | `0`=空, `1`=X, `2`=O |
| `0x0B` ~ `0x0F` | 保留 | `0x00` |

棋盘偏移对应位置：

```
 0x02 | 0x03 | 0x04
------+-------+------
 0x05 | 0x06 | 0x07
------+-------+------
 0x08 | 0x09 | 0x0A
```

read-bin 显示效果（HEX 模式）：

```
00000000  00 01 00 00 00 00 00 00  00 00 00 00 00 00 00 00  │................│
          ^^ ^^
          状态 轮到X
```

---

## 游戏引擎脚本

保存为 `game_engine.py`：

```python
#!/usr/bin/env python3
"""井字棋游戏引擎 —— 与 read-bin 实时对战"""

import struct
import sys
import time
import os
import subprocess

GAME_FILE = "game.bin"
LINES = [(0,1,2),(3,4,5),(6,7,8),(0,3,6),(1,4,7),(2,5,8),(0,4,8),(2,4,6)]

def create_game():
    """初始化游戏文件：X 先手"""
    with open(GAME_FILE, "wb") as f:
        f.write(b'\x00\x01' + b'\x00' * 14)

def read_state():
    with open(GAME_FILE, "rb") as f:
        data = f.read(16)
    return list(data)

def check_winner(board):
    for a, b, c in LINES:
        if board[a] != 0 and board[a] == board[b] == board[c]:
            return board[a]
    if all(board[i] != 0 for i in range(9)):
        return 3  # 平局
    return 0

def render_board(data):
    """在终端绘制棋盘"""
    symbols = {0: '.', 1: 'X', 2: 'O'}
    board = data[2:11]
    status = data[0]
    turn = data[1]
    print("\n 井字棋 ── read-bin 实时对战")
    print(" ─────────────────────────")
    for row in range(3):
        cells = [symbols[board[row*3+col]] for col in range(3)]
        print(f"  {cells[0]} │ {cells[1]} │ {cells[2]}")
        if row < 2:
            print("  ──┼───┼──")
    print()
    if status == 1:
        print(" 🎉 X 获胜！")
    elif status == 2:
        print(" 🎉 O 获胜！")
    elif status == 3:
        print(" 🤝 平局！")
    else:
        who = "X" if turn == 1 else "O"
        print(f" 轮到: {who}")
        if who == "O":
            print(" (引擎思考中...)")
    print()

def engine_move(data):
    """引擎（O）简单策略：赢 > 堵 > 中心 > 角 > 边"""
    board = data[2:11]
    empty = [i for i in range(9) if board[i] == 0]

    # 能赢就赢
    for i in empty:
        board[i] = 2
        if check_winner(board) == 2:
            return i
        board[i] = 0

    # 堵对手
    for i in empty:
        board[i] = 1
        if check_winner(board) == 1:
            board[i] = 0
            return i
        board[i] = 0

    # 中心
    if 4 in empty:
        return 4

    # 角
    for i in [0, 2, 6, 8]:
        if i in empty:
            return i

    # 边
    for i in [1, 3, 5, 7]:
        if i in empty:
            return i

    return -1

def main():
    create_game()
    print("=" * 40)
    print("  井字棋引擎已启动！")
    print(f"  请在另一个终端运行：")
    print(f"  read-bin {GAME_FILE} --inotify --immediate")
    print("=" * 40)

    if len(sys.argv) > 1 and sys.argv[1] == "--launch-read-bin":
        subprocess.Popen(["read-bin", GAME_FILE, "--inotify", "--immediate"])

    prev_data = None
    while True:
        data = read_state()

        # 状态变化时刷新显示
        if data != prev_data:
            render_board(data)
            prev_data = data

        status = data[0]
        if status != 0:
            print(" 游戏结束！删除 game.bin 重新开始。")
            break

        turn = data[1]
        if turn == 1:
            # 轮到玩家 X —— 监控偏移 0x02~0x0A 的变化
            time.sleep(0.05)
            continue

        # 轮到引擎 O
        board = data[2:11]
        pos = engine_move(data)
        if pos < 0:
            break

        # 写入引擎的落子
        data[2 + pos] = 2
        data[1] = 1  # 切换回 X
        data[0] = check_winner(data[2:11])

        with open(GAME_FILE, "r+b") as f:
            f.seek(0)
            f.write(bytes(data))

        time.sleep(0.05)

if __name__ == "__main__":
    main()
```

---

## 游戏步骤

### 1. 启动游戏引擎

```bash
python3 game_engine.py
```

终端输出：

```
========================================
  井字棋引擎已启动！
  请在另一个终端运行：
  read-bin game.bin --inotify --immediate
========================================
```

### 2. 启动 read-bin 玩家端

在**另一个终端**运行：

```bash
read-bin game.bin --inotify --immediate
```

此时 read-bin 顶栏显示 `[16B-iT]`（`i`=立即模式，`T`=inotify 跟踪）。

### 3. 玩家 X 落子

在 read-bin 中操作：

1. 按 **`i`** 进入编辑模式
2. 用方向键移动光标到目标棋盘位置（偏移 `0x02` ~ `0x0A`）
3. 输入 **`1`**（代表 X）
4. 将偏移 `0x01` 改为 **`2`**（切换到 O 回合）
5. 按 **`ESC`** 退出编辑模式

`--immediate` 确保你的编辑立刻写入磁盘。

### 4. 引擎自动响应

游戏引擎检测到文件变化后：
- 显示更新后的棋盘
- 计算 O 的落子
- 写入文件

read-bin 的 `--inotify` 自动刷新显示——你立刻看到引擎的落子。

### 5. 持续对弈

重复步骤 3-4，直到出现胜负或平局。

### 6. 开始新局

```bash
rm game.bin && python3 game_engine.py
```

---

## 技术要点

### 为什么需要两个标志？

| 标志 | 方向 | 作用 |
|------|------|------|
| `--immediate` | 玩家 → 文件 | 你的编辑立刻落盘，引擎能立即读到 |
| `--inotify` | 文件 → 玩家 | 引擎的写入立刻反映到 read-bin 显示 |

缺少任何一个，游戏都无法实时进行。

### 与 `--track` 的区别

`--track` 每 50ms 轮询，`--inotify` 是事件驱动。对于这个小游戏两者都够用，但 `--inotify` 更省 CPU、响应更快。

### 文件锁注意事项

如果需要防止读写竞争，可以加锁：

```bash
read-bin game.bin --inotify --immediate --lock full
```

引擎端也用 `fcntl.flock()` 加锁。对于井字棋这种低频操作，不加锁也没问题。

---

## 扩展思路

- **多人聊天**：偏移 0x00 存消息长度，后续字节存消息内容，多终端用 `--inotify --immediate` 实时聊天
- **贪吃蛇**：16×16 网格存 256 字节，引擎推进游戏帧，玩家编辑方向字节
- **协作画板**：大文件每个字节代表一个像素颜色，多人同时用 read-bin 编辑
- **生命游戏**：引擎按康威规则迭代，玩家编辑初始状态或在运行中注入新细胞

---

*read-bin：不只是查看器，更是实时二进制交互终端。*
