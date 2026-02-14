import { useState, useCallback, useRef } from "react";
import { Sidebar } from "./components/Sidebar";
import { TopBar } from "./components/TopBar";
import { ProposalList } from "./components/ProposalList";
import { ProposalEditor } from "./components/ProposalEditor";
import { JsonView } from "./components/JsonView";
import { RoundEditor } from "./components/RoundEditor";
import { RoundsList } from "./components/RoundsList";
import { useStore } from "./store/useStore";
import { Shield, Plus, FileText, Settings, Link2, Server } from "lucide-react";
import type { Proposal, RoundSettings, RoundStatus, VotingRound } from "./types";
import { Settings2 } from "lucide-react";

type Section = "about" | "rounds" | "builder" | "json" | "downloads" | "preview" | "settings";

function App() {
  const store = useStore();
  const [section, setSection] = useState<Section>("about");
  const [filter, setFilter] = useState<RoundStatus | "all">("all");
  const importRef = useRef<HTMLInputElement>(null);

  const handleSelectRound = useCallback(
    (id: string) => {
      store.setActiveRoundId(id);
      store.setActiveProposalId(null);
      setSection("builder");
    },
    [store]
  );

  const handleCreateRound = useCallback(() => {
    store.createRound();
    setSection("builder");
  }, [store]);

  const handleImportJson = useCallback(() => {
    importRef.current?.click();
  }, []);

  const handleFileImport = useCallback(
    (e: React.ChangeEvent<HTMLInputElement>) => {
      const file = e.target.files?.[0];
      if (!file) return;
      const reader = new FileReader();
      reader.onload = (ev) => {
        try {
          const data = JSON.parse(ev.target?.result as string);
          const roundData = data.round ?? data;
          const round = store.createRound(roundData.name ?? "Imported Round");
          if (roundData.proposals) {
            store.updateRound(round.id, { proposals: roundData.proposals });
          }
          if (roundData.settings) {
            store.updateRound(round.id, { settings: roundData.settings });
          }
          setSection("builder");
        } catch {
          alert("Invalid JSON file");
        }
      };
      reader.readAsText(file);
      e.target.value = "";
    },
    [store]
  );

  const handleNavigate = useCallback(
    (s: string) => {
      setSection(s as Section);
    },
    []
  );

  const sampleRound = store.rounds.find((r) => r.name.includes("(SAMPLE)"));

  return (
    <div className="flex h-screen overflow-hidden bg-surface-0">
      <input
        ref={importRef}
        type="file"
        accept=".json"
        className="hidden"
        onChange={handleFileImport}
      />

      <Sidebar
        rounds={store.rounds}
        activeRoundId={store.activeRoundId}
        activeFilter={filter}
        onFilterChange={setFilter}
        onSelectRound={handleSelectRound}
        onCreateRound={handleCreateRound}
        onImportJson={handleImportJson}
        onNavigate={handleNavigate}
        currentSection={section}
      />

      <main className="flex-1 flex flex-col overflow-hidden">
        {/* About page */}
        {section === "about" && (
          <AboutPage
            onCreateRound={handleCreateRound}
            onOpenSample={
              sampleRound
                ? () => handleSelectRound(sampleRound.id)
                : undefined
            }
          />
        )}

        {/* Rounds list */}
        {section === "rounds" && (
          <RoundsList
            rounds={store.rounds}
            activeFilter={filter}
            onFilterChange={setFilter}
            onSelectRound={handleSelectRound}
            onDuplicate={(id) => store.duplicateRound(id)}
            onArchive={(id) => store.setRoundStatus(id, "archived")}
          />
        )}

        {/* Builder with round selected */}
        {section === "builder" && store.activeRound && (
          <>
            <TopBar
              round={store.activeRound}
              saveState={store.saveState}
              onUpdateName={(name) =>
                store.updateRound(store.activeRound!.id, { name })
              }
              onPublish={() =>
                store.setRoundStatus(store.activeRound!.id, "published")
              }
              onExportJson={() => setSection("json")}
              onPreview={() => setSection("preview")}
              onDuplicate={() => store.duplicateRound(store.activeRound!.id)}
              onArchive={() =>
                store.setRoundStatus(store.activeRound!.id, "archived")
              }
              onDelete={() => {
                store.deleteRound(store.activeRound!.id);
                setSection("rounds");
              }}
              onNavigate={handleNavigate}
            />
            <div className="flex flex-1 overflow-hidden">
              <div className="w-[380px] min-w-[320px] border-r border-border bg-surface-0 overflow-hidden">
                <ProposalList
                  proposals={store.activeRound.proposals}
                  activeProposalId={store.activeProposalId}
                  onSelectProposal={(id) => store.setActiveProposalId(id)}
                  onAddProposal={() =>
                    store.addProposal(store.activeRound!.id)
                  }
                  onDuplicateProposal={(id) =>
                    store.duplicateProposal(store.activeRound!.id, id)
                  }
                  onDeleteProposal={(id) =>
                    store.deleteProposal(store.activeRound!.id, id)
                  }
                  onReorder={(from, to) =>
                    store.reorderProposals(store.activeRound!.id, from, to)
                  }
                />
              </div>
              <RightPanel
                round={store.activeRound}
                activeProposal={store.activeProposal}
                onUpdateRoundName={(name) =>
                  store.updateRound(store.activeRound!.id, { name })
                }
                onUpdateRoundSettings={(patch) =>
                  store.updateRound(store.activeRound!.id, {
                    settings: { ...store.activeRound!.settings, ...patch },
                  })
                }
                onUpdateProposal={(patch) =>
                  store.updateProposal(
                    store.activeRound!.id,
                    store.activeProposal!.id,
                    patch
                  )
                }
                onDeleteProposal={() =>
                  store.deleteProposal(
                    store.activeRound!.id,
                    store.activeProposal!.id
                  )
                }
              />
            </div>
          </>
        )}

        {/* Builder with no round */}
        {section === "builder" && !store.activeRound && (
          <div className="flex items-center justify-center h-full">
            <div className="text-center">
              <p className="text-xs text-text-muted mb-3">
                No round selected
              </p>
              <button
                onClick={handleCreateRound}
                className="px-4 py-2 bg-accent/90 hover:bg-accent text-surface-0 rounded-lg text-xs font-semibold transition-colors cursor-pointer"
              >
                Create a new round
              </button>
            </div>
          </div>
        )}

        {/* JSON view */}
        {section === "json" && store.activeRound && (
          <JsonView round={store.activeRound} />
        )}
        {section === "json" && !store.activeRound && (
          <div className="flex items-center justify-center h-full">
            <p className="text-xs text-text-muted">
              Select a round first to view its JSON.
            </p>
          </div>
        )}

        {/* Preview */}
        {section === "preview" && store.activeRound && (
          <PreviewView round={store.activeRound} onBack={() => setSection("builder")} />
        )}

        {/* Downloads stub */}
        {section === "downloads" && (
          <div className="flex items-center justify-center h-full">
            <p className="text-xs text-text-muted">
              Download history will appear here.
            </p>
          </div>
        )}

        {/* Settings */}
        {section === "settings" && <SettingsPage />}
      </main>
    </div>
  );
}

