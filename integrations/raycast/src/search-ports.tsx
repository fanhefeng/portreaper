/**
 * Portreaper 的 Raycast 前端。
 *
 * 与桌面版的分工：这里是「快速处置」——找到残留、一键终止、加星豁免。判定的
 * 完整解释（证据链、启动链、豁免理由的双语文案）留给桌面版的详情面板。
 *
 * 关于理由为什么显示成 `ppid1_orphan` 这样的机器码：翻译属于「表达」，是前端的
 * 事；而桌面版的文案住在 `src/i18n.ts`，那个模块在顶层访问 localStorage /
 * navigator，Node 环境 import 不进来。与其为 Raycast 复制第二份文案（第二份真相
 * 源 + 第二条漂移路径），不如诚实地显示引擎的原始判定码 —— 本扩展的用户是开发者，
 * `ppid1_orphan` 比一句含糊的翻译更有信息量。
 */

import { useEffect, useState } from "react";
import {
  Action,
  ActionPanel,
  Alert,
  Color,
  Icon,
  List,
  Toast,
  confirmAlert,
  getPreferenceValues,
  showToast,
} from "@raycast/api";

import {
  CliNotFoundError,
  SchemaMismatchError,
  type Confidence,
  type ProcessEntry,
  kill,
  resolveCliPath,
  scan,
  verifyCli,
  whitelist,
  whitelistKey,
} from "./cli";

type Prefs = { cliPath?: string };

type State =
  | { kind: "loading" }
  | { kind: "ready"; cliPath: string; entries: ProcessEntry[] }
  | { kind: "no-cli"; searched: string[] }
  | { kind: "error"; message: string };

const CONFIDENCE_COLOR: Record<Confidence, Color> = {
  confirmed: Color.Red,
  likely: Color.Orange,
  possible: Color.Yellow,
  none: Color.SecondaryText,
};

