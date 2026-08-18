// 应用内更新的前端一路：类型契约镜像 + useUpdater hook。
// 后端是 src-tauri/src/updater.rs（tauri-plugin-updater 的薄壳命令）；
// 下载进度经 IPC Channel（命令参数）回传 —— 不走 event 系统，无需任何权限。
import { useCallback, useEffect, useRef, useState } from "react";
import { Channel, invoke } from "@tauri-apps/api/core";
import { withTimeout } from "./model";

/** Rust updater::UpdateInfo 的 serde 契约镜像 */
export type UpdateInfo = {
  version: string;
  current_version: string;
  notes: string | null;
};

/** Rust updater::InstallProgress 的 serde 契约镜像（tag = "event"，蛇形变体名） */
export type InstallProgress =
  | { event: "started"; total: number | null }
  | { event: "chunk"; downloaded: number; total: number | null }
  | { event: "installing" };

/**
 * 更新流程状态机（单向为主）：
 * idle → checking → available → downloading → installing → installed
 *                 ↘ upToDate / checkFailed（手动检查的短暂反馈，自动回落 idle）
 * downloading/installing 失败 → installFailed（可重试，回 downloading）。
 */
export type UpdaterState =
  | { phase: "idle" }
  | { phase: "checking"; manual: boolean }
  | { phase: "upToDate" }
  | { phase: "checkFailed"; message: string }
  | { phase: "available"; info: UpdateInfo }
  | { phase: "downloading"; info: UpdateInfo; downloaded: number; total: number | null }
  | { phase: "installing"; info: UpdateInfo }
  | { phase: "installed"; info: UpdateInfo }
  | { phase: "installFailed"; info: UpdateInfo; message: string };

/** 检查更新的前端兜底超时。后端自带 30s 网络超时，这层只防 invoke 永不 settle
 *  （与 model.ts ACTION_TIMEOUT_MS 同一故障类），故取更宽的值。 */
export const CHECK_TIMEOUT_MS = 45_000;

/** 下载 + 安装的兜底超时。慢速网络下载一个 ~15 MB 的包也该在此之内；
 *  超过它更可能是后端挂死 —— 与其永久转圈，不如给一条可重试的失败。 */
export const INSTALL_TIMEOUT_MS = 10 * 60_000;

/** 手动检查的短暂反馈（已是最新 / 检查失败）在 footer 停留多久后回落 idle。 */
export const TRANSIENT_REVERT_MS = 6_000;

/** 自动检查节奏：启动时一次 + 之后每 24h（常驻托盘应用，进程可以活很多天）。 */
const AUTO_CHECK_INTERVAL_MS = 24 * 60 * 60_000;

/** 应用内更新：状态机 + 动作。弹窗开合是独立一路（modalOpen）——
 *  「稍后」收起弹窗后 footer 徽标仍在，available 状态不因此丢失。 */
