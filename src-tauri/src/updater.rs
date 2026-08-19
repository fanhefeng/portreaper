//! 应用内更新（检查 / 下载安装 / 重启），基于 tauri-plugin-updater。
//!
//! 更新源是 GitHub Release 上的 `latest.json`（publish job 生成，见
//! `.github/workflows/release.yml` 与 `scripts/generate-latest-json.mjs`），
//! 端点与公钥在 `tauri.conf.json` 的 `plugins.updater` 里。
//!
//! 结构上与其它命令同一模式：插件**纯 Rust 侧**注册，前端只经本 crate 自己的
//! `#[tauri::command]` 触达 —— 不装 `@tauri-apps/plugin-updater` npm 包，
//! capabilities 白名单（security-config.test.ts 的精确断言）零改动；下载进度用
//! IPC `Channel` 作为命令参数回传，同样不需要任何 event 权限。

use std::sync::{Mutex, PoisonError};

use tauri::{ipc::Channel, AppHandle, State};
use tauri_plugin_updater::UpdaterExt;

/// 检查更新的网络超时。手动检查有 UI 等着它，挂死 30s 已是极限；
/// 前端另有一层更宽的 withTimeout 兜底（invoke 永不 settle 的那类故障）。
///
/// **只作用于检查请求**：`UpdaterBuilder::timeout` 设的值不会传给 `check()` 产出的
/// `Update`（插件在那里把 `timeout` 硬编码成 `None`），下载超时另见 `INSTALL_TIMEOUT`。
const CHECK_TIMEOUT_SECS: u64 = 30;

/// 下载 + 安装的后端超时。
///
/// **必须短于前端的 `INSTALL_TIMEOUT_MS`（10 分钟，src/updater.ts）**，因为只有
/// 后端这一侧能把 `Update` 放回 `PendingUpdate`。若前端先超时：它把状态落到
/// `installFailed`，而挂死的后端任务仍 `take` 着那份 update —— 用户点「重试」拿到
/// `no pending update; run check_update first`，可 `useUpdater.check` 在
/// `installFailed` 态又直接早退，那句提示指向一个 UI 不允许的动作，只能重启应用
/// 才能再更新（评审发现）。给两侧留 2 分钟余量，后端总是先醒。
const INSTALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8 * 60);

/// `check_update` 拿到的待装更新，供 `install_update` 消费。
///
/// 存引擎对象而非重新 check：下载安装必须用**检查时看到的那一份**
/// （URL + 签名成对），中间再查一次可能拿到另一个版本。
pub struct PendingUpdate(pub Mutex<Option<tauri_plugin_updater::Update>>);

/// 前端展示所需的最小字段集（serde 蛇形，与 src/updater.ts 的 TS 镜像一一对应）。
#[derive(Clone, serde::Serialize)]
pub struct UpdateInfo {
    pub version: String,
    pub current_version: String,
    pub notes: Option<String>,
}

/// 下载/安装进度（经命令参数里的 IPC Channel 回传）。
/// `total` 是 Content-Length，服务器不给时为 None —— 前端按字节数降级展示。
#[derive(Clone, serde::Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum InstallProgress {
    Started {
        total: Option<u64>,
    },
    Chunk {
        downloaded: u64,
        total: Option<u64>,
    },
    /// 下载完毕、开始落盘安装（安装本身没有细粒度进度）。
    Installing,
}

/// 检查更新。有更新时返回信息并把引擎对象存入 `PendingUpdate`；无更新返回 None。
///
/// 错误以 String 上抛：更新错误多是网络/服务端形态（DNS、超时、404 ——
/// 第一个带 latest.json 的 release 发布之前 404 是**常态**），没有可分派的
/// 语义分支，前端套一层本地化的「检查更新失败」标签原样展示。
#[tauri::command]
pub async fn check_update(
    app: AppHandle,
    state: State<'_, PendingUpdate>,
) -> Result<Option<UpdateInfo>, String> {
    let builder = app
        .updater_builder()
        .timeout(std::time::Duration::from_secs(CHECK_TIMEOUT_SECS));

    // dev 专用端点覆盖：不发一个 release 也能真机联调整条检查链路
    // （`PORTREAPER_UPDATER_URL=http://localhost:…/latest.json pnpm tauri dev`；
    // debug 下插件放行 http，release 只认 https）。用 shadowing 而非 mut ——
    // release 构建裁掉本块后 `mut` 会变成 unused_mut 警告（release 构建实测）。
    #[cfg(debug_assertions)]
    let builder = if let Ok(url) = std::env::var("PORTREAPER_UPDATER_URL") {
        let parsed = url
            .parse()
            .map_err(|e| format!("PORTREAPER_UPDATER_URL invalid: {e}"))?;
        builder
            .endpoints(vec![parsed])
            .map_err(|e| format!("PORTREAPER_UPDATER_URL rejected: {e}"))?
    } else {
        builder
    };

    let updater = builder.build().map_err(|e| e.to_string())?;
    match updater.check().await {
        Ok(Some(update)) => {
            log::info!(
                "update check: {} -> {} available",
                update.current_version,
                update.version
            );
            let info = UpdateInfo {
                version: update.version.clone(),
                current_version: update.current_version.clone(),
                notes: update.body.clone(),
            };
            // 毒化恢复：与 scan_ports / whitelist 的锁同一套取舍 —— 内容物只是
            // 一个可整体替换的 Option，持锁 panic 不可能留下半更新状态。
            *state.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(update);
            Ok(Some(info))
        }
        Ok(None) => {
            log::info!("update check: already up to date");
            *state.0.lock().unwrap_or_else(PoisonError::into_inner) = None;
            Ok(None)
        }
        Err(e) => {
            log::warn!("update check failed: {e}");
            Err(e.to_string())
        }
    }
}