export default function SearchPorts() {
  const [state, setState] = useState<State>({ kind: "loading" });
  // 默认展开详情：本工具的卖点是「为什么判它是残留」，藏起证据就只剩一个端口列表。
  const [showDetail, setShowDetail] = useState(true);

  async function load() {
    const prefs = getPreferenceValues<Prefs>();
    const searched = [
      prefs.cliPath?.trim() || "(preference not set)",
      "/Applications/Portreaper.app/Contents/MacOS/portreaper-cli",
      "~/.cargo/bin/portreaper-cli",
      "$PATH",
    ];
    try {
      const cliPath = resolveCliPath(prefs.cliPath);
      await verifyCli(cliPath, searched);
      const report = await scan(cliPath);
      setState({ kind: "ready", cliPath, entries: report.entries });
    } catch (e) {
      if (e instanceof CliNotFoundError) {
        setState({ kind: "no-cli", searched: e.searched });
      } else if (e instanceof SchemaMismatchError) {
        setState({
          kind: "error",
          message: `${e.message}. Update this extension (or the CLI) so both speak the same contract.`,
        });
      } else {
        setState({ kind: "error", message: e instanceof Error ? e.message : String(e) });
      }
    }
  }

  useEffect(() => {
    void load();
    // 只在首次挂载时扫描；后续刷新走 Action（避免每次渲染都 spawn 一个进程）
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  if (state.kind === "no-cli") {
    return <NotFoundView searched={state.searched} onRetry={load} />;
  }
  if (state.kind === "error") {
    return (
      <List>
        <List.EmptyView
          icon={{ source: Icon.Warning, tintColor: Color.Red }}
          title="Scan failed"
          description={state.message}
          actions={
            <ActionPanel>
              <Action title="Retry" icon={Icon.ArrowClockwise} onAction={load} />
            </ActionPanel>
          }
        />
      </List>
    );
  }

  const loading = state.kind === "loading";
  const entries = state.kind === "ready" ? state.entries : [];
  const cliPath = state.kind === "ready" ? state.cliPath : "";

  // 分组与桌面版一致：疑似 → 收藏 → 其余。引擎已按「疑似优先 + 置信度」排好序，
  // 这里只做分桶，不再二次排序（排序规则属于引擎，前端重排会与桌面版视觉不一致）。
  const suspects = entries.filter((e) => e.is_zombie_suspect);
  const starred = entries.filter((e) => e.is_whitelisted);
  const healthy = entries.filter((e) => !e.is_zombie_suspect && !e.is_whitelisted);

  const shared = {
    cliPath,
    onChanged: load,
    showDetail,
    onToggleDetail: () => setShowDetail((v) => !v),
  };

  return (
    <List
      isLoading={loading}
      isShowingDetail={showDetail && entries.length > 0}
      searchBarPlaceholder="Filter by name, port, or PID…"
    >
      <Section title="Suspects" entries={suspects} {...shared} />
      <Section title="Starred" entries={starred} {...shared} />
      <Section title="Healthy" entries={healthy} {...shared} />
      {!loading && entries.length === 0 && (
        <List.EmptyView
          icon={Icon.Check}
          title="Nothing is listening"
          description="No listening processes and no orphaned dev processes."
        />
      )}
    </List>
  );
}

type SharedProps = {
  cliPath: string;
  onChanged: () => void;
  showDetail: boolean;
  onToggleDetail: () => void;
};

function Section(props: { title: string; entries: ProcessEntry[] } & SharedProps) {
  const { title, entries, ...shared } = props;
  if (entries.length === 0) return null;
  return (
    <List.Section title={title} subtitle={String(entries.length)}>
      {entries.map((e) => (
        <Row key={e.pid} entry={e} {...shared} />
      ))}
    </List.Section>
  );
}

function Row({ entry, ...shared }: { entry: ProcessEntry } & SharedProps) {
  const ports = entry.ports.length > 0 ? entry.ports.join(", ") : "no port";
  const accessories: List.Item.Accessory[] = [];

  if (entry.is_whitelisted) {
    accessories.push({ icon: { source: Icon.Star, tintColor: Color.Yellow } });
  }
  if (entry.is_zombie_suspect) {
    accessories.push({
      tag: { value: entry.confidence, color: CONFIDENCE_COLOR[entry.confidence] },
    });
  }
  // 子树 CPU 而非行内 CPU：headless 浏览器把 CPU 全烧在子进程里，
  // 主进程读数是 ~0%（这正是桌面版 cpu_percent_tree 存在的原因）
  if (entry.cpu_percent_tree >= 1) {
    accessories.push({ text: `${entry.cpu_percent_tree.toFixed(0)}% cpu` });
  }
  accessories.push({ text: `pid ${entry.pid}` });

  return (
    <List.Item
      icon={entry.is_zombie_suspect ? Icon.ExclamationMark : Icon.CircleFilled}
      title={entry.app_label || entry.command}
      subtitle={ports}
      accessories={accessories}
      keywords={[String(entry.pid), ...entry.ports.map(String), entry.command, entry.app_category]}
      detail={shared.showDetail ? <Detail entry={entry} /> : undefined}
      actions={<Actions entry={entry} {...shared} />}
    />
  );
}

function Detail({ entry }: { entry: ProcessEntry }) {
  const chain = entry.parent_chain.map((p) => `${p.label} (${p.pid})`).join(" → ") || "—";
  const md = [
    `# ${entry.app_label || entry.command}`,
    "",
    `\`${entry.full_command || entry.command}\``,
    "",
    "## Why it is listed",
    "",
    entry.zombie_reasons.length > 0
      ? entry.zombie_reasons.map((r) => `- \`${r}\``).join("\n")
      : "- not flagged",
    "",
    "> Codes come straight from the engine. The desktop app explains each one in full.",
    "",
    "## Launcher chain",
    "",
    chain,
  ].join("\n");

  return (
    <List.Item.Detail
      markdown={md}
      metadata={
        <List.Item.Detail.Metadata>
          <List.Item.Detail.Metadata.Label title="PID" text={String(entry.pid)} />
          <List.Item.Detail.Metadata.Label title="Category" text={entry.app_category} />
          <List.Item.Detail.Metadata.Label
            title="Ports"
            text={entry.ports.length > 0 ? entry.ports.join(", ") : "—"}
          />
          <List.Item.Detail.Metadata.Label title="Uptime" text={formatUptime(entry.elapsed_secs)} />
          <List.Item.Detail.Metadata.Label
            title="CPU (self / tree)"
            text={`${entry.cpu_percent.toFixed(1)}% / ${entry.cpu_percent_tree.toFixed(1)}%`}
          />
          <List.Item.Detail.Metadata.Label title="Memory" text={`${entry.mem_mb.toFixed(0)} MB`} />
          <List.Item.Detail.Metadata.Label title="User" text={entry.user || "—"} />
          <List.Item.Detail.Metadata.Label title="Executable" text={entry.exe_path || "—"} />
        </List.Item.Detail.Metadata>
      }
    />
  );
}

function Actions({
  entry,
  cliPath,
  onChanged,
  onToggleDetail,
}: { entry: ProcessEntry } & SharedProps) {
  async function doKill(force: boolean) {
    // 没有身份令牌就不该走到这里（引擎会 fail-closed 拒绝）——但提前挡住，
    // 给出的提示比引擎的通用错误更有指导性。
    if (entry.start_unix == null) {
      await showToast({
        style: Toast.Style.Failure,
        title: "No identity token",
        message: "Refresh the list first — killing without one is refused by design.",
      });
      return;
    }
    const ok = await confirmAlert({
      title: `Terminate ${entry.app_label || entry.command}?`,
      message: `PID ${entry.pid}${entry.ports.length ? ` · port ${entry.ports.join(", ")}` : ""}`,
      icon: Icon.Trash,
      primaryAction: {
        title: force ? "Force kill" : "Terminate",
        style: Alert.ActionStyle.Destructive,
      },
    });
    if (!ok) return;

    const toast = await showToast({ style: Toast.Style.Animated, title: "Terminating…" });
    try {
      await kill(cliPath, entry.pid, entry.start_unix, force);
      toast.style = Toast.Style.Success;
      toast.title = `Terminated ${entry.pid}`;
      onChanged();
    } catch (e) {
      toast.style = Toast.Style.Failure;
      toast.title = "Could not terminate";
      toast.message = e instanceof Error ? e.message : String(e);
    }
  }

  async function toggleStar() {
    const action = entry.is_whitelisted ? "remove" : "add";
    const toast = await showToast({ style: Toast.Style.Animated, title: "Saving…" });
    try {
      await whitelist(cliPath, action, whitelistKey(entry));
      toast.style = Toast.Style.Success;
      toast.title = action === "add" ? "Starred" : "Unstarred";
      onChanged();
    } catch (e) {
      toast.style = Toast.Style.Failure;
      toast.title = "Could not update the whitelist";
      toast.message = e instanceof Error ? e.message : String(e);
    }
  }

  return (
    <ActionPanel>
      <ActionPanel.Section>
        <Action
          title="Terminate"
          icon={Icon.Trash}
          style={Action.Style.Destructive}
          onAction={() => doKill(false)}
        />
        <Action
          title="Force Kill"
          icon={Icon.Trash}
          style={Action.Style.Destructive}
          shortcut={{ modifiers: ["cmd", "shift"], key: "backspace" }}
          onAction={() => doKill(true)}
        />
      </ActionPanel.Section>
      <ActionPanel.Section>
        <Action
          title={entry.is_whitelisted ? "Remove Star" : "Star (Exempt from Suspicion)"}
          icon={Icon.Star}
          shortcut={{ modifiers: ["cmd"], key: "s" }}
          onAction={toggleStar}
        />
        <Action
          title="Refresh"
          icon={Icon.ArrowClockwise}
          shortcut={{ modifiers: ["cmd"], key: "r" }}
          onAction={onChanged}
        />
        <Action
          title="Toggle Details"
          icon={Icon.Sidebar}
          shortcut={{ modifiers: ["cmd"], key: "d" }}
          onAction={onToggleDetail}
        />
      </ActionPanel.Section>
      <ActionPanel.Section>
        <Action.CopyToClipboard title="Copy PID" content={String(entry.pid)} />
        <Action.CopyToClipboard
          title="Copy Command"
          content={entry.full_command || entry.command}
        />
      </ActionPanel.Section>
    </ActionPanel>
  );
}

function NotFoundView({ searched, onRetry }: { searched: string[]; onRetry: () => void }) {
  const md = [
    "# portreaper-cli not found",
    "",
    "This extension drives the same engine as the Portreaper desktop app — it needs the",
    "`portreaper-cli` binary to talk to it.",
    "",
    "**Looked in:**",
    "",
    ...searched.map((s) => `- \`${s}\``),
    "",
    "**How to get it:**",
    "",
    "- Build from the repo: `cargo build --release -p portreaper-cli`, then point the",
    "  extension preference at `target/release/portreaper-cli`",
    "- Or install it on your `PATH`: `cargo install --path crates/portreaper-cli`",
  ].join("\n");

  return (
    <List>
      <List.EmptyView
        icon={{ source: Icon.QuestionMark, tintColor: Color.Orange }}
        title="portreaper-cli not found"
        description={md}
        actions={
          <ActionPanel>
            <Action title="Retry" icon={Icon.ArrowClockwise} onAction={onRetry} />
          </ActionPanel>
        }
      />
    </List>
  );
}

function formatUptime(secs: number): string {
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m`;
  if (secs < 86400) return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`;
  return `${Math.floor(secs / 86400)}d ${Math.floor((secs % 86400) / 3600)}h`;
}
