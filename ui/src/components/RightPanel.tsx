import { useState, useEffect } from "react"
import { invoke } from "@tauri-apps/api/core"

interface ChainSlotRow {
  id: number
  plugin_id: number | null
  plugin_identity: string
  position: number
  bypass: boolean
  wet: number
  preset_name: string | null
  opaque_state: string | null
}

interface ChainDetail {
  id: number
  sync_id: string
  name: string
  daw: string
  source_track: string | null
  notes: string | null
  tags: string | null
  created_at: string
  slots: ChainSlotRow[]
}

interface Props {
  chainId: number | null
  onDeleteChain: (id: number) => void
}

const FORMAT_COLORS: Record<string, string> = {
  VST3: "text-blue-400",
  AU: "text-green-400",
  VST2: "text-yellow-400",
  CLAP: "text-purple-400",
}

function parseIdentity(raw: string): { name: string; format: string | null } {
  try {
    const obj = JSON.parse(raw) as Record<string, unknown>
    return {
      name: typeof obj.name === "string" ? obj.name : "Unknown",
      format: typeof obj.format === "string" ? obj.format : null,
    }
  } catch {
    return { name: "Unknown", format: null }
  }
}

function estimateBytes(b64: string): number {
  return Math.floor((b64.length * 3) / 4)
}

