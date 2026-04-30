import { useState, useEffect } from "react"
import { invoke } from "@tauri-apps/api/core"
import { cn } from "../lib/utils"
import type { Plugin } from "./PluginFilters"

interface ChainRow {
  id: number
  sync_id: string
  name: string
  daw: string
  tags: string | null
  source_track: string | null
  created_at: string
}

interface Props {
  selectedPlugin: Plugin | null
  selectedChainId: number | null
  onSelectChain: (chainId: number) => void
}

type Tab = "chains" | "presets"

export function CenterPanel({ selectedPlugin, selectedChainId, onSelectChain }: Props) {
  const [tab, setTab] = useState<Tab>("chains")
  const [chains, setChains] = useState<ChainRow[]>([])
  const [loading, setLoading] = useState(false)

  useEffect(() => {
    if (!selectedPlugin) {
      setChains([])
      return
    }
    setLoading(true)
    invoke<ChainRow[]>("list_chains_for_plugin", { pluginName: selectedPlugin.name })
      .then(setChains)
      .catch(() => setChains([]))
      .finally(() => setLoading(false))
  }, [selectedPlugin])

  if (!selectedPlugin) {
    return (
      <div className="flex items-center justify-center h-full text-zinc-700 text-xs">
        Select a plugin
      </div>
    )
  }

  return (
    <div className="flex flex-col h-full">
      {/* Tab bar */}
      <div className="flex items-center border-b border-zinc-800 shrink-0">
        <div className="px-3 py-2 text-xs text-zinc-400 font-medium truncate max-w-[180px] shrink-0">
          {selectedPlugin.name}
        </div>
        <div className="flex-1" />
        {(["chains", "presets"] as Tab[]).map(t => (
          <button
            key={t}
            onClick={() => setTab(t)}
            className={cn(
              "px-4 py-2 text-xs capitalize border-b-2 -mb-px transition-colors shrink-0",
              tab === t
                ? "border-zinc-400 text-zinc-200"
                : "border-transparent text-zinc-600 hover:text-zinc-400"
            )}
          >
            {t}
            {t === "chains" && chains.length > 0 && (
              <span className="ml-1.5 text-zinc-600">{chains.length}</span>
            )}
          </button>
        ))}
      </div>

      {tab === "chains" ? (
        loading ? (
          <div className="flex items-center justify-center flex-1 text-zinc-700 text-xs">
            Loading…
          </div>
        ) : chains.length === 0 ? (
          <div className="flex items-center justify-center flex-1 text-zinc-700 text-xs text-center px-8 leading-relaxed">
            No saved chains contain {selectedPlugin.name}
          </div>
        ) : (
          <div className="flex-1 overflow-auto">
            {chains.map(chain => (
              <ChainItem
                key={chain.id}
                chain={chain}
                selected={chain.id === selectedChainId}
                onSelect={() => onSelectChain(chain.id)}
              />
            ))}
          </div>
        )
      ) : (
        <div className="flex flex-col items-center justify-center flex-1 gap-2 text-center px-8">
          <span className="text-zinc-500 text-xs font-medium">Preset Browser</span>
          <span className="text-zinc-700 text-xs leading-relaxed">
            Timbral tagging and AI preset matching — Phase 4
          </span>
        </div>
      )}
    </div>
  )
}

function ChainItem({
  chain,
  selected,
  onSelect,
}: {
  chain: ChainRow
  selected: boolean
  onSelect: () => void
}) {
  const tags = chain.tags?.split(",").map(t => t.trim()).filter(Boolean) ?? []
  const date = chain.created_at.slice(0, 10)

  return (
    <div
      onClick={onSelect}
      className={cn(
        "px-3 py-2.5 border-b border-zinc-900 cursor-pointer",
        selected ? "bg-zinc-800" : "hover:bg-zinc-900/50"
      )}
    >
      <div className="flex items-center gap-2">
        <span className="text-sm font-medium truncate flex-1">{chain.name}</span>
        <span className="text-xs text-zinc-700 shrink-0 tabular-nums">{date}</span>
      </div>
      <div className="flex items-center gap-2 mt-1 flex-wrap">
        <span className="text-xs text-zinc-500 bg-zinc-900 border border-zinc-800 px-1.5 py-0.5 rounded shrink-0">
          {chain.daw}
        </span>
        {chain.source_track && (
          <span className="text-xs text-zinc-600 truncate">{chain.source_track}</span>
        )}
        {tags.map(tag => (
          <span key={tag} className="text-xs text-zinc-600">#{tag}</span>
        ))}
      </div>
    </div>
  )
}
