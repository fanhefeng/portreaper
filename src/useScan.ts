import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { SCAN_TIMEOUT_MS, sweepableEntries, withTimeout, type ProcessEntry } from "./model";

/** 轮询间隔 —— 产品口径「每 2 秒自动扫描」（footer 文案与 CLAUDE.md 同）。 */
const POLL_INTERVAL_MS = 2000;

/**
 * 扫描轮询 hook：entries / scanError 状态 + 2s 轮询 + 托盘计数推送。
 * 从 App.tsx 抽出（评审发现：这是容器里最微妙的并发逻辑 —— inFlight 复用与
 * freshScan「等在途再扫」语义 —— 此前只能靠整棵渲染 + 假定时器间接覆盖，
 * 抽成 hook 后可直接单测，见 useScan.test.ts）。
 *
 * 错误语义（评审发现）：scanError 由下一次成功轮询自动清除（自愈）；与之相对的
 * actionError（kill / 清扫 / 收藏）属于操作流程，留在 App —— 两路的展示优先级
 * 与「点击关闭」也由 App 决定，本 hook 只负责扫描一路。
 */
export function useScan() {
  const [entries, setEntries] = useState<ProcessEntry[]>([]);
  const [scanError, setScanError] = useState<string | null>(null);
  // 「还没扫过」与「扫过了，是空的」必须分得开：entries 初值是 []，不区分的话
  // 每次启动的第一句话都是「没有发现任何监听端口」—— 在任何一台 Mac 上都是假话，
  // 首轮扫描失败时更是自信地宣布它（评审发现）。
  const [hasScanned, setHasScanned] = useState(false);
  // 与 scanError 分开的一路事实：**最近一轮扫描成没成功**。scanError 是给横幅用的，
  // 用户点一下就清掉；而空态文案不能因此退化成「没有发现任何监听端口」——
  // 那是一句假话，且会连带弄丢唯一的重试入口（评审发现）。
  const [lastScanOk, setLastScanOk] = useState(true);
  const inFlight = useRef<Promise<ProcessEntry[] | null> | null>(null);

  /** 返回本轮 entries（失败时 null）—— kill 之后的「目标真的消失了吗」要用它，
   *  只写进 state 的话调用方拿不到与本次动作对应的那一份快照。 */
  const runScan = useCallback(async (): Promise<ProcessEntry[] | null> => {
    try {
      const data = await withTimeout(
        invoke<ProcessEntry[]>("scan_ports"),
        SCAN_TIMEOUT_MS,
        "ERR_SCAN_TIMEOUT",
      );
      setEntries(data);
      setScanError(null);
      setHasScanned(true);
      setLastScanOk(true);
      // 托盘只计入会被清扫的层级（Confirmed + Likely），避免宽限期内的闪烁。
      // 托盘更新是装饰性的 —— fire-and-forget（与 i18n.ts 的 set_tray_language
      // 同一约定），绝不能让它把一次成功的扫描标成错误（评审发现）。
      const suspectCount = sweepableEntries(data).length;
      const totalPorts = data.reduce((sum, e) => sum + e.ports.length, 0);
      invoke("update_tray_title", {
        count: totalPorts,
        suspectCount,
      }).catch(() => {});
      return data;
    } catch (e) {
      setScanError(String(e));
      // 失败也算「扫过了」：空态文案要说「扫描失败」而不是「一切正常」
      setHasScanned(true);
      setLastScanOk(false);
      return null;
    }
  }, []);

  /** 轮询入口：已有扫描在跑则复用它的 Promise（防并发扫描） */
  const refresh = useCallback((): Promise<ProcessEntry[] | null> => {
    if (!inFlight.current) {
      inFlight.current = runScan().finally(() => {
        inFlight.current = null;
      });
    }
    return inFlight.current;
  }, [runScan]);

  /** kill / 收藏之后用：先等正在跑的扫描收尾，再扫一次，确保拿到变更后的真实状态
   *（runScan 内部消化所有异常，这里的 await 不会抛） */
  const freshScan = useCallback(async (): Promise<ProcessEntry[] | null> => {
    if (inFlight.current) await inFlight.current;
    return refresh();
  }, [refresh]);

  useEffect(() => {
    refresh();
    const id = setInterval(refresh, POLL_INTERVAL_MS);
    return () => clearInterval(id);
  }, [refresh]);

  /** 错误横幅「点击关闭」用：清掉扫描错误（操作错误由 App 自己清）。 */
  const clearScanError = useCallback(() => setScanError(null), []);

  return { entries, scanError, hasScanned, lastScanOk, clearScanError, freshScan };
}
