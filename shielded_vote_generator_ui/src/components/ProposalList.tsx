import { Plus, GripVertical, AlertTriangle, CheckCircle2, MoreHorizontal, Copy, Trash2 } from "lucide-react";
import { useState, useRef } from "react";
import type { Proposal } from "../types";

interface ProposalListProps {
  proposals: Proposal[];
  activeProposalId: string | null;
  onSelectProposal: (id: string) => void;
  onAddProposal: () => void;
  onDuplicateProposal: (id: string) => void;
  onDeleteProposal: (id: string) => void;
  onReorder: (from: number, to: number) => void;
}

function isProposalValid(p: Proposal): boolean {
  return p.title.trim().length > 0 && p.options.length >= 2;
}

export function ProposalList({
  proposals,
  activeProposalId,
  onSelectProposal,
  onAddProposal,
  onDuplicateProposal,
  onDeleteProposal,
  onReorder,
}: ProposalListProps) {
  const [menuOpen, setMenuOpen] = useState<string | null>(null);
  const dragItem = useRef<number | null>(null);
  const dragOver = useRef<number | null>(null);

  const handleDragStart = (index: number) => {
    dragItem.current = index;
  };

  const handleDragEnter = (index: number) => {
    dragOver.current = index;
  };

  const handleDragEnd = () => {
    if (dragItem.current !== null && dragOver.current !== null && dragItem.current !== dragOver.current) {
      onReorder(dragItem.current, dragOver.current);
    }
    dragItem.current = null;
    dragOver.current = null;
  };

  return (
    <div className="flex flex-col h-full">
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 border-b border-border-subtle">
        <h3 className="text-xs font-semibold text-text-primary">Proposals</h3>
        <div className="flex items-center gap-2">
          <button
            onClick={onAddProposal}
            className="flex items-center gap-1.5 px-2.5 py-1 bg-surface-3 hover:bg-accent-dim/30 text-text-secondary hover:text-accent-glow rounded-md text-[11px] transition-colors cursor-pointer"
          >
            <Plus size={12} />
            Add proposal
          </button>
        </div>
      </div>

      {/* List */}
      <div className="flex-1 overflow-y-auto p-2">
        {proposals.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-full text-center px-6 py-12">
            <div className="w-12 h-12 rounded-full bg-surface-3 flex items-center justify-center mb-3">
              <Plus size={20} className="text-text-muted" />
            </div>
            <p className="text-xs text-text-muted mb-3">
              Add your first proposal
            </p>
            <button
              onClick={onAddProposal}
              className="flex items-center gap-1.5 px-3 py-1.5 bg-accent/90 hover:bg-accent text-surface-0 rounded-lg text-[11px] font-semibold transition-colors cursor-pointer"
            >
              <Plus size={12} />
              Add Support/Oppose proposal
            </button>
          </div>
        ) : (
          <div className="flex flex-col gap-1">
            {proposals.map((proposal, index) => {
              const valid = isProposalValid(proposal);
              return (
                <div
                  key={proposal.id}
                  draggable
                  onDragStart={() => handleDragStart(index)}
                  onDragEnter={() => handleDragEnter(index)}
                  onDragEnd={handleDragEnd}
                  onDragOver={(e) => e.preventDefault()}
                  onClick={() => onSelectProposal(proposal.id)}
                  className={`group flex items-center gap-2 px-2 py-2 rounded-lg cursor-pointer transition-colors ${
                    activeProposalId === proposal.id
                      ? "bg-surface-3 border border-border"
                      : "hover:bg-surface-2 border border-transparent"
                  }`}
                >
                  <GripVertical
                    size={14}
                    className="text-text-muted opacity-0 group-hover:opacity-100 transition-opacity cursor-grab shrink-0"
                  />
                  <span className="text-[10px] font-bold text-text-muted bg-surface-2 rounded px-1.5 py-0.5 shrink-0">
                    {String(index + 1).padStart(2, "0")}
                  </span>
                  <div className="flex-1 min-w-0">
                    <div className="flex items-center gap-2">
                      <span className="text-xs text-text-primary truncate">
                        {proposal.title || "Untitled proposal"}
                      </span>
                      <span className="text-[9px] text-text-muted shrink-0">
                        {proposal.type === "binary" ? "Binary" : "Multi-Choice"}
                      </span>
                    </div>
                  </div>
                  <div className="flex items-center gap-1 shrink-0">
                    {valid ? (
                      <CheckCircle2 size={13} className="text-success" />
                    ) : (
                      <AlertTriangle size={13} className="text-warning" />
                    )}
                    <div className="relative">
                      <button
                        onClick={(e) => {
                          e.stopPropagation();
                          setMenuOpen(menuOpen === proposal.id ? null : proposal.id);
                        }}
                        className="p-0.5 rounded hover:bg-surface-3 text-text-muted opacity-0 group-hover:opacity-100 transition-opacity cursor-pointer"
                      >
                        <MoreHorizontal size={14} />
                      </button>
                      {menuOpen === proposal.id && (
                        <div className="absolute right-0 top-6 z-10 bg-surface-2 border border-border rounded-lg shadow-lg py-1 min-w-[130px]">
                          <button
                            onClick={(e) => {
                              e.stopPropagation();
                              onDuplicateProposal(proposal.id);
                              setMenuOpen(null);
                            }}
                            className="w-full flex items-center gap-2 px-3 py-1.5 text-[11px] text-text-secondary hover:bg-surface-3 hover:text-text-primary cursor-pointer"
                          >
                            <Copy size={12} /> Duplicate
                          </button>
                          <button
                            onClick={(e) => {
                              e.stopPropagation();
                              onDeleteProposal(proposal.id);
                              setMenuOpen(null);
                            }}
                            className="w-full flex items-center gap-2 px-3 py-1.5 text-[11px] text-danger hover:bg-surface-3 cursor-pointer"
                          >
                            <Trash2 size={12} /> Delete
                          </button>
                        </div>
                      )}
                    </div>
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
