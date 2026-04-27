import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";

interface FormatInstance {
  format: string;
  path: string;
  version: string | null;
}

interface ManualEntry {
  id: number;
  source: string;
  path_or_url: string;
}

interface PluginDetailData {
  id: number;
  name: string;
  vendor: string | null;
  category: string | null;
  instances: FormatInstance[];
  note: string;
  manuals: ManualEntry[];
}

const FORMAT_COLORS: Record<string, string> = {
  VST3: "text-blue-400",
  AU:   "text-green-400",
  VST2: "text-yellow-400",
  CLAP: "text-purple-400",
};

interface PluginDetailProps {
  name: string;
  onBack: () => void;
}

export default function PluginDetail({ name, onBack }: PluginDetailProps) {
  const [detail, setDetail] = useState<PluginDetailData | null>(null);
  const [loading, setLoading] = useState(true);
  const [note, setNote] = useState("");
  const [noteDirty, setNoteDirty] = useState(false);
  const [noteSaved, setNoteSaved] = useState(false);
  const [newManual, setNewManual] = useState("");

  useEffect(() => {
    setLoading(true);
    invoke<PluginDetailData | null>("get_plugin_detail", { name })
      .then((d) => {
        setDetail(d);
        setNote(d?.note ?? "");
        setNoteDirty(false);
      })
      .finally(() => setLoading(false));
  }, [name]);

  async function saveNote() {
    if (!detail) return;
    await invoke("save_plugin_note", { pluginId: detail.id, body: note });
    setNoteDirty(false);
    setNoteSaved(true);
    setTimeout(() => setNoteSaved(false), 2000);
  }

  async function addManual() {
    if (!detail || !newManual.trim()) return;
    const val = newManual.trim();
    const source = val.startsWith("http://") || val.startsWith("https://") ? "url" : "local";
    await invoke("save_plugin_manual", { pluginId: detail.id, source, pathOrUrl: val });
    const updated = await invoke<PluginDetailData | null>("get_plugin_detail", { name });
    setDetail(updated);
    setNewManual("");
  }

  async function deleteManual(id: number) {
    await invoke("delete_plugin_manual", { manualId: id });
    const updated = await invoke<PluginDetailData | null>("get_plugin_detail", { name });
    setDetail(updated);
  }

  if (loading) {
    return (
      <div className="flex items-center justify-center h-full text-zinc-600 text-xs">
        Loading…
      </div>
    );
  }

  if (!detail) {
    return (
      <div className="flex flex-col h-full">
        <BackNav onBack={onBack} />
        <div className="flex items-center justify-center flex-1 text-zinc-600 text-xs">
          Plugin not found
        </div>
      </div>
    );
  }

  return (
    <div className="flex flex-col h-full">
      {/* Nav bar */}
      <div className="flex items-center gap-3 px-4 py-3 border-b border-zinc-800 shrink-0">
        <button
          onClick={onBack}
          className="text-xs text-zinc-500 hover:text-zinc-300 transition-colors"
        >
          ← All Plugins
        </button>
        <span className="text-zinc-700">/</span>
        <span className="font-bold text-sm truncate">{detail.name}</span>
        {detail.vendor && (
          <span className="ml-auto text-xs text-zinc-500 bg-zinc-900 border border-zinc-700 px-2 py-0.5 rounded shrink-0">
            {detail.vendor}
          </span>
        )}
      </div>

      {/* Scrollable body */}
      <div className="flex-1 overflow-auto px-4 py-5 space-y-7 max-w-3xl">

        {/* Installed formats */}
        <section>
          <SectionLabel>Installed Formats</SectionLabel>
          <div className="space-y-1.5">
            {detail.instances.map((inst, i) => (
              <div key={i} className="flex items-baseline gap-3 text-xs">
                <span className={`w-10 shrink-0 font-mono font-medium ${FORMAT_COLORS[inst.format] ?? "text-zinc-400"}`}>
                  {inst.format}
                </span>
                {inst.version ? (
                  <span className="text-zinc-600 w-12 shrink-0">{inst.version}</span>
                ) : (
                  <span className="w-12 shrink-0" />
                )}
                <span className="text-zinc-400 font-mono truncate">{inst.path}</span>
              </div>
            ))}
          </div>
        </section>

        {/* Meta row */}
        <section className="flex gap-8">
          {detail.category && (
            <div>
              <SectionLabel>Category</SectionLabel>
              <p className="text-sm mt-1">{detail.category}</p>
            </div>
          )}
          <div>
            <SectionLabel>License</SectionLabel>
            <p className="text-sm mt-1 text-zinc-600">Not detected</p>
          </div>
        </section>

        {/* Manual */}
        <section>
          <SectionLabel>Manual</SectionLabel>
          {detail.manuals.length > 0 && (
            <div className="space-y-1 mb-2">
              {detail.manuals.map((m) => (
                <div key={m.id} className="flex items-center gap-2 text-xs">
                  <span className="text-zinc-500 shrink-0">{m.source === "url" ? "↗" : "↗"}</span>
                  <span className="text-zinc-300 font-mono truncate flex-1">{m.path_or_url}</span>
                  <button
                    onClick={() => deleteManual(m.id)}
                    className="text-zinc-600 hover:text-red-400 transition-colors px-1 shrink-0"
                    aria-label="Remove manual"
                  >
                    ×
                  </button>
                </div>
              ))}
            </div>
          )}
          <div className="flex gap-2 mt-1">
            <input
              type="text"
              placeholder="Paste URL or file path…"
              value={newManual}
              onChange={(e) => setNewManual(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && addManual()}
              className="flex-1 bg-zinc-900 border border-zinc-700 rounded px-2 py-1 text-xs focus:outline-none focus:border-zinc-500"
            />
            <button
              onClick={addManual}
              disabled={!newManual.trim()}
              className="bg-zinc-800 hover:bg-zinc-700 disabled:opacity-40 border border-zinc-700 rounded px-3 py-1 text-xs transition-colors"
            >
              Add
            </button>
          </div>
        </section>

        {/* Notes */}
        <section>
          <SectionLabel>Notes</SectionLabel>
          <textarea
            value={note}
            onChange={(e) => { setNote(e.target.value); setNoteDirty(true); setNoteSaved(false); }}
            placeholder="Your notes about this plugin…"
            rows={5}
            className="w-full mt-1 bg-zinc-900 border border-zinc-700 rounded px-3 py-2 text-xs focus:outline-none focus:border-zinc-500 resize-none font-mono"
          />
          <div className="flex items-center gap-3 mt-1.5">
            <button
              onClick={saveNote}
              disabled={!noteDirty}
              className="bg-zinc-800 hover:bg-zinc-700 disabled:opacity-40 border border-zinc-700 rounded px-3 py-1 text-xs transition-colors"
            >
              Save Note
            </button>
            {noteSaved && <span className="text-xs text-green-500">Saved</span>}
          </div>
        </section>

      </div>
    </div>
  );
}

function BackNav({ onBack }: { onBack: () => void }) {
  return (
    <div className="px-4 py-3 border-b border-zinc-800">
      <button onClick={onBack} className="text-xs text-zinc-500 hover:text-zinc-300 transition-colors">
        ← All Plugins
      </button>
    </div>
  );
}

function SectionLabel({ children }: { children: React.ReactNode }) {
  return (
    <p className="text-xs text-zinc-500 uppercase tracking-widest mb-2">{children}</p>
  );
}
