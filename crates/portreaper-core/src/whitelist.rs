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
//! # 六层防护（全部原样保留，每一层都是事故换来的）
//!
//! - **原子写**：同目录临时文件 + rename，崩溃中途不会留下半个 JSON；
//! - **损坏备份**：解析失败的旧文件先挪到 `.corrupt`，绝不让后续首次保存覆盖旧数据；
//! - **失败回滚**：持久化失败时回滚内存修改并上抛错误 —— 内存与磁盘永远一致，
//!   前端得以「星标弹回 + 错误横幅」，而不是「看起来成功、重启后消失」。
//! - **读不出就不写**（`writable` + `DiskRead` 三态）：只有 `NotFound` 才意味着
//!   「首次启动，空表」。权限/IO 错误下文件**是存在的**，把它当空表再保存等于用空表
//!   覆盖用户全部收藏。备份 `.corrupt` 的 rename 若失败同理 —— 旧数据还躺在原地，
//!   让位没成功。这类实例转为只读：读照常，add/remove 响亮失败（评审发现）。
//!
//!   这一层**必须覆盖全部三条读盘路径**（`load` / `merge_from_disk` / `refresh`），
//!   而不只是 `load`：常驻 GUI 一辈子只 load 一次，之后每次 add/remove 都走
//!   `merge_from_disk`。它一度把三种结果压成 `Option::None`「保留内存视图后继续
//!   覆写」，于是「GUI 启动时文件还不存在（内存空表）→ 用户在 Raycast 加了一批星
//!   → 文件变得读不出 → 在桌面版点一次 ★」会用空表覆写掉全部收藏 —— `save` 走
//!   rename，只要目录可写就能替换掉一个读不出的文件（评审发现）。
//! - **写前合并**（`merge_from_disk`）：core 拆分后 GUI / CLI / Raycast 是**三个
//!   进程**共写同一份文件，而 add/remove 的落盘是「整表覆写」。常驻 GUI 的内存
//!   快照会越来越旧，直接覆写就把 CLI 刚加的收藏抹掉。故每次修改前先取一遍磁盘
//!   最新内容作基准，再施加本次增删（评审发现）。
//! - **读时刷新**（`refresh`）：上一条的对偶，且**两条都不可省**。写前合并保证
//!   常驻 GUI 不覆盖别人的改动，但它只在 GUI 自己 add/remove 时才触发；两次
//!   mutate 之间，GUI 读到的仍是旧快照 —— 外部加的星它看不见，那一行照旧标红、
//!   照旧计入托盘、**照旧留在一键清扫的目标集里**，用户刚收藏的进程会被清扫杀掉。
//!   v0.8.1 带着这个缺口发过版，v0.9.0 上架 Raycast 前的跨端 ★ 验收才在真机上撞到。
//!   教训：「共享状态」这一个说法盖住了两个方向，写方向有测试就没人再验读方向。
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

/// 一次读盘的三种结果。**`Absent` 与 `Unusable` 必须分开**：前者意味着磁盘上
/// 没有任何用户数据（首次启动 / 文件被删），拿内存视图重建它是安全的；后者意味着
/// 文件**就在那儿**、只是这一刻读不出或解析不了，覆写它就是抹掉用户的收藏。
/// 把两者压成一个 `Option::None` 正是 v0.11.0 之前那条数据丢失路径的成因。
enum DiskRead {
    Entries(Vec<String>),
    Absent,
    Unusable,
}

/// 一份已加载的白名单，连同它的落盘位置。
#[derive(Clone)]
pub struct Whitelist {
    entries: Vec<String>,
    path: PathBuf,
    /// 磁盘上那份文件是否可以安全覆写。见模块文档「读不出就不写」。
    writable: bool,
}

