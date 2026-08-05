//! 白名单（用户「收藏」的进程）的持久化 —— **值类型**，路径由调用方显式给出。
//!
//! # 为什么是值类型而不是进程级全局量
//!
//! 旧实现（`src-tauri/src/whitelist.rs`）是一对 `static` + `init(path)` 注入，
//! 贴合「常驻 GUI，启动一次、活到退出」的模型。但 CLI 与 Raycast 是**短命进程**：
//! 每次调用都要 init 一遍全局量，既别扭又让并行测试互相踩状态（旧测试不得不
//! 把整个生命周期串成单个 `#[test]`，正是全局量逼出来的）。
//!
//! 值类型让两种模型都自然：GUI 在自己那侧包一层 static 缓存（语义完全不变），
//! CLI 每次 load / mutate / drop。
//!
//! # 三层防护（全部原样保留，每一层都是事故换来的）
//!
//! - **原子写**：同目录临时文件 + rename，崩溃中途不会留下半个 JSON；
//! - **损坏备份**：解析失败的旧文件先挪到 `.corrupt`，绝不让后续首次保存覆盖旧数据；
//! - **失败回滚**：持久化失败时回滚内存修改并上抛错误 —— 内存与磁盘永远一致，
//!   前端得以「星标弹回 + 错误横幅」，而不是「看起来成功、重启后消失」。
//!
//! # 错误类型为什么仍是 String
//!
//! 与 `platform::KillError` 的枚举化刻意不同：kill 的失败有**语义分支**
//! （PID 被复用 / 进程已消失 / 无权限），前端要据此分叉 UI 与文案；白名单的
//! 失败只有一种处理方式 —— 原样展示给用户并回滚。为没有分支的错误引入枚举
//! 只是仪式感。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default, Clone)]
struct WhitelistStore {
    entries: Vec<String>,
}

/// 一份已加载的白名单，连同它的落盘位置。
#[derive(Clone)]
pub struct Whitelist {
    entries: Vec<String>,
    path: PathBuf,
}

impl Whitelist {
    /// 从磁盘载入。文件不存在 / 不可读是首次启动的常态，返回空表而非错误。
    ///
    /// 文件存在但解析失败时，把它备份为 `whitelist.json.corrupt` 再让位 ——
    /// 用户的旧收藏可以手工找回，而不是被下一次保存静默覆盖。
    pub fn load(path: PathBuf) -> Self {
        let entries = match fs::read_to_string(&path) {
            Ok(data) => match serde_json::from_str::<WhitelistStore>(&data) {
                Ok(store) => store.entries,
                Err(e) => {
                    log::warn!("whitelist.json corrupted ({e}), backing up to .corrupt");
                    let _ = fs::rename(&path, path.with_extension("json.corrupt"));
                    Vec::new()
                }
            },
            Err(_) => Vec::new(),
        };
        Self { entries, path }
    }

    /// 空白名单（无落盘位置时的降级形态：一切操作照常，只是存不下来）。
    pub fn empty(path: PathBuf) -> Self {
        Self {
            entries: Vec::new(),
            path,
        }
    }

    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    pub fn contains(&self, key: &str) -> bool {
        self.entries.iter().any(|e| e == key)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 加入白名单并落盘。重复 key 是幂等的 no-op。
    pub fn add(&mut self, key: String) -> Result<(), String> {
        if self.contains(&key) {
            return Ok(());
        }
        self.entries.push(key);
        if let Err(e) = self.save() {
            self.entries.pop(); // 回滚：内存与磁盘保持一致
            return Err(e);
        }
        Ok(())
    }

    /// 移出白名单并落盘。不存在的 key 是幂等的 no-op。
    pub fn remove(&mut self, key: &str) -> Result<(), String> {
        let Some(idx) = self.entries.iter().position(|x| x == key) else {
            return Ok(());
        };
        let removed = self.entries.remove(idx);
        if let Err(e) = self.save() {
            self.entries.insert(idx, removed); // 回滚
            return Err(e);
        }
        Ok(())
    }

    /// 原子持久化：写同目录 `.tmp` 再 rename（同卷 rename 在 macOS/Windows 均为
    /// 原子替换）。刻意不 fsync：断电窗口内最坏丢一次收藏修改（可重建数据），
    /// 换每次星标零卡顿。
    fn save(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create config dir: {e}"))?;
        }
        let store = WhitelistStore {
            entries: self.entries.clone(),
        };
        let json = serde_json::to_string_pretty(&store).map_err(|e| e.to_string())?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, &json).map_err(|e| format!("write whitelist: {e}"))?;
        fs::rename(&tmp, &self.path).map_err(|e| format!("commit whitelist: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个测试独占一个目录 —— 值类型不再有进程级全局量，故可以并行跑
    /// （旧实现被迫把整个生命周期串成单个 `#[test]`）。
    fn temp_dir_for(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("portreaper-wl-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn corrupted_file_is_backed_up_not_discarded() {
        let dir = temp_dir_for("corrupt");
        let path = dir.join("whitelist.json");
        fs::write(&path, "{ definitely not json").unwrap();

        let wl = Whitelist::load(path.clone());

        assert!(wl.entries().is_empty());
        assert!(
            dir.join("whitelist.json.corrupt").exists(),
            "损坏文件必须备份"
        );
        assert!(!path.exists(), "损坏文件应已挪走");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_is_atomic_and_idempotent() {
        let dir = temp_dir_for("add");
        let path = dir.join("whitelist.json");
        let mut wl = Whitelist::load(path.clone());

        wl.add("/usr/bin/x".to_string()).unwrap();
        assert!(path.exists());
        assert!(
            !dir.join("whitelist.json.tmp").exists(),
            "临时文件必须已被 rename 消费"
        );
        let disk: WhitelistStore =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(disk.entries, vec!["/usr/bin/x".to_string()]);

        wl.add("/usr/bin/x".to_string()).unwrap(); // 幂等
        assert_eq!(wl.entries().len(), 1);

        // 重新 load 必须看到同样的内容（落盘格式可往返）
        let reloaded = Whitelist::load(path.clone());
        assert_eq!(reloaded.entries(), wl.entries());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_persists_and_tolerates_missing_key() {
        let dir = temp_dir_for("remove");
        let path = dir.join("whitelist.json");
        let mut wl = Whitelist::load(path.clone());
        wl.add("/usr/bin/x".to_string()).unwrap();

        wl.remove("/usr/bin/x").unwrap();
        let disk: WhitelistStore =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(disk.entries.is_empty());

        wl.remove("/never/there").unwrap(); // no-op 且不报错
        let _ = fs::remove_dir_all(&dir);
    }

    /// 持久化失败必须上抛 + 回滚内存 —— 绝不「内存假成功、重启后丢收藏」。
    /// 构造失败：把目标路径做成一个已存在的**目录**，rename 必然失败。
    #[test]
    fn save_failure_rolls_back_memory() {
        let dir = temp_dir_for("rollback");
        let blocked = dir.join("whitelist.json");
        fs::create_dir_all(&blocked).unwrap();
        let mut wl = Whitelist::load(blocked);

        let err = wl.add("/usr/bin/y".to_string()).unwrap_err();
        assert!(!err.is_empty());
        assert!(!wl.contains("/usr/bin/y"), "保存失败必须回滚内存");
        let _ = fs::remove_dir_all(&dir);
    }
}
