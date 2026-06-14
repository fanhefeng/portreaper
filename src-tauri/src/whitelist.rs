//! 白名单持久化。三层防护（评审发现，曾经全部静默吞错）：
//! - 原子写：同目录临时文件 + rename，崩溃中途不会留下半个 JSON；
//! - 损坏备份：解析失败的旧文件先挪到 .corrupt，绝不让后续首次保存覆盖旧数据；
//! - 失败回滚：持久化失败时回滚内存修改并上抛错误 —— 内存与磁盘永远一致，
//!   前端星标弹回 + 错误横幅，而不是「看起来成功、重启后消失」。
//!
//! 锁序：add/remove 持 WHITELIST 期间在 save_locked 内短暂取 WHITELIST_PATH；
//! init 改为顺序取锁（先 PATH 后 WHITELIST，互不嵌套），不构成 AB-BA 倒置。

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct WhitelistStore {
    pub entries: Vec<String>,
}

static WHITELIST: Lazy<Mutex<WhitelistStore>> = Lazy::new(|| Mutex::new(WhitelistStore::default()));
static WHITELIST_PATH: Lazy<Mutex<Option<PathBuf>>> = Lazy::new(|| Mutex::new(None));

pub fn init(path: PathBuf) {
    // 文件 IO 在持锁之外完成；两把锁顺序短暂持有、不嵌套
    let loaded = match fs::read_to_string(&path) {
        Ok(data) => match serde_json::from_str::<WhitelistStore>(&data) {
            Ok(store) => Some(store),
            Err(e) => {
                // 损坏的文件备份后让位 —— 用户的旧收藏可从 .corrupt 手工找回
                log::warn!("whitelist.json corrupted ({e}), backing up to .corrupt");
                let _ = fs::rename(&path, path.with_extension("json.corrupt"));
                None
            }
        },
        Err(_) => None, // 不存在/不可读：首次启动的常态
    };
    *WHITELIST_PATH.lock().unwrap() = Some(path);
    if let Some(store) = loaded {
        *WHITELIST.lock().unwrap() = store;
    }
}

pub fn get_all() -> Vec<String> {
    WHITELIST.lock().unwrap().entries.clone()
}

pub fn add(key: String) -> Result<(), String> {
    let mut wl = WHITELIST.lock().unwrap();
    if wl.entries.contains(&key) {
        return Ok(());
    }
    wl.entries.push(key);
    if let Err(e) = save_locked(&wl) {
        wl.entries.pop(); // 回滚：内存与磁盘保持一致
        return Err(e);
    }
    Ok(())
}

pub fn remove(key: &str) -> Result<(), String> {
    let mut wl = WHITELIST.lock().unwrap();
    let Some(idx) = wl.entries.iter().position(|x| x == key) else {
        return Ok(());
    };
    let removed = wl.entries.remove(idx);
    if let Err(e) = save_locked(&wl) {
        wl.entries.insert(idx, removed); // 回滚
        return Err(e);
    }
    Ok(())
}

/// 原子持久化：写同目录 .tmp 再 rename（同卷 rename 在 macOS/Windows 均为原子替换）。
/// 刻意不 fsync：断电窗口内最坏丢一次收藏修改（可重建数据），换每次星标零卡顿。
fn save_locked(store: &WhitelistStore) -> Result<(), String> {
    let path_guard = WHITELIST_PATH.lock().unwrap();
    let Some(path) = path_guard.as_ref() else {
        return Err("whitelist path not initialized".to_string());
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create config dir: {e}"))?;
    }
    let json = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, &json).map_err(|e| format!("write whitelist: {e}"))?;
    fs::rename(&tmp, path).map_err(|e| format!("commit whitelist: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 全生命周期串成单个 #[test]：WHITELIST/WHITELIST_PATH 是进程级全局量，
    /// 多个并行 #[test] 会互相踩状态 —— 顺序场景在一个测试体内推进。
    #[test]
    fn whitelist_lifecycle() {
        let dir = std::env::temp_dir().join(format!("portreaper-wl-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("whitelist.json");

        // 1. 损坏文件：init 备份为 .corrupt 而非静默丢弃，旧数据可找回
        fs::write(&path, "{ definitely not json").unwrap();
        init(path.clone());
        assert!(get_all().is_empty());
        assert!(
            dir.join("whitelist.json.corrupt").exists(),
            "损坏文件必须备份"
        );
        assert!(!path.exists(), "损坏文件应已挪走");

        // 2. add → 原子落盘（无残留 .tmp），重新解析与内存一致
        add("/usr/bin/x".to_string()).unwrap();
        assert!(path.exists());
        assert!(
            !dir.join("whitelist.json.tmp").exists(),
            "临时文件必须已被 rename 消费"
        );
        let disk: WhitelistStore =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(disk.entries, vec!["/usr/bin/x".to_string()]);

        // 3. 重复 add 幂等
        add("/usr/bin/x".to_string()).unwrap();
        assert_eq!(get_all().len(), 1);

        // 4. remove 落盘
        remove("/usr/bin/x").unwrap();
        let disk: WhitelistStore =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(disk.entries.is_empty());
        // 不存在的 key：no-op 且不报错
        remove("/never/there").unwrap();

        // 5. 持久化失败 → 上抛错误 + 内存回滚（目标路径是已存在目录 ⇒ rename 失败）
        let blocked = dir.join("blocked.json");
        fs::create_dir_all(&blocked).unwrap();
        init(blocked);
        let err = add("/usr/bin/y".to_string()).unwrap_err();
        assert!(!err.is_empty());
        assert!(
            !get_all().contains(&"/usr/bin/y".to_string()),
            "保存失败必须回滚内存"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