/// 损坏文件的备份落点。**不能是固定的 `.json.corrupt`**（评审发现）：Unix 的
/// `rename` 是替换语义，第二次损坏会直接盖掉第一次那份备份，而「用户的旧收藏可以
/// 手工找回」正是这层防护承诺的全部内容 —— 第二次事故时它就不成立了。
///
/// 后缀用「自纪元起的秒数」而非可读时间：不引入时间格式化依赖，且天然单调递增；
/// 取不到系统时间时退回 PID。秒级粒度挡不住同一秒内的第二次事故，故再加一个
/// 「占用就换名」的序号 —— 这个函数的**唯一职责**就是绝不返回一个已存在的路径。
fn corrupt_backup_path(path: &Path) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| std::process::id().to_string());
    let base = path.with_extension(format!("json.{stamp}.corrupt"));
    if !base.exists() {
        return base;
    }
    for n in 1..1000 {
        let candidate = path.with_extension(format!("json.{stamp}-{n}.corrupt"));
        if !candidate.exists() {
            return candidate;
        }
    }
    // 同一秒内一千次损坏：已经不是数据保护问题了，退回基名（rename 会替换它）
    base
}

impl Whitelist {
    /// 从磁盘载入。文件不存在（`NotFound`）是首次启动的常态，返回空表而非错误。
    ///
    /// 文件存在但解析失败时，把它备份到一个带时间戳的 `.corrupt` 文件再让位
    /// （命名规则见 `corrupt_backup_path` —— 刻意不是固定名，否则第二次损坏会
    /// 盖掉第一份备份）：用户的旧收藏可以手工找回，而不是被下一次保存静默覆盖。
    ///
    /// 文件存在却读不出来（权限/IO），或备份 rename 没成功 —— 两种情况下旧数据
    /// 都还在磁盘上，此时返回的实例是**只读**的：绝不拿一张空表去覆盖它。
    pub fn load(path: PathBuf) -> Self {
        let (entries, writable) = match fs::read_to_string(&path) {
            Ok(data) => match serde_json::from_str::<WhitelistStore>(&data) {
                Ok(store) => (store.entries, true),
                Err(e) => {
                    let backup = corrupt_backup_path(&path);
                    log::warn!(
                        "whitelist.json corrupted ({e}), backing up to {}",
                        backup.display()
                    );
                    match fs::rename(&path, &backup) {
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

    /// 无落盘位置时的降级形态：读照常为空，写走到 save 时响亮失败 ——
    /// 不是「静默存不下来」，add/remove 会回滚并上抛错误。
    ///
    /// **刻意无参**（评审发现）：曾是 `empty(path)`，签名允许传真实路径，那样
    /// `merge_from_disk` 会吸入磁盘内容、行为退化成 `load` —— 一个名叫 empty 的
    /// 构造器却能非空。两个调用点本来传的也都是 `PathBuf::new()`，把「脱离磁盘」
    /// 编码进签名后，误用在类型层面即不可表达。
    pub fn detached() -> Self {
        Self {
            entries: Vec::new(),
            path: PathBuf::new(),
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

    /// 只读地取一次磁盘现状。**三态**，绝不把「文件不在」与「文件在但读不出」
    /// 压成同一个值 —— 那正是「读不出就不写」这层防护的判据所在：前者可以安全地
    /// 用内存视图重建，后者的磁盘上**还躺着用户的数据**。
    ///
    /// 这里仍然绝不做 `.corrupt` 备份、也不自己改 `writable`：备份是 `load`
    /// 的职责（有副作用的「让位」动作只该有一处），`writable` 由调用方按各自的
    /// 读/写语义处置。
    fn read_disk_entries(&self) -> DiskRead {
        match fs::read_to_string(&self.path) {
            Ok(data) => match serde_json::from_str::<WhitelistStore>(&data) {
                Ok(store) => DiskRead::Entries(store.entries),
                // 解析不了：文件里仍是用户的（可能可人工恢复的）数据，不可覆写
                Err(e) => {
                    log::warn!("whitelist.json unparsable during merge/refresh ({e})");
                    DiskRead::Unusable
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => DiskRead::Absent,
            Err(e) => {
                log::error!("whitelist.json unreadable during merge/refresh ({e})");
                DiskRead::Unusable
            }
        }
    }

    /// 用磁盘现状作为本次修改的基准，纳入其他进程期间的改动。
    ///
    /// **失败会上抛**（评审发现）：模块文档第四层防护「读不出就不写」此前只在
    /// `load` 里兑现，而常驻 GUI 只 load 一次 —— 之后每一次 add/remove 都走这里，
    /// 把三种读盘结果压成「保留内存视图，继续整表覆写」。真实事故形态：GUI 启动
    /// 时文件尚不存在（内存视图为空表），用户随后在 Raycast/CLI 加了一批星，此后
    /// 文件变得读不出（权限变更 / Windows 上被别的进程独占 / IO 错误）—— 下一次
    /// 在桌面版点 ★ 就会用「空表 + 这一个」整表覆写掉全部收藏。`save` 走的是
    /// rename，只需目录写权限，文件本身读不出**不妨碍**覆写它。
    fn merge_from_disk(&mut self) -> Result<(), String> {
        match self.read_disk_entries() {
            DiskRead::Entries(disk) => {
                self.entries = disk;
                Ok(())
            }
            // 文件不存在：内存视图就是唯一真相，save 会重建它
            DiskRead::Absent => Ok(()),
            DiskRead::Unusable => Err(
                "whitelist file exists but could not be read; refusing to overwrite it".to_string(),
            ),
        }
    }

    /// 重新对齐磁盘现状 —— **读路径专用**。
    ///
    /// 写前合并（`add`/`remove` 里的 `merge_from_disk`）只保证「常驻 GUI 不会
    /// 覆盖掉 CLI 刚加的星」，那是**写**方向。读方向需要这一个：常驻 GUI 的内存
    /// 快照在两次 mutate 之间会越来越旧，不刷新的话 Raycast/CLI 加的星在桌面版
    /// 永远不可见 —— 那一行仍标红、仍计入托盘计数、**仍留在一键清扫的目标集里**。
    /// 用户刚在 Raycast 收藏的进程被桌面版一键清扫杀掉，是本项目最不能出的误杀。
    ///
    /// 语义是**替换**而非并集：取消星标同样要传播。取并集会让内存里的旧键永远
    /// 留着，un-star 在桌面版永不生效。
    ///
    /// 读路径不返回错误（每 2 秒一次的扫描没有可展示的失败出口），但它**会**顺带
    /// 维护 `writable`：读得出就解除只读、读不出就转入只读。两个方向都必要 ——
    /// 只转不解的话，启动那一刻恰好读不出的常驻实例会整个会话拒绝收藏（托盘应用
    /// 可以连开数天，用户只能重启）；只解不转的话，运行期变得不可读的文件会在
    /// 下一次 add 时被整表覆写。
    pub fn refresh(&mut self) {
        match self.read_disk_entries() {
            DiskRead::Entries(disk) => {
                self.entries = disk;
                self.writable = true;
            }
            // 文件不存在：内存视图即真相，save 会重建它 —— 创建新文件是安全的
            DiskRead::Absent => self.writable = true,
            // 读不出：保留内存视图，并转入只读，绝不让后续 save 覆写磁盘上那份
            DiskRead::Unusable => self.writable = false,
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
        self.merge_from_disk()?;
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
        self.merge_from_disk()?;
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

    /// 目录里所有 `.corrupt` 结尾的备份。备份名带时间戳，不能再按固定名断言。
    fn corrupt_backups(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".corrupt"))
            .collect();
        v.sort();
        v
    }

    #[test]
    fn corrupted_file_is_backed_up_not_discarded() {
        let dir = temp_dir_for("corrupt");
        let path = dir.join("whitelist.json");
        fs::write(&path, "{ definitely not json").unwrap();

        let wl = Whitelist::load(path.clone());

        assert!(wl.entries().is_empty());
        assert_eq!(corrupt_backups(&dir).len(), 1, "损坏文件必须备份");
        assert!(!path.exists(), "损坏文件应已挪走");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 第二次损坏不得盖掉第一次那份备份 —— 固定名 `.json.corrupt` 配上 Unix
    /// rename 的替换语义，正好把「旧收藏可以手工找回」这句承诺在第二次事故时作废
    /// （评审发现）。
    #[test]
    fn second_corruption_keeps_the_first_backup() {
        let dir = temp_dir_for("corrupt-twice");
        let path = dir.join("whitelist.json");

        fs::write(&path, r#"{"entries":["/first/round"]}x"#).unwrap();
        Whitelist::load(path.clone());
        let after_first = corrupt_backups(&dir);
        assert_eq!(after_first.len(), 1);

        // 第二次损坏（测试跑在同一秒内 —— 靠「占用就换名」的序号区分）
        fs::write(&path, r#"{"entries":["/second/round"]}y"#).unwrap();
        Whitelist::load(path.clone());
        let after_second = corrupt_backups(&dir);

        assert_eq!(
            after_second.len(),
            2,
            "第二次备份把第一次覆盖掉了：{after_second:?}"
        );
        let first_content = fs::read_to_string(dir.join(&after_first[0])).unwrap();
        assert!(
            first_content.contains("/first/round"),
            "第一份备份的内容被改写了：{first_content}"
        );
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

    /// 上一条钉的是「**load 那一刻**读不出」。这条钉「**常驻实例活着的时候**才变得
    /// 读不出」—— 同一层防护的另一半，而它一度完全缺失（评审发现）。
    ///
    /// 缺失的原因很具体：GUI 一辈子只 `load` 一次，此后每次 add/remove 都走
    /// `merge_from_disk`，而它曾把「文件不存在」与「文件读不出」压成同一个
    /// `Option::None`，两者都当作「保留内存视图，继续整表覆写」。
    ///
    /// 事故形态：GUI 启动时文件还不存在（内存是空表）→ 用户在 Raycast/CLI 加了
    /// 一批星 → 文件此后读不出 → 用户在桌面版点一次 ★ → 磁盘被「空表 + 这一个」
    /// 覆写，此前的收藏全没。注意 `save` 走的是 rename：文件本身读不出**不妨碍**
    /// 覆写它，只要目录可写。
    #[test]
    fn disk_becoming_unreadable_after_load_blocks_writes() {
        let dir = temp_dir_for("unreadable-later");
        let path = dir.join("whitelist.json");

        // GUI 启动：文件尚不存在 ⇒ 一个可写的空表实例
        let mut resident = Whitelist::load(path.clone());
        assert!(resident.entries().is_empty());

        // 另一个前端（Raycast / CLI）随后加了一批星
        let mut ephemeral = Whitelist::load(path.clone());
        ephemeral.add("/raycast/one".to_string()).unwrap();
        ephemeral.add("/raycast/two".to_string()).unwrap();

        // 文件在常驻实例运行期间损坏：**内容还在**，只是解析不了 ——
        // 正是「读不出就不写」要保护的形态（用户的收藏仍可人工找回）
        let intact = fs::read_to_string(&path).unwrap();
        fs::write(&path, format!("{intact}<<<truncated garbage")).unwrap();

        let err = resident.add("/gui/three".to_string()).unwrap_err();
        assert!(!err.is_empty(), "读不出磁盘时必须响亮失败");
        assert!(!resident.contains("/gui/three"), "拒写后不得留下内存假象");

        let after = fs::read_to_string(&path).unwrap();
        assert!(
            after.starts_with(&intact) && after.contains("/raycast/one"),
            "读不出的文件绝不可被整表覆写 —— 用户的收藏还在里面"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// `refresh` 必须**双向**维护 `writable`，两个方向都是事故点：
    /// - 只解不转：运行期变得不可读的文件会在下一次 add 时被覆写；
    /// - 只转不解：启动那一刻恰好读不出的常驻 GUI 会**整个会话**拒绝收藏，
    ///   而托盘应用可以连开数天，用户只能靠重启应用自愈 —— 偏偏 refresh 每
    ///   2 秒就拿着磁盘现状路过一次，没有理由不复位。
    #[test]
    fn refresh_maintains_writability_in_both_directions() {
        let dir = temp_dir_for("writable-recovery");
        let path = dir.join("whitelist.json");

        // 启动那一刻读不出（目录占位）⇒ 只读实例
        fs::create_dir_all(&path).unwrap();
        let mut wl = Whitelist::load(path.clone());
        assert!(wl.add("/a".to_string()).is_err(), "只读实例必须拒写");

        // 磁盘恢复正常，且外部此间已写入内容
        fs::remove_dir_all(&path).unwrap();
        fs::write(&path, r#"{"entries":["/external/one"]}"#).unwrap();
        wl.refresh();

        assert!(wl.contains("/external/one"), "refresh 必须取到磁盘现状");
        wl.add("/a".to_string())
            .expect("磁盘恢复可读后必须重新可写");
        let disk = Whitelist::load(path.clone());
        assert!(disk.contains("/external/one"), "恢复写入不得抹掉外部内容");
        assert!(disk.contains("/a"));

        // 反方向：再次变得读不出 ⇒ refresh 要转回只读，把拒绝提前到 add 入口
        fs::remove_file(&path).unwrap();
        fs::create_dir_all(&path).unwrap();
        wl.refresh();
        assert!(
            wl.add("/b".to_string()).is_err(),
            "磁盘重新变得读不出后必须转回只读"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// 损坏文件的备份 rename 失败时同样转只读 —— 让位没成功，旧数据还在原地。
    ///
    /// 构造：把备份目标预先占成一个**非空目录**，rename 过不去。备份名现在带时间戳
    /// **构造手法变了**（备份改带时间戳的连带后果）：`corrupt_backup_path` 现在
    /// 「占用就换名」，再也无法靠预置一个同名目录把 rename 堵死 —— 那本身正是这次
    /// 改动想要的行为。改成把**目录本身**设为只读，让 rename 在权限上失败。
    ///
    /// 这个构造是 unix-only 的（Windows 的只读目录语义不同），故 cfg 到 macOS ——
    /// 不变量本身与平台无关，只是造不出同一个现场。root 下 chmod 形同虚设，
    /// 那时**跳过**而不是假通过。
    #[cfg(target_os = "macos")]
    #[test]
    fn failed_corrupt_backup_also_locks_writes() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            eprintln!("skipped: 以 root 运行时 chmod 不生效，造不出 rename 失败");
            return;
        }
        let dir = temp_dir_for("backup-fail");
        let path = dir.join("whitelist.json");
        fs::write(&path, "{ definitely not json").unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();

        let mut wl = Whitelist::load(path.clone());

        // 先把权限放回来：下面的断言与清理都要读写这个目录
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();

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

    /// 上一条钉的是**写**方向（常驻实例不覆盖别人）。这条钉**读**方向：常驻实例
    /// 在两次自身 mutate 之间也必须看得见别人的改动。
    ///
    /// 真机复现过的事故形态（v0.9.0 上架 Raycast 前的跨端 ★ 同步验收）：在 CLI /
    /// Raycast 里加星，桌面版托盘计数纹丝不动 —— 那一行仍标红、仍计入托盘，
    /// **仍留在一键清扫的目标集里**，用户刚收藏的进程会被清扫杀掉。
    #[test]
    fn resident_instance_sees_external_changes_without_mutating() {
        let dir = temp_dir_for("refresh");
        let path = dir.join("whitelist.json");

        let mut resident = Whitelist::load(path.clone());
        resident.add("/gui/kept".to_string()).unwrap();

        // 另一个进程（CLI/Raycast）加星 + 取消星标，常驻实例全程没有任何 mutate
        let mut ephemeral = Whitelist::load(path.clone());
        ephemeral.add("/cli/starred".to_string()).unwrap();
        ephemeral.remove("/gui/kept".to_string().as_str()).unwrap();

        assert!(
            !resident.contains("/cli/starred"),
            "前提：不 refresh 时内存快照确实是旧的（否则本测试没在测东西）"
        );

        resident.refresh();

        assert!(
            resident.contains("/cli/starred"),
            "外部加的星必须可见 —— 否则桌面版会把用户刚收藏的进程算进清扫目标"
        );
        assert!(
            !resident.contains("/gui/kept"),
            "外部取消的星必须同步消失 —— refresh 是替换语义，取并集会让 un-star 永不生效"
        );

        // refresh 是只读的：不得反过来把内存写回磁盘
        let disk = Whitelist::load(path);
        assert!(disk.contains("/cli/starred"));
        assert!(
            !disk.contains("/gui/kept"),
            "refresh 不得复活已被删除的条目"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
