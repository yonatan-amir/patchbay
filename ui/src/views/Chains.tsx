import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// ── Types ──────────────────────────────────────────────────────────────────────

interface LiveSlot {
  position: number;
  name: string;
  vendor: string | null;
  format: string | null;
  bypass: boolean;
  wet: number;
  preset_name: string | null;
  plugin_identity: Record<string, unknown>;
  opaque_state: string | null;
}

interface LiveTrack {
  name: string;
  kind: string;
  slots: LiveSlot[];
}

interface LiveProject {
  path: string;
  daw: string;
  tracks: LiveTrack[];
}

interface ChainRow {
  id: number;
  sync_id: string;
  name: string;
  daw: string;
  tags: string | null;
  source_track: string | null;
  created_at: string;
}

// ── Icons ──────────────────────────────────────────────────────────────────────

const KIND_ICON: Record<string, string> = {
  audio: "≈",
  instrument: "♪",
  bus: "⊕",
  group: "G",
  master: "M",
};

const FORMAT_COLORS: Record<string, string> = {
  VST3: "text-blue-400",
  AU: "text-green-400",
  VST2: "text-yellow-400",
  CLAP: "text-purple-400",
};

// ── Root view ─────────────────────────────────────────────────────────────────

interface ChainsProps {
  onBack: () => void;
}

export default function Chains({ onBack }: ChainsProps) {
  const [liveProject, setLiveProject] = useState<LiveProject | null>(null);
  const [chains, setChains] = useState<ChainRow[]>([]);
  const [pendingTrack, setPendingTrack] = useState<LiveTrack | null>(null);
  const [watcherError, setWatcherError] = useState<string | null>(null);

  useEffect(() => {
    invoke<ChainRow[]>("list_chains").then(setChains).catch(() => {});
    invoke<LiveProject | null>("get_live_project")
      .then(lp => { if (lp) setLiveProject(lp); })
      .catch(() => {});
  }, []);

  useEffect(() => {
    let offChanged: (() => void) | undefined;
    let offClosed: (() => void) | undefined;
    let offError: (() => void) | undefined;

    listen<LiveProject>("project-changed", e => {
      setLiveProject(e.payload);
      setWatcherError(null);
    }).then(fn => { offChanged = fn; });

    listen<unknown>("project-closed", () => setLiveProject(null))
      .then(fn => { offClosed = fn; });

    listen<{ path: string; error: string }>("project-error", e => {
      setWatcherError(e.payload.error);
    }).then(fn => { offError = fn; });

    return () => {
      offChanged?.();
      offClosed?.();
      offError?.();
    };
  }, []);

  async function reloadChains() {
    const updated = await invoke<ChainRow[]>("list_chains");
    setChains(updated);
  }

  async function deleteChain(id: number) {
    await invoke("delete_chain", { chainId: id });
    setChains(prev => prev.filter(c => c.id !== id));
  }

  return (
    <div className="flex flex-col h-full">
      {/* Nav bar */}
      <div className="flex items-center gap-3 px-4 py-3 border-b border-zinc-800 shrink-0">
        <button
          onClick={onBack}
          className="text-xs text-zinc-500 hover:text-zinc-300 transition-colors"
        >
          ← Plugins
        </button>
        <span className="text-zinc-700">/</span>
        <span className="font-bold text-sm">Chains</span>
      </div>

      {/* Two-column layout */}
      <div className="flex flex-1 overflow-hidden">
        {/* Left: live project */}
        <div className="w-1/2 flex flex-col border-r border-zinc-800 overflow-hidden">
          <div className="px-4 py-2 border-b border-zinc-800 shrink-0">
            <span className="text-xs text-zinc-500 uppercase tracking-widest">Live Project</span>
            {liveProject && (
              <span className="ml-2 text-xs text-zinc-600 font-medium">{liveProject.daw}</span>
            )}
          </div>
          {watcherError && (
            <div className="px-4 py-2 text-xs text-red-400 border-b border-zinc-900 font-mono leading-snug">
              {watcherError}
            </div>
          )}
          {!liveProject ? (
            <div className="flex items-center justify-center flex-1 text-zinc-600 text-xs p-8 text-center leading-relaxed">
              Open a project in your DAW — tracks will appear here automatically
            </div>
          ) : (
            <LivePanel project={liveProject} onSave={setPendingTrack} />
          )}
        </div>

        {/* Right: saved chains */}
        <div className="w-1/2 flex flex-col overflow-hidden">
          <div className="px-4 py-2 border-b border-zinc-800 shrink-0 flex items-center gap-2">
            <span className="text-xs text-zinc-500 uppercase tracking-widest">Saved Chains</span>
            {chains.length > 0 && (
              <span className="text-xs text-zinc-600">{chains.length}</span>
            )}
          </div>
          {chains.length === 0 ? (
            <div className="flex items-center justify-center flex-1 text-zinc-600 text-xs p-8 text-center leading-relaxed">
              Click &ldquo;Save chain&rdquo; on any track to add it here
            </div>
          ) : (
            <div className="flex-1 overflow-auto">
              {chains.map(chain => (
                <ChainCard key={chain.id} chain={chain} onDelete={deleteChain} />
              ))}
            </div>
          )}
        </div>
      </div>

      {/* Save modal */}
      {pendingTrack && (
        <SaveModal
          track={pendingTrack}
          daw={liveProject?.daw ?? ""}
          onSaved={() => { setPendingTrack(null); reloadChains(); }}
          onClose={() => setPendingTrack(null)}
        />
      )}
    </div>
  );
}

