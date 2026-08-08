import type { ProcessEntry } from "../model";
import { ProcessRow, type RowShared } from "./ProcessRow";

type SectionProps = {
  title: string;
  sub?: string;
  danger?: boolean;
  entries: ProcessEntry[];
  expandedPid: number | null;
  shared: RowShared;
};

/** 嫌疑 / 健康 / 收藏三个分区共用：section-head + 行列表（评审 E4 去重）。
 *  计数直接取 entries.length —— 曾是独立 count prop，三个调用点恒等于它（评审发现）。 */
export function Section({ title, sub, danger, entries, expandedPid, shared }: SectionProps) {
  return (
    <section>
      <div className={danger ? "section-head section-head-danger" : "section-head"}>
        <span className="section-title">{title}</span>
        <span className="section-count">{entries.length}</span>
        {sub && <span className="section-sub">{sub}</span>}
      </div>
      {entries.map((e) => (
        <ProcessRow key={e.pid} e={e} expanded={expandedPid === e.pid} shared={shared} />
      ))}
    </section>
  );
}