/* ── Right panel (round settings / proposal editor) ──────────── */

function RightPanel({
  round,
  activeProposal,
  onUpdateRoundName,
  onUpdateRoundSettings,
  onUpdateProposal,
  onDeleteProposal,
}: {
  round: VotingRound;
  activeProposal: Proposal | null;
  onUpdateRoundName: (name: string) => void;
  onUpdateRoundSettings: (patch: Partial<RoundSettings>) => void;
  onUpdateProposal: (patch: Partial<Proposal>) => void;
  onDeleteProposal: () => void;
}) {
  const [tab, setTab] = useState<"proposal" | "round">(
    activeProposal ? "proposal" : "round"
  );

  // If a proposal gets selected, switch to proposal tab
  const effectiveTab = activeProposal ? tab : "round";

  return (
    <div className="flex-1 bg-surface-0 overflow-hidden flex flex-col">
      {/* Tab bar */}
      <div className="flex border-b border-border-subtle bg-surface-0 shrink-0">
        <button
          onClick={() => setTab("round")}
          className={`flex items-center gap-1.5 px-4 py-2 text-[11px] transition-colors cursor-pointer border-b-2 ${
            effectiveTab === "round"
              ? "border-accent text-accent-glow"
              : "border-transparent text-text-muted hover:text-text-secondary"
          }`}
        >
          <Settings2 size={12} />
          Round
        </button>
        <button
          onClick={() => setTab("proposal")}
          disabled={!activeProposal}
          className={`flex items-center gap-1.5 px-4 py-2 text-[11px] transition-colors cursor-pointer border-b-2 disabled:opacity-30 disabled:cursor-not-allowed ${
            effectiveTab === "proposal"
              ? "border-accent text-accent-glow"
              : "border-transparent text-text-muted hover:text-text-secondary"
          }`}
        >
          Proposal
        </button>
      </div>

      {/* Panel content */}
      <div className="flex-1 overflow-hidden">
        {effectiveTab === "round" && (
          <RoundEditor
            round={round}
            onUpdateName={onUpdateRoundName}
            onUpdateSettings={onUpdateRoundSettings}
          />
        )}
        {effectiveTab === "proposal" && activeProposal && (
          <ProposalEditor
            key={activeProposal.id}
            proposal={activeProposal}
            onUpdate={onUpdateProposal}
            onDelete={onDeleteProposal}
          />
        )}
      </div>
    </div>
  );
}