// ── Live project panel ────────────────────────────────────────────────────────

function LivePanel({
  project,
  onSave,
}: {
  project: LiveProject;
  onSave: (t: LiveTrack) => void;
}) {
  const stem = project.path.split(/[/\\]/).pop() ?? "";
  const withSlots = project.tracks.filter(t => t.slots.length > 0);
  const withoutSlots = project.tracks.filter(t => t.slots.length === 0);

  return (
    <div className="flex-1 overflow-auto">
      {stem && (
        <div className="px-4 py-1.5 border-b border-zinc-900 text-xs text-zinc-600 font-mono truncate">
          {stem}
        </div>
      )}
      {project.tracks.length === 0 ? (
        <div className="text-zinc-600 text-xs p-4">No tracks detected</div>
      ) : (
        <>
          {withSlots.map((track, i) => (
            <TrackRow key={i} track={track} onSave={() => onSave(track)} />
          ))}
          {withoutSlots.length > 0 && withSlots.length > 0 && (
            <div className="px-4 py-1.5 text-xs text-zinc-700 border-t border-zinc-900">
              {withoutSlots.length} track{withoutSlots.length !== 1 ? "s" : ""} with no plugins
            </div>
          )}
          {withSlots.length === 0 && (
            <div className="text-zinc-600 text-xs p-4">No tracks with plugins detected</div>
          )}
        </>
      )}
    </div>
  );
}

function TrackRow({ track, onSave }: { track: LiveTrack; onSave: () => void }) {
  const [hover, setHover] = useState(false);
  return (
    <div
      className={`px-4 py-2.5 border-b border-zinc-900 ${hover ? "bg-zinc-900/50" : ""}`}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
    >
      <div className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-1.5 min-w-0">
          <span className="text-zinc-600 text-xs shrink-0">{KIND_ICON[track.kind] ?? "·"}</span>
          <span className="text-sm font-medium truncate">{track.name}</span>
        </div>
        {hover && (
          <button
            onClick={e => { e.stopPropagation(); onSave(); }}
            className="text-xs bg-zinc-800 hover:bg-zinc-700 border border-zinc-700 rounded px-2 py-0.5 shrink-0 transition-colors"
          >
            Save chain
          </button>
        )}
      </div>
      <div className="flex flex-wrap gap-1 mt-1.5">
        {track.slots.map((s, i) => (
          <span
            key={i}
            className={`text-xs px-1.5 py-0.5 rounded border border-zinc-800 ${
              s.bypass ? "text-zinc-700" : (FORMAT_COLORS[s.format ?? ""] ?? "text-zinc-400")
            }`}
          >
            {s.name}
          </span>
        ))}
      </div>
    </div>
  );
}

