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
//! # 五层防护（全部原样保留，每一层都是事故换来的）
//!
//! - **原子写**：同目录临时文件 + rename，崩溃中途不会留下半个 JSON；
//! - **损坏备份**：解析失败的旧文件先挪到 `.corrupt`，绝不让后续首次保存覆盖旧数据；
//! - **失败回滚**：持久化失败时回滚内存修改并上抛错误 —— 内存与磁盘永远一致，
//!   前端得以「星标弹回 + 错误横幅」，而不是「看起来成功、重启后消失」。
//! - **读不出就不写**（`writable`）：只有 `NotFound` 才意味着「首次启动，空表」。
//!   权限/IO 错误下文件**是存在的**，把它当空表再保存等于用空表覆盖用户全部收藏。
//!   备份 `.corrupt` 的 rename 若失败同理 —— 旧数据还躺在原地，让位没成功。
//!   这类实例转为只读：读照常，add/remove 响亮失败（评审发现）。
//! - **写前合并**（`merge_from_disk`）：core 拆分后 GUI / CLI / Raycast 是**三个
//!   进程**共写同一份文件，而 add/remove 的落盘是「整表覆写」。常驻 GUI 的内存
//!   快照会越来越旧，直接覆写就把 CLI 刚加的收藏抹掉。故每次修改前先取一遍磁盘
//!   最新内容作基准，再施加本次增删（评审发现）。
//!
//! # 为什么不上跨进程文件锁
//!
//! 合并后仅剩「两个进程几乎同刻各自 read→write」这一窄窗口，代价是最坏丢一次
//! 星标操作（用户重按即可，不是数据损坏）。为它引入一个文件锁 crate，与本项目
//! 「依赖精简」的取舍不相称 —— 白名单是低频人工操作，不是高并发写入点。
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
    /// 磁盘上那份文件是否可以安全覆写。见模块文档「读不出就不写」。
    writable: bool,
}