/* ── About page ──────────────────────────────────────────────── */

function AboutPage({
  onCreateRound,
  onOpenSample,
}: {
  onCreateRound: () => void;
  onOpenSample?: () => void;
}) {
  return (
    <div className="flex-1 overflow-y-auto">
      <div className="max-w-xl mx-auto px-6 py-12">
        {/* Hero */}
        <div className="flex items-center gap-3 mb-6">
          <div className="w-10 h-10 rounded-xl bg-accent/15 flex items-center justify-center">
            <Shield size={22} className="text-accent" />
          </div>
          <div>
            <h1 className="text-lg font-bold text-text-primary">
              Shielded Vote Creator
            </h1>
            <p className="text-[11px] text-text-muted">
              Build private voting rounds for the shielded vote chain
            </p>
          </div>
        </div>

        {/* Description */}
        <div className="bg-surface-1 border border-border-subtle rounded-xl p-5 mb-6">
          <p className="text-xs text-text-secondary leading-relaxed">
            This tool lets you build proposals for new shielded voting rounds.
            Define your proposals, configure options, preview the ballot, and
            export the round as JSON. Eventually, you'll be able to submit
            rounds directly to the vote chain from here.
          </p>
        </div>

        {/* Getting started */}
        <h2 className="text-xs font-semibold text-text-primary mb-3">
          Getting started
        </h2>
        <div className="space-y-3 mb-8">
          {onOpenSample && (
            <button
              onClick={onOpenSample}
              className="w-full flex items-start gap-3 bg-surface-1 border border-border-subtle hover:border-accent/30 rounded-xl p-4 text-left transition-colors cursor-pointer group"
            >
              <div className="w-8 h-8 rounded-lg bg-accent/10 flex items-center justify-center shrink-0 mt-0.5">
                <FileText size={16} className="text-accent" />
              </div>
              <div>
                <p className="text-xs font-semibold text-text-primary group-hover:text-accent-glow transition-colors">
                  Open the sample round
                </p>
                <p className="text-[11px] text-text-muted mt-0.5">
                  Explore a pre-loaded draft based on NU7 Sentiment Polling
                  with 11 proposals to see how the builder works.
                </p>
              </div>
            </button>
          )}

          <button
            onClick={onCreateRound}
            className="w-full flex items-start gap-3 bg-surface-1 border border-border-subtle hover:border-accent/30 rounded-xl p-4 text-left transition-colors cursor-pointer group"
          >
            <div className="w-8 h-8 rounded-lg bg-accent/10 flex items-center justify-center shrink-0 mt-0.5">
              <Plus size={16} className="text-accent" />
            </div>
            <div>
              <p className="text-xs font-semibold text-text-primary group-hover:text-accent-glow transition-colors">
                Create a new round
              </p>
              <p className="text-[11px] text-text-muted mt-0.5">
                Start from scratch. Add proposals, configure options, and
                export when you're ready.
              </p>
            </div>
          </button>
        </div>

        {/* How it works */}
        <h2 className="text-xs font-semibold text-text-primary mb-3">
          How it works
        </h2>
        <div className="bg-surface-1 border border-border-subtle rounded-xl p-5 space-y-3 mb-8">
          <Step n={1} text="Create a voting round and add one or more proposals with Support/Oppose or multi-choice options." />
          <Step n={2} text="Preview the ballot as voters will see it, and validate for completeness." />
          <Step n={3} text="Export the round as JSON or submit it to the shielded vote chain (coming soon)." />
        </div>

        {/* Footer note */}
        <p className="text-[10px] text-text-muted text-center">
          All data is stored locally in your browser. Nothing is sent to a
          server until you choose to publish.
        </p>
      </div>
    </div>
  );
}

function Step({ n, text }: { n: number; text: string }) {
  return (
    <div className="flex items-start gap-3">
      <span className="text-[10px] font-bold text-accent bg-accent/10 rounded-full w-5 h-5 flex items-center justify-center shrink-0 mt-0.5">
        {n}
      </span>
      <p className="text-[11px] text-text-secondary leading-relaxed">{text}</p>
    </div>
  );
}

