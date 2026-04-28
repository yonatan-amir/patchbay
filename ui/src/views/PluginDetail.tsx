import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import ReactMarkdown from "react-markdown";

interface FormatInstance {
  format: string;
  path: string;
  version: string | null;
}

interface PluginDetailData {
  id: number;
  name: string;
  vendor: string | null;
  category: string | null;
  instances: FormatInstance[];
  note: string;
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
  const [noteMode, setNoteMode] = useState<"edit" | "preview">("edit");

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

        {/* Notes */}
        <section>
          <div className="flex items-center justify-between mb-2">
            <SectionLabel>Notes</SectionLabel>
            <div className="flex text-xs border border-zinc-700 rounded overflow-hidden">
              {(["edit", "preview"] as const).map((m) => (
                <button
                  key={m}
                  onClick={() => setNoteMode(m)}
                  className={`px-2.5 py-1 capitalize transition-colors ${
                    noteMode === m
                      ? "bg-zinc-700 text-white"
                      : "bg-transparent text-zinc-500 hover:text-zinc-300"
                  }`}
                >
                  {m}
                </button>
              ))}
            </div>
          </div>

          {noteMode === "edit" ? (
            <textarea
              value={note}
              onChange={(e) => { setNote(e.target.value); setNoteDirty(true); setNoteSaved(false); }}
              placeholder="Your notes about this plugin… (supports Markdown)"
              rows={6}
              className="w-full bg-zinc-900 border border-zinc-700 rounded px-3 py-2 text-xs focus:outline-none focus:border-zinc-500 resize-none font-mono"
            />
          ) : (
            <div className="w-full min-h-[7.5rem] bg-zinc-900 border border-zinc-700 rounded px-3 py-2 text-xs prose-note">
              {note.trim() ? (
                <ReactMarkdown>{note}</ReactMarkdown>
              ) : (
                <span className="text-zinc-600">Nothing here yet.</span>
              )}
            </div>
          )}

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
    <p className="text-xs text-zinc-500 uppercase tracking-widest">{children}</p>
  );
}
