use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct WhitelistStore {
    pub entries: Vec<String>,
}

static WHITELIST: Lazy<Mutex<WhitelistStore>> =
    Lazy::new(|| Mutex::new(WhitelistStore::default()));
static WHITELIST_PATH: Lazy<Mutex<Option<PathBuf>>> = Lazy::new(|| Mutex::new(None));

pub fn init(path: PathBuf) {
    *WHITELIST_PATH.lock().unwrap() = Some(path.clone());
    if let Ok(data) = fs::read_to_string(&path) {
        if let Ok(store) = serde_json::from_str::<WhitelistStore>(&data) {
            *WHITELIST.lock().unwrap() = store;
        }
    }
}

pub fn get_all() -> Vec<String> {
    WHITELIST.lock().unwrap().entries.clone()
}

pub fn add(key: String) {
    let mut wl = WHITELIST.lock().unwrap();
    if !wl.entries.contains(&key) {
        wl.entries.push(key);
        save_locked(&wl);
    }
}

pub fn remove(key: &str) {
    let mut wl = WHITELIST.lock().unwrap();
    let before = wl.entries.len();
    wl.entries.retain(|x| x != key);
    if wl.entries.len() != before {
        save_locked(&wl);
    }
}

fn save_locked(store: &WhitelistStore) {
    if let Some(path) = WHITELIST_PATH.lock().unwrap().as_ref() {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(store) {
            let _ = fs::write(path, json);
        }
    }
}