/// 下载并安装 `check_update` 找到的那份更新。成功返回后由前端决定何时重启
/// （macOS：`.app` 已被替换，重启即生效；Windows：NSIS 以 passive 模式运行，
/// 安装器会自行结束本进程，本命令可能根本来不及返回 —— 两种收尾都正常）。
#[tauri::command]
pub async fn install_update(
    state: State<'_, PendingUpdate>,
    on_progress: Channel<InstallProgress>,
) -> Result<(), String> {
    // dev 二进制不在 bundle 里，updater 没有可替换的安装位置；更别说把
    // `target/debug` 覆盖成正式版。检查链路 dev 可用，安装链路明确拒绝。
    if cfg!(debug_assertions) {
        return Err("updater install is disabled in dev builds".to_string());
    }

    // take 出来再 await（std Mutex 不能跨 await 持有）。并发二次调用拿到 None，
    // 直接报错 —— 前端在安装期间禁用按钮，这里只是兜底。
    let update = state
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .ok_or_else(|| "no pending update; run check_update first".to_string())?;

    log::info!("update install: downloading {}", update.version);
    let _ = on_progress.send(InstallProgress::Started { total: None });

    // 进度节流：chunk 回调每几 KB 一次，逐条过 IPC 会刷爆 webview ——
    // 每累计 ≥256 KiB 才发一条（最后的 Installing 保证终态一定送达）。
    let mut downloaded: u64 = 0;
    let mut last_sent: u64 = 0;
    // 超时包在最外层：插件自己的 Update 没有超时（见 INSTALL_TIMEOUT 的注释），
    // 一条连接不断、却不再有数据的 TCP 黑洞会让这个 await 永不返回。
    // `download_and_install` 取 &self，故超时后 `update` 仍在本栈上，放得回去。
    let result = tokio::time::timeout(
        INSTALL_TIMEOUT,
        update.download_and_install(
            |chunk_len, content_len| {
                downloaded += chunk_len as u64;
                if downloaded - last_sent >= 256 * 1024 {
                    last_sent = downloaded;
                    let _ = on_progress.send(InstallProgress::Chunk {
                        downloaded,
                        total: content_len,
                    });
                }
            },
            || {
                let _ = on_progress.send(InstallProgress::Installing);
            },
        ),
    )
    .await;

    // 三种收尾，两种都要把那份 update 放回去 —— 唯一不放回的是装成功了。
    match result {
        Ok(Ok(())) => {
            log::info!("update install: {} installed", update.version);
            Ok(())
        }
        Ok(Err(e)) => {
            log::warn!("update install failed: {e}");
            // 失败的那份放回去：网络闪断后用户点重试，不必重新 check
            *state.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(update);
            Err(e.to_string())
        }
        Err(_elapsed) => {
            log::warn!(
                "update install timed out after {}s",
                INSTALL_TIMEOUT.as_secs()
            );
            // 同样放回：超时最可能是网络卡住，重试完全合理，不该逼用户重启应用
            *state.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(update);
            Err("update download timed out".to_string())
        }
    }
}

/// 安装完成后的重启（macOS 路径；Windows 由 NSIS 安装器接管进程生命周期）。
#[tauri::command]
pub fn restart_app(app: AppHandle) {
    log::info!("restarting to finish update");
    app.restart();
}

/// 在浏览器打开本版本的 GitHub Release 页（更新弹窗的「查看发布说明」）。
///
/// 走 Rust 侧 opener 而非前端 `openUrl`：capabilities 把 `opener:allow-open-url`
/// 收窄到 `http://localhost:*`（security-config.test.ts 钉死），放宽它就是给
/// 注入后的 webview 一条任意跳转通道。URL 在这里由常量拼出，webview 只能传
/// 版本号，且先做字符白名单 —— 结构上到不了任意 URL。
#[tauri::command]
pub fn open_release_page(app: AppHandle, version: String) -> Result<(), String> {
    if version.is_empty()
        || !version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    {
        return Err(format!("invalid version: {version:?}"));
    }
    let url = format!(
        "{}/releases/tag/v{version}",
        env!("CARGO_PKG_REPOSITORY").trim_end_matches('/')
    );
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}
