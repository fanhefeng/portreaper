//! Portreaper 的判定引擎 —— 采集、分类、终止，零 GUI 依赖。
//!
//! 这个 crate 是「孤儿 / 僵尸进程判定」的**唯一真相源**：桌面 GUI（`src-tauri`）、
//! 命令行（`portreaper-cli`）以及任何第三方前端都只消费它的结果，绝不各自复刻
//! 一份判定。判定逻辑本就与 Tauri 无关（`scanner` 与 `platform` 对 tauri 的引用
//! 数一直是 0），本 crate 只是把这份既有的解耦**暴露出去**。
//!
//! 依赖纪律：这里不得出现任何 GUI / IPC / 异步运行时依赖。并发策略由调用方决定
//! —— GUI 侧用 `spawn_blocking` 把 `scan()` 挪出主线程，CLI 直接同步调用。
//!
//! 模块地图见 `docs/ARCHITECTURE-CORE-SPLIT.md`。

pub mod platform;
pub mod scanner;

pub use platform::kill;
pub use scanner::{scan, ProcessEntry};
