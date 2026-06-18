# TODO

## 跟踪模式 — 平台事件驱动

| 平台 | 机制 | 状态 | 依赖 |
|------|------|------|------|
| Linux/Android | inotify + libc::poll | ✅ 已实现 | `inotify` crate |
| macOS/iOS/BSD | kqueue | ⏳ 待实现 | `kqueue` 或 `mio` |
| Windows | ReadDirectoryChangesW | ⏳ 待实现 | `windows-sys` 或 `mio` |

### kqueue 实现思路（macOS/iOS/BSD）
- 用 `kqueue` crate 或 `mio` 统一封装
- `EVFILT_VNODE` + `NOTE_WRITE | NOTE_DELETE | NOTE_RENAME`
- `kevent()` 同时等待 stdin fd + kqueue fd

### Windows 实现思路
- `ReadDirectoryChangesW` 监听文件变化
- `WaitForMultipleObjects` 同时等待 stdin + 文件变化事件
- 或用 `mio` / `notify` crate 统一封装

### 统一方案
- `mio` crate 跨平台封装 inotify/kqueue/IOCP
- 一套代码覆盖所有平台
- 代价：多一个依赖，API 抽象层

## 废弃代码清理

- `search.rs`：旧 `Search` struct 及相关函数（已被 `bitmap.rs` BitSearch 替代）
- `InputMode::FileBrowser`：从未构造的枚举变体
- `App::data_len()` / `App::total_rows()`：从未使用的方法
- `DirEntry::size`：从未读取的字段
- `BitSearch::pack_matches()`：从未使用的方法
- `ColorConfig::sp_blank` / `sp_unknown`：从未读取的字段
