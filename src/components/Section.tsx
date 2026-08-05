import type { ProcessEntry } from "../model";
import { ProcessRow, type RowProps } from "./ProcessRow";

type SectionProps = {
  title: string;
  count: number;
  sub?: string;
  danger?: boolean;
  entries: ProcessEntry[];
  expandedPid: number | null;
  rowProps: Omit<RowProps, "e" | "expanded">;
};

/** 嫌疑 / 健康 / 收藏三个分区共用：section-head + 行列表（评审 E4 去重） */
export function Section({
  title,
  count,
  sub,
  danger,
  entries,
  expandedPid,
  rowProps,
}: SectionProps) {
  return (
    <section>
      <div className={danger ? "section-head section-head-danger" : "section-head"}>
        <span className="section-title">{title}</span>
        <span className="section-count">{count}</span>
        {sub && <span className="section-sub">{sub}</span>}
      </div>
      {entries.map((e) => (
        <ProcessRow key={e.pid} e={e} expanded={expandedPid === e.pid} {...rowProps} />
      ))}
    </section>
  );
}