export function RightPanel({ chainId, onDeleteChain }: Props) {
  const [detail, setDetail] = useState<ChainDetail | null>(null)
  const [loading, setLoading] = useState(false)
  const [exporting, setExporting] = useState(false)
  const [exportedPath, setExportedPath] = useState<string | null>(null)
  const [exportError, setExportError] = useState<string | null>(null)

  useEffect(() => {
    if (chainId === null) {
      setDetail(null)
      return
    }
    setLoading(true)
    setExportedPath(null)
    setExportError(null)
    invoke<ChainDetail | null>("get_chain", { chainId })
      .then(setDetail)
      .catch(() => setDetail(null))
      .finally(() => setLoading(false))
  }, [chainId])

  async function handleExport() {
    if (!detail) return
    setExporting(true)
    setExportedPath(null)
    setExportError(null)
    try {
      const path = await invoke<string>("export_chain", { chainId: detail.id })
      setExportedPath(path)
    } catch (e) {
      setExportError(String(e))
    } finally {
      setExporting(false)
    }
  }

  function openFolder() {
    if (!exportedPath) return
    const dir = exportedPath.replace(/[/\\][^/\\]+$/, "")
    invoke("open_path", { path: dir }).catch(() => {})
  }

  if (!chainId) {
    return (
      <div className="flex items-center justify-center h-full text-zinc-700 text-xs">
        Select a chain
      </div>
    )
  }

  if (loading) {
    return (
      <div className="flex items-center justify-center h-full text-zinc-700 text-xs">
        Loading…
      </div>
    )
  }

  if (!detail) {
    return (
      <div className="flex items-center justify-center h-full text-zinc-700 text-xs">
        Chain not found
      </div>
    )
  }

  const tags = detail.tags?.split(",").map(t => t.trim()).filter(Boolean) ?? []
  const exportedFilename = exportedPath?.split(/[/\\]/).pop() ?? ""

  return (
    <div className="flex flex-col h-full overflow-hidden">
      {/* Chain header */}
      <div className="px-4 py-3 border-b border-zinc-800 shrink-0">
        <div className="flex items-start justify-between gap-2">
          <div className="flex-1 min-w-0">
            <div className="font-semibold text-sm truncate">{detail.name}</div>
            <div className="flex items-center gap-2 mt-1 flex-wrap">
              <span className="text-xs text-zinc-500 bg-zinc-900 border border-zinc-800 px-1.5 py-0.5 rounded shrink-0">
                {detail.daw}
              </span>
              {detail.source_track && (
                <span className="text-xs text-zinc-600 truncate">{detail.source_track}</span>
              )}
            </div>
            {tags.length > 0 && (
              <div className="flex flex-wrap gap-1.5 mt-1.5">
                {tags.map(tag => (
                  <span key={tag} className="text-xs text-zinc-600">#{tag}</span>
                ))}
              </div>
            )}
          </div>
          <div className="flex flex-col items-end gap-1 shrink-0">
            <button
              onClick={handleExport}
              disabled={exporting}
              className="text-xs bg-zinc-800 hover:bg-zinc-700 border border-zinc-700 rounded px-2.5 py-1 transition-colors disabled:opacity-40"
            >
              {exporting ? "…" : "Export"}
            </button>
            <button
              onClick={() => onDeleteChain(detail.id)}
              className="text-xs text-zinc-600 hover:text-red-400 transition-colors"
            >
              Delete
            </button>
          </div>
        </div>
        {exportedPath && (
          <div className="flex items-center gap-2 mt-2">
            <span className="text-xs text-green-500 font-mono truncate">{exportedFilename}</span>
            <button
              onClick={openFolder}
              className="text-xs text-zinc-500 hover:text-zinc-300 shrink-0 transition-colors"
            >
              Open folder
            </button>
          </div>
        )}
        {exportError && (
          <div className="text-xs text-red-400 mt-1.5">{exportError}</div>
        )}
      </div>

      {/* Slots */}
      <div className="flex-1 overflow-auto px-4 py-3 space-y-2">
        <div className="text-xs text-zinc-600 uppercase tracking-widest mb-3">
          Signal chain · {detail.slots.length} slot{detail.slots.length !== 1 ? "s" : ""}
        </div>
        {detail.slots.length === 0 ? (
          <div className="text-xs text-zinc-700">No slots recorded</div>
        ) : (
          detail.slots.map((slot, i) => {
            const { name, format } = parseIdentity(slot.plugin_identity)
            const hasState = Boolean(slot.opaque_state)
            const stateBytes = hasState ? estimateBytes(slot.opaque_state!) : 0

            return (
              <div key={i} className="bg-zinc-900 border border-zinc-800 rounded p-3 space-y-1.5">
                <div className="flex items-center gap-2">
                  <span className="text-xs text-zinc-700 tabular-nums w-4 shrink-0">
                    {slot.position + 1}
                  </span>
                  <span className={`text-xs font-mono w-9 shrink-0 ${FORMAT_COLORS[format ?? ""] ?? "text-zinc-600"}`}>
                    {format ?? "?"}
                  </span>
                  <span className={`text-sm font-medium flex-1 truncate ${slot.bypass ? "text-zinc-600 line-through" : ""}`}>
                    {name}
                  </span>
                  {slot.bypass && (
                    <span className="text-xs text-zinc-700 shrink-0">bypassed</span>
                  )}
                </div>
                <div className="flex items-center gap-4 text-xs text-zinc-500 pl-[52px]">
                  <span>
                    wet{" "}
                    <span className="text-zinc-300 tabular-nums">{(slot.wet * 100).toFixed(0)}%</span>
                  </span>
                  {slot.preset_name && (
                    <span className="truncate">
                      preset <span className="text-zinc-300">{slot.preset_name}</span>
                    </span>
                  )}
                </div>
                <div className="pl-[52px]">
                  {hasState ? (
                    <div className="text-xs text-zinc-700 bg-zinc-950 border border-zinc-900 rounded px-2 py-1.5 leading-relaxed">
                      <span className="text-zinc-500">Opaque state</span>
                      {" · "}
                      <span className="tabular-nums">~{stateBytes.toLocaleString()} B</span>
                      {" · "}
                      <span>decoded view in Phase 4</span>
                    </div>
                  ) : (
                    <div className="text-xs text-zinc-700">No state captured</div>
                  )}
                </div>
              </div>
            )
          })
        )}

        {/* Timbral placeholder */}
        <div className="pt-4 mt-2 border-t border-zinc-900 space-y-1">
          <div className="text-xs text-zinc-600 uppercase tracking-widest">Timbral Profile</div>
          <div className="text-xs text-zinc-700 leading-relaxed">
            Spectral fingerprint and AI preset matching — Phase 4
          </div>
        </div>

        {detail.notes && (
          <div className="pt-4 border-t border-zinc-900 space-y-1">
            <div className="text-xs text-zinc-600 uppercase tracking-widest">Notes</div>
            <div className="text-xs text-zinc-400 leading-relaxed whitespace-pre-wrap">
              {detail.notes}
            </div>
          </div>
        )}
      </div>
    </div>
  )
}
