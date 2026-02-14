import type { RoundStatus } from "../types";

const STATUS_COLORS: Record<RoundStatus, string> = {
  draft: "bg-surface-3 text-text-secondary",
  in_progress: "bg-accent-dim/40 text-accent-glow",
  published: "bg-success/20 text-success",
  archived: "bg-surface-3 text-text-muted",
};

const STATUS_LABELS: Record<RoundStatus, string> = {
  draft: "Draft",
  in_progress: "In Progress",
  published: "Published",
  archived: "Archived",
};

export function StatusPill({ status }: { status: RoundStatus }) {
  return (
    <span
      className={`text-[10px] px-2 py-0.5 rounded-full font-medium ${STATUS_COLORS[status]}`}
    >
      {STATUS_LABELS[status]}
    </span>
  );
}