export function useUpdater() {
  const [state, setState] = useState<UpdaterState>({ phase: "idle" });
  const [modalOpen, setModalOpen] = useState(false);
  // 状态的 ref 镜像：动作函数要在**同步代码里**读当前状态做准入判断，而
  // setState 的函数式 updater 是延迟执行的（React 批处理），闭包读不到。
  const stateRef = useRef<UpdaterState>(state);
  useEffect(() => {
    stateRef.current = state;
  }, [state]);
  // 短暂反馈的回落定时器：新动作开始时清掉，避免旧定时器把新状态打回 idle
  const revertTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  // install 在飞行中（invoke 未 settle 期间 state 会多次变化，用 ref 判飞行）
  const installing = useRef(false);

  const clearRevert = useCallback(() => {
    if (revertTimer.current) {
      clearTimeout(revertTimer.current);
      revertTimer.current = null;
    }
  }, []);

  const scheduleRevert = useCallback(() => {
    clearRevert();
    revertTimer.current = setTimeout(() => {
      setState((s) =>
        s.phase === "upToDate" || s.phase === "checkFailed" ? { phase: "idle" } : s,
      );
    }, TRANSIENT_REVERT_MS);
  }, [clearRevert]);

  const check = useCallback(
    async (manual: boolean) => {
      // 安装流程进行中/已有检查在飞 —— 绝不被一次（定时的）检查打断
      const cur = stateRef.current.phase;
      if (
        installing.current ||
        cur === "checking" ||
        cur === "downloading" ||
        cur === "installing" ||
        cur === "installed" ||
        cur === "installFailed"
      ) {
        return;
      }
      clearRevert();
      setState({ phase: "checking", manual });
      try {
        const info = await withTimeout(
          invoke<UpdateInfo | null>("check_update"),
          CHECK_TIMEOUT_MS,
          "ERR_ACTION_TIMEOUT",
        );
        setState((s) => {
          if (s.phase !== "checking") return s; // 状态已被别的流程接管
          if (info) return { phase: "available", info };
          if (s.manual) return { phase: "upToDate" };
          return { phase: "idle" };
        });
        if (info && manual) setModalOpen(true);
        if (!info && manual) scheduleRevert();
      } catch (err) {
        // 自动检查失败保持静默（第一个带 latest.json 的 release 发布之前，
        // 端点 404 是常态）；手动检查给一条短暂的失败反馈。
        setState((s) => {
          if (s.phase !== "checking") return s;
          return s.manual ? { phase: "checkFailed", message: String(err) } : { phase: "idle" };
        });
        if (manual) scheduleRevert();
      }
    },
    [clearRevert, scheduleRevert],
  );

  const install = useCallback(async () => {
    const s = stateRef.current;
    if (installing.current || (s.phase !== "available" && s.phase !== "installFailed")) return;
    const info = s.info;
    installing.current = true;
    setState({ phase: "downloading", info, downloaded: 0, total: null });
    setModalOpen(true);
    const onProgress = new Channel<InstallProgress>();
    onProgress.onmessage = (msg) => {
      setState((s) => {
        // 只在下载/安装态接收进度：兜底超时已把状态判成失败后，迟到的消息不再翻盘
        if (s.phase !== "downloading" && s.phase !== "installing") return s;
        switch (msg.event) {
          case "started":
            return { phase: "downloading", info: s.info, downloaded: 0, total: msg.total };
          case "chunk":
            return {
              phase: "downloading",
              info: s.info,
              downloaded: msg.downloaded,
              total: msg.total,
            };
          case "installing":
            return { phase: "installing", info: s.info };
        }
      });
    };
    try {
      await withTimeout(
        invoke<void>("install_update", { onProgress }),
        INSTALL_TIMEOUT_MS,
        "ERR_ACTION_TIMEOUT",
      );
      setState({ phase: "installed", info });
    } catch (err) {
      setState({ phase: "installFailed", info, message: String(err) });
    } finally {
      installing.current = false;
    }
  }, []);

  /** 安装完成后的重启。Windows 上 NSIS 安装器可能已经在接管进程，此调用
   *  失败与否都不再有可展示的下文 —— fire-and-forget。 */
  const restart = useCallback(() => {
    invoke("restart_app").catch(() => {});
  }, []);

  /** 打开本次更新的 GitHub Release 页（Rust 侧拼 URL，见 updater.rs 的安全注释） */
  const openReleasePage = useCallback((version: string) => {
    invoke("open_release_page", { version }).catch(() => {});
  }, []);

  const openModal = useCallback(() => setModalOpen(true), []);
  const closeModal = useCallback(() => setModalOpen(false), []);

  // 自动检查：启动一次 + 每 24h。dev 下跳过 —— 端点上是正式版的 latest.json，
  // 对着 debug 构建报「有新版本」只会制造噪音（手动检查仍可用，便于联调）。
  useEffect(() => {
    if (import.meta.env.DEV) return;
    void check(false);
    const id = setInterval(() => void check(false), AUTO_CHECK_INTERVAL_MS);
    return () => clearInterval(id);
  }, [check]);

  useEffect(() => clearRevert, [clearRevert]);

  return { state, modalOpen, check, install, restart, openReleasePage, openModal, closeModal };
}