/* ── Settings page ───────────────────────────────────────────── */

function SettingsPage() {
  return (
    <div className="flex-1 overflow-y-auto">
      <div className="max-w-xl mx-auto px-6 py-12">
        <div className="flex items-center gap-3 mb-6">
          <div className="w-10 h-10 rounded-xl bg-surface-3 flex items-center justify-center">
            <Settings size={22} className="text-text-secondary" />
          </div>
          <div>
            <h1 className="text-lg font-bold text-text-primary">Settings</h1>
            <p className="text-[11px] text-text-muted">
              Configuration for chain connectivity and defaults
            </p>
          </div>
        </div>

        {/* Chain configuration */}
        <h2 className="text-xs font-semibold text-text-primary mb-3">
          Chain configuration
        </h2>
        <div className="bg-surface-1 border border-border-subtle rounded-xl p-5 space-y-4 mb-6">
          <SettingsStubRow
            icon={<Server size={14} />}
            label="RPC endpoint"
            value="Not configured"
          />
          <SettingsStubRow
            icon={<Link2 size={14} />}
            label="Chain ID"
            value="Not configured"
          />
          <SettingsStubRow
            icon={<Shield size={14} />}
            label="Shielded pool"
            value="Not configured"
          />
        </div>

        <p className="text-[10px] text-text-muted">
          Chain configuration will be required to submit rounds directly to the
          vote chain. For now, you can export rounds as JSON.
        </p>
      </div>
    </div>
  );
}

function SettingsStubRow({
  icon,
  label,
  value,
}: {
  icon: React.ReactNode;
  label: string;
  value: string;
}) {
  return (
    <div className="flex items-center justify-between">
      <div className="flex items-center gap-2.5">
        <span className="text-text-muted">{icon}</span>
        <span className="text-xs text-text-secondary">{label}</span>
      </div>
      <span className="text-[11px] text-text-muted italic">{value}</span>
    </div>
  );
}

/* ── Preview page ────────────────────────────────────────────── */

function PreviewView({ round, onBack }: { round: VotingRound; onBack: () => void }) {
  return (
    <div className="flex flex-col h-full">
      <div className="px-6 py-4 border-b border-border flex items-center justify-between">
        <div>
          <h2 className="text-sm font-semibold text-text-primary">
            Preview: {round.name}
          </h2>
          <p className="text-[11px] text-text-muted mt-0.5">
            Read-only view as a voter would see it
          </p>
        </div>
        <button
          onClick={onBack}
          className="px-3 py-1.5 bg-surface-3 hover:bg-surface-2 text-text-secondary rounded-md text-[11px] transition-colors cursor-pointer"
        >
          Back to builder
        </button>
      </div>
      <div className="flex-1 overflow-y-auto p-6 max-w-2xl">
        {round.proposals.length === 0 ? (
          <p className="text-xs text-text-muted italic">No proposals yet.</p>
        ) : (
          <div className="space-y-6">
            {round.proposals.map((p, i) => (
              <div
                key={p.id}
                className="bg-surface-1 border border-border-subtle rounded-xl p-5"
              >
                <div className="flex items-center gap-2 mb-2">
                  <span className="text-[10px] font-bold text-text-muted bg-surface-2 rounded px-1.5 py-0.5">
                    {String(i + 1).padStart(2, "0")}
                  </span>
                  <h3 className="text-xs font-semibold text-text-primary">
                    {p.title || "Untitled"}
                  </h3>
                </div>
                {p.description && (
                  <p className="text-[11px] text-text-secondary mb-3 whitespace-pre-wrap">
                    {p.description}
                  </p>
                )}
                <div className="space-y-1.5">
                  {p.options.map((opt) => (
                    <div
                      key={opt.id}
                      className="flex items-center gap-2 px-3 py-2 bg-surface-2 rounded-lg border border-border-subtle hover:border-accent/30 transition-colors cursor-pointer"
                    >
                      <div className="w-3 h-3 rounded-full border-2 border-text-muted" />
                      <span className="text-xs text-text-primary">
                        {opt.label}
                      </span>
                    </div>
                  ))}
                  {p.allowAbstain && (
                    <div className="flex items-center gap-2 px-3 py-2 bg-surface-2 rounded-lg border border-border-subtle hover:border-accent/30 transition-colors cursor-pointer">
                      <div className="w-3 h-3 rounded-full border-2 border-text-muted" />
                      <span className="text-xs text-text-muted italic">
                        Abstain
                      </span>
                    </div>
                  )}
                </div>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

export default App;