impl Whitelist {
    /// 从磁盘载入。文件不存在（`NotFound`）是首次启动的常态，返回空表而非错误。
    ///
    /// 文件存在但解析失败时，把它备份为 `whitelist.json.corrupt` 再让位 ——
    /// 用户的旧收藏可以手工找回，而不是被下一次保存静默覆盖。
    ///
    /// 文件存在却读不出来（权限/IO），或备份 rename 没成功 —— 两种情况下旧数据
    /// 都还在磁盘上，此时返回的实例是**只读**的：绝不拿一张空表去覆盖它。
    pub fn load(path: PathBuf) -> Self {
        let (entries, writable) = match fs::read_to_string(&path) {
            Ok(data) => match serde_json::from_str::<WhitelistStore>(&data) {
                Ok(store) => (store.entries, true),
                Err(e) => {
                    log::warn!("whitelist.json corrupted ({e}), backing up to .corrupt");
                    match fs::rename(&path, path.with_extension("json.corrupt")) {
                        // 让位成功：原文件已挪走，可以安全地从空表重建
                        Ok(()) => (Vec::new(), true),
                        Err(e) => {
                            log::error!(
                                "whitelist.json backup failed ({e}); refusing to overwrite it"
                            );
                            (Vec::new(), false)
                        }
                    }
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Vec::new(), true),
            Err(e) => {
                log::error!("whitelist.json unreadable ({e}); refusing to overwrite it");
                (Vec::new(), false)
            }
        };
        Self {
            entries,
            path,
            writable,
        }
    }

    /// 空白名单（无落盘位置时的降级形态：读照常为空，写走到 save 时响亮失败 ——
    /// 不是「静默存不下来」，add/remove 会回滚并上抛错误）。
    pub fn empty(path: PathBuf) -> Self {
        Self {
            entries: Vec::new(),
            path,
            writable: true,
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

    /// 只读地取一次磁盘现状。读不出/解析不了都返回 `None` —— 这里绝不做备份、
    /// 也绝不改 `writable`：那是 `load` 的职责，写路径上重复一遍只会把「让位」
    /// 这种有副作用的动作散落到多处。
    fn read_disk_entries(&self) -> Option<Vec<String>> {
        let data = fs::read_to_string(&self.path).ok()?;
        serde_json::from_str::<WhitelistStore>(&data)
            .ok()
            .map(|s| s.entries)
    }

    /// 用磁盘现状作为本次修改的基准，纳入其他进程期间的改动。读不出就保留内存
    /// 视图（文件被删/被占的降级形态，后续 save 会重建它）。
    fn merge_from_disk(&mut self) {
        if let Some(disk) = self.read_disk_entries() {
            self.entries = disk;
        }
    }

    fn ensure_writable(&self) -> Result<(), String> {
        if self.writable {
            return Ok(());
        }
        Err("whitelist file exists but could not be read; refusing to overwrite it".to_string())
    }

    /// 加入白名单并落盘。重复 key 是幂等的 no-op。
    ///
    /// 回滚快照必须取在 `merge_from_disk` **之后**：save 失败时磁盘仍是合并前
    /// 拿到的那份现状，回滚到「合并后、修改前」恰好与之相等 —— 快照取在合并前
    /// 会把其他进程的改动从内存里丢掉，直到下次 mutate 才自愈（评审发现）。
    pub fn add(&mut self, key: String) -> Result<(), String> {
        self.ensure_writable()?;
        self.merge_from_disk();
        let snapshot = self.entries.clone();
        if self.contains(&key) {
            return Ok(());
        }
        self.entries.push(key);
        if let Err(e) = self.save() {
            self.entries = snapshot; // 回滚：内存与磁盘保持一致
            return Err(e);
        }
        Ok(())
    }

    /// 移出白名单并落盘。不存在的 key 是幂等的 no-op。快照时序同 `add`。
    pub fn remove(&mut self, key: &str) -> Result<(), String> {
        self.ensure_writable()?;
        self.merge_from_disk();
        let snapshot = self.entries.clone();
        let Some(idx) = self.entries.iter().position(|x| x == key) else {
            return Ok(());
        };
        self.entries.remove(idx);
        if let Err(e) = self.save() {
            self.entries = snapshot; // 回滚
            return Err(e);
        }
        Ok(())
    }

    /// 原子持久化：写同目录 `.tmp` 再 rename（同卷 rename 在 macOS/Windows 均为
    /// 原子替换）。刻意不 fsync：断电窗口内最坏丢一次收藏修改（可重建数据），
    /// 换每次星标零卡顿。
    ///
    /// 临时文件名带 PID：三个前端进程共写同一目录，固定的 `.tmp` 会互相踩 ——
    /// A 的 write 与 B 的 rename 交错，B 就把 A 的半截内容 rename 成了正式文件。
    fn save(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create config dir: {e}"))?;
        }
        let store = WhitelistStore {
            entries: self.entries.clone(),
        };
        let json = serde_json::to_string_pretty(&store).map_err(|e| e.to_string())?;
        let tmp = self
            .path
            .with_extension(format!("json.{}.tmp", std::process::id()));
        // 两条失败路径都清 tmp：PID 后缀意味着短命 CLI 每次的临时名都不同，
        // 磁盘满写半截时的残留会在配置目录逐个累积、永远没人回收。
        if let Err(e) = fs::write(&tmp, &json) {
            let _ = fs::remove_file(&tmp);
            return Err(format!("write whitelist: {e}"));
        }
        if let Err(e) = fs::rename(&tmp, &self.path) {
            let _ = fs::remove_file(&tmp);
            return Err(format!("commit whitelist: {e}"));
        }
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

    /// tmp 文件名带 PID，不能再按固定名断言。
    fn has_tmp_leftover(dir: &Path) -> bool {
        fs::read_dir(dir)
            .unwrap()
            .any(|e| e.unwrap().file_name().to_string_lossy().ends_with(".tmp"))
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
            !has_tmp_leftover(&dir),
            "临时文件必须已被 rename 消费（文件名带 PID，故按后缀查）"
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
    ///
    /// 构造失败：先在**路径不存在**时 load（正常的可写实例），再把目标路径做成
    /// 一个目录，此后 rename 必然失败。顺序很重要 —— 先建目录再 load 走的是
    /// 「读不出 ⇒ 只读」那条路，测不到本用例要测的回滚。
    #[test]
    fn save_failure_rolls_back_memory() {
        let dir = temp_dir_for("rollback");
        let path = dir.join("whitelist.json");
        let mut wl = Whitelist::load(path.clone());
        fs::create_dir_all(&path).unwrap();

        let err = wl.add("/usr/bin/y".to_string()).unwrap_err();
        assert!(!err.is_empty());
        assert!(!wl.contains("/usr/bin/y"), "保存失败必须回滚内存");
        assert!(!has_tmp_leftover(&dir), "提交失败后不得留下临时文件");
        let _ = fs::remove_dir_all(&dir);
    }

    /// save 失败时的回滚基准必须是「合并后」的视图：磁盘此刻就是合并前读到的
    /// 现状，回滚到合并前的旧快照会把其他进程的改动从内存里丢掉、直到下次
    /// mutate 才自愈（评审发现）。用「tmp 路径被目录占位」制造 write 阶段失败 ——
    /// 读路径不受影响，merge 照常成功。
    #[test]
    fn rollback_after_merge_keeps_other_processes_changes() {
        let dir = temp_dir_for("rollback-merge");
        let path = dir.join("whitelist.json");

        let mut resident = Whitelist::load(path.clone());
        resident.add("/gui/first".to_string()).unwrap();

        let mut ephemeral = Whitelist::load(path.clone());
        ephemeral.add("/cli/second".to_string()).unwrap();

        // 占住本进程的 tmp 名，让 resident 的下一次 save 在 write 阶段失败
        let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();

        let err = resident.add("/gui/third".to_string()).unwrap_err();
        assert!(!err.is_empty());
        assert!(!resident.contains("/gui/third"), "保存失败必须回滚本次修改");
        assert!(
            resident.contains("/cli/second"),
            "回滚不得把已合并进来的他进程改动一并丢掉"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// 文件存在却读不出来时，绝不能当成「首次启动的空表」再保存回去 ——
    /// 那等于用空表覆盖用户的全部收藏。读用目录制造 `NotADirectory`/`IsADirectory`
    /// 类错误：跨平台都不是 `NotFound`，且不依赖 chmod（root 下会失效）。
    #[test]
    fn unreadable_file_is_never_overwritten() {
        let dir = temp_dir_for("unreadable");
        let path = dir.join("whitelist.json");
        fs::create_dir_all(&path).unwrap();

        let mut wl = Whitelist::load(path.clone());

        assert!(wl.entries().is_empty());
        assert!(
            wl.add("/usr/bin/z".to_string()).is_err(),
            "只读实例必须拒写"
        );
        assert!(wl.remove("/usr/bin/z").is_err(), "只读实例必须拒写");
        assert!(path.is_dir(), "原有数据必须原封不动");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 损坏文件的备份 rename 失败时同样转只读 —— 让位没成功，旧数据还在原地。
    /// 构造：`.corrupt` 目标预先占成一个非空目录，rename 过不去。
    #[test]
    fn failed_corrupt_backup_also_locks_writes() {
        let dir = temp_dir_for("backup-fail");
        let path = dir.join("whitelist.json");
        fs::write(&path, "{ definitely not json").unwrap();
        let blocker = dir.join("whitelist.json.corrupt");
        fs::create_dir_all(blocker.join("occupied")).unwrap();

        let mut wl = Whitelist::load(path.clone());

        assert!(
            wl.add("/usr/bin/z".to_string()).is_err(),
            "备份失败必须拒写"
        );
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "{ definitely not json",
            "备份没成功时原文件必须原封不动"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// 三个前端进程共写一份文件：常驻实例的内存快照会变旧，直接整表覆写会抹掉
    /// 另一个进程刚加的收藏。写前合并必须让两侧的修改都留存。
    #[test]
    fn concurrent_instances_do_not_clobber_each_other() {
        let dir = temp_dir_for("merge");
        let path = dir.join("whitelist.json");

        // 长命实例（GUI），加载一次后一直活着
        let mut resident = Whitelist::load(path.clone());
        resident.add("/gui/first".to_string()).unwrap();

        // 短命实例（CLI）：自己 load、自己改、退出
        let mut ephemeral = Whitelist::load(path.clone());
        ephemeral.add("/cli/second".to_string()).unwrap();

        // 常驻实例此刻的内存里还没有 /cli/second —— 它的下一次写不得把它抹掉
        resident.add("/gui/third".to_string()).unwrap();

        let disk = Whitelist::load(path.clone());
        for key in ["/gui/first", "/cli/second", "/gui/third"] {
            assert!(
                disk.contains(key),
                "{key} 丢失：整表覆写抹掉了其他进程的修改"
            );
        }
        assert!(
            resident.contains("/cli/second"),
            "合并后内存视图必须与磁盘一致"
        );

        // 删除同理：常驻实例删自己的项，不得连带复活/抹掉对方的项
        resident.remove("/gui/first").unwrap();
        let disk = Whitelist::load(path);
        assert!(!disk.contains("/gui/first"));
        assert!(disk.contains("/cli/second"), "删除不得波及其他进程的条目");
        let _ = fs::remove_dir_all(&dir);
    }
}