// ── Saved chain card ──────────────────────────────────────────────────────────

function ChainCard({
  chain,
  onDelete,
}: {
  chain: ChainRow;
  onDelete: (id: number) => void;
}) {
  const [hover, setHover] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [exportedPath, setExportedPath] = useState<string | null>(null);
  const [exportError, setExportError] = useState<string | null>(null);

  const tags = chain.tags
    ?.split(",")
    .map(t => t.trim())
    .filter(Boolean) ?? [];
  const date = chain.created_at.slice(0, 10);

  async function handleExport() {
    setExporting(true);
    setExportedPath(null);
    setExportError(null);
    try {
      const path = await invoke<string>("export_chain", { chainId: chain.id });
      setExportedPath(path);
    } catch (e) {
      setExportError(String(e));
    } finally {
      setExporting(false);
    }
  }

  function openFolder() {
    if (!exportedPath) return;
    const dir = exportedPath.replace(/[/\\][^/\\]+$/, "");
    invoke("open_path", { path: dir }).catch(() => {});
  }

  const exportLabel = `Export for ${chain.daw}`;
  const exportedFilename = exportedPath?.split(/[/\\]/).pop() ?? "";

  return (
    <div
      className={`px-4 py-3 border-b border-zinc-900 ${hover ? "bg-zinc-900/30" : ""}`}
      onMouseEnter={() => setHover(true)}
      onMouseLeave={() => setHover(false)}
    >
      <div className="flex items-start justify-between gap-2">
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2 flex-wrap">
            <span className="font-medium text-sm">{chain.name}</span>
            <span className="text-xs text-zinc-500 bg-zinc-800 border border-zinc-700 px-1.5 py-0.5 rounded shrink-0">
              {chain.daw}
            </span>
          </div>
          {chain.source_track && (
            <div className="text-xs text-zinc-600 mt-0.5 truncate">
              {chain.source_track}
            </div>
          )}
          {tags.length > 0 && (
            <div className="flex flex-wrap gap-1 mt-1.5">
              {tags.map(tag => (
                <span
                  key={tag}
                  className="text-xs text-zinc-400 bg-zinc-900 border border-zinc-800 px-1.5 py-0.5 rounded"
                >
                  {tag}
                </span>
              ))}
            </div>
          )}
          {exportedPath && (
            <div className="flex items-center gap-2 mt-1.5">
              <span className="text-xs text-green-500 font-mono truncate max-w-[160px]">
                {exportedFilename}
              </span>
              <button
                onClick={openFolder}
                className="text-xs text-zinc-500 hover:text-zinc-300 transition-colors shrink-0"
              >
                Open folder
              </button>
            </div>
          )}
          {exportError && (
            <div className="text-xs text-red-400 mt-1 leading-snug">{exportError}</div>
          )}
        </div>
        <div className="flex items-center gap-2 shrink-0">
          <span className="text-xs text-zinc-700">{date}</span>
          {hover && (
            <>
              <button
                onClick={handleExport}
                disabled={exporting}
                className="text-xs bg-zinc-800 hover:bg-zinc-700 border border-zinc-700 rounded px-2 py-0.5 transition-colors disabled:opacity-40"
                title={exportLabel}
              >
                {exporting ? "…" : "Export"}
              </button>
              <button
                onClick={() => onDelete(chain.id)}
                className="text-xs text-zinc-600 hover:text-red-400 transition-colors"
              >
                ✕
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

// ── Save chain modal ──────────────────────────────────────────────────────────

interface SaveModalProps {
  track: LiveTrack;
  daw: string;
  onSaved: () => void;
  onClose: () => void;
}

function SaveModal({ track, daw, onSaved, onClose }: SaveModalProps) {
  const [name, setName] = useState(track.name);
  const [tags, setTags] = useState("");
  const [notes, setNotes] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function save() {
    setSaving(true);
    setError(null);
    try {
      await invoke("save_chain", {
        name: name.trim() || track.name,
        daw,
        tags: tags.trim() || null,
        sourceTrack: track.name,
        notes: notes.trim() || null,
        slots: track.slots.map(s => ({
          position: s.position,
          bypass: s.bypass,
          wet: s.wet,
          presetName: s.preset_name ?? null,
          pluginIdentity: s.plugin_identity,
          opaqueState: s.opaque_state ?? null,
        })),
      });
      onSaved();
    } catch (e) {
      setError(String(e));
      setSaving(false);
    }
  }

  return (
    <div
      className="fixed inset-0 bg-black/60 flex items-center justify-center z-50"
      onClick={e => { if (e.target === e.currentTarget) onClose(); }}
    >
      <div className="bg-zinc-900 border border-zinc-700 rounded-lg w-full max-w-md mx-4 p-5 space-y-4">
        <div className="flex items-center justify-between">
          <span className="font-bold text-sm">Save Chain</span>
          <button
            onClick={onClose}
            className="text-zinc-500 hover:text-zinc-300 transition-colors"
          >
            ✕
          </button>
        </div>

        {/* Slot preview */}
        <div>
          <div className="text-xs text-zinc-600 uppercase tracking-widest mb-1.5">
            Slots ({track.slots.length})
          </div>
          {track.slots.length === 0 ? (
            <span className="text-xs text-zinc-600">No plugins in this track</span>
          ) : (
            <div className="flex flex-wrap gap-1">
              {track.slots.map((s, i) => (
                <span
                  key={i}
                  className={`text-xs px-1.5 py-0.5 rounded border border-zinc-700 ${
                    s.bypass ? "text-zinc-600" : (FORMAT_COLORS[s.format ?? ""] ?? "text-zinc-400")
                  }`}
                >
                  {s.name}
                </span>
              ))}
            </div>
          )}
        </div>

        {/* Fields */}
        <div className="space-y-3">
          <label className="block">
            <span className="text-xs text-zinc-500 uppercase tracking-widest block mb-1">
              Name
            </span>
            <input
              value={name}
              onChange={e => setName(e.target.value)}
              className="w-full bg-zinc-800 border border-zinc-700 rounded px-3 py-2 text-sm focus:outline-none focus:border-zinc-500"
            />
          </label>
          <label className="block">
            <span className="text-xs text-zinc-500 uppercase tracking-widest block mb-1">
              Tags
            </span>
            <input
              value={tags}
              onChange={e => setTags(e.target.value)}
              placeholder="mastering, drums, compression"
              className="w-full bg-zinc-800 border border-zinc-700 rounded px-3 py-2 text-sm focus:outline-none focus:border-zinc-500 placeholder:text-zinc-600"
            />
          </label>
          <label className="block">
            <span className="text-xs text-zinc-500 uppercase tracking-widest block mb-1">
              Notes
            </span>
            <textarea
              value={notes}
              onChange={e => setNotes(e.target.value)}
              rows={3}
              className="w-full bg-zinc-800 border border-zinc-700 rounded px-3 py-2 text-sm focus:outline-none focus:border-zinc-500 resize-none"
            />
          </label>
        </div>

        {error && <div className="text-xs text-red-400">{error}</div>}

        <div className="flex gap-2 justify-end">
          <button
            onClick={onClose}
            className="bg-zinc-800 hover:bg-zinc-700 border border-zinc-700 rounded px-4 py-2 text-xs transition-colors"
          >
            Cancel
          </button>
          <button
            onClick={save}
            disabled={saving}
            className="bg-zinc-100 hover:bg-white text-zinc-950 disabled:opacity-40 rounded px-4 py-2 text-xs font-medium transition-colors"
          >
            {saving ? "Saving…" : "Save Chain"}
          </button>
        </div>
      </div>
    </div>
  );
}
