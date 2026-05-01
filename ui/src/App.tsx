import { useState, useEffect, useRef } from "react"
import { invoke } from "@tauri-apps/api/core"
import Chains from "./views/Chains"
import { PluginFilters, type Filters, type Plugin } from "./components/PluginFilters"
import { PluginList } from "./components/PluginList"
import { CenterPanel } from "./components/CenterPanel"
import { RightPanel } from "./components/RightPanel"
import { SearchResults, type SearchHit } from "./components/SearchResults"

interface ScanResult {
  plugins_found: number
  plugins_skipped: number
  errors: string[]
}

interface ExportResult {
  plugin_count: number
  json_path: string
  html_path: string
}

const FORMAT_COLORS: Record<string, string> = {
  VST3: "text-blue-400",
  AU: "text-green-400",
  VST2: "text-yellow-400",
  CLAP: "text-purple-400",
}

type Mode = "browser" | "live"

export default function App() {
  const [mode, setMode] = useState<Mode>("browser")
  const [plugins, setPlugins] = useState<Plugin[]>([])
  const [scanning, setScanning] = useState(false)
  const [lastScan, setLastScan] = useState<ScanResult | null>(null)
  const [scanError, setScanError] = useState<string | null>(null)
  const [exporting, setExporting] = useState(false)
  const [exportResult, setExportResult] = useState<ExportResult | null>(null)
  const [exportError, setExportError] = useState<string | null>(null)

  const [filters, setFilters] = useState<Filters>({
    search: "",
    format: null,
    vendor: null,
    category: null,
  })
  const [selectedPlugin, setSelectedPlugin] = useState<Plugin | null>(null)
  const [selectedChainId, setSelectedChainId] = useState<number | null>(null)

  const [searchQuery, setSearchQuery] = useState("")
  const [searchHits, setSearchHits] = useState<SearchHit[]>([])
  const [searchOpen, setSearchOpen] = useState(false)
  const searchRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    invoke<Plugin[]>("list_plugins").then(setPlugins).catch(() => {})
  }, [])

  useEffect(() => {
    if (!searchQuery.trim()) {
      setSearchHits([])
      return
    }
    const timer = setTimeout(() => {
      invoke<SearchHit[]>("search_library", { query: searchQuery })
        .then(setSearchHits)
        .catch(() => setSearchHits([]))
    }, 200)
    return () => clearTimeout(timer)
  }, [searchQuery])

  useEffect(() => {
    function onPointerDown(e: PointerEvent) {
      if (searchRef.current && !searchRef.current.contains(e.target as Node)) {
        setSearchOpen(false)
      }
    }
    document.addEventListener("pointerdown", onPointerDown)
    return () => document.removeEventListener("pointerdown", onPointerDown)
  }, [])

  async function scan() {
    setScanning(true)
    setScanError(null)
    try {
      const result = await invoke<ScanResult>("scan_plugins")
      setLastScan(result)
      const updated = await invoke<Plugin[]>("list_plugins")
      setPlugins(updated)
    } catch (e) {
      setScanError(String(e))
    } finally {
      setScanning(false)
    }
  }

  async function exportDossier() {
    setExporting(true)
    setExportError(null)
    setExportResult(null)
    try {
      const result = await invoke<ExportResult>("export_library_dossier")
      setExportResult(result)
    } catch (e) {
      setExportError(String(e))
    } finally {
      setExporting(false)
    }
  }

  function openPath(path: string) {
    invoke("open_path", { path }).catch(() => {})
  }

  function handleSelectPlugin(p: Plugin) {
    setSelectedPlugin(p)
    setSelectedChainId(null)
  }

  function handleSearchSelectPlugin(name: string) {
    const match = plugins.find(p => p.name === name) ?? null
    setSelectedPlugin(match)
    setSelectedChainId(null)
    setSearchQuery("")
    setSearchOpen(false)
  }

  function handleSearchSelectChain(id: number) {
    setSelectedChainId(id)
    setSearchQuery("")
    setSearchOpen(false)
  }

  function handleDeleteChain(id: number) {
    if (selectedChainId === id) setSelectedChainId(null)
  }

  function patchFilters(patch: Partial<Filters>) {
    setFilters(prev => ({ ...prev, ...patch }))
  }

  const counts = plugins.reduce<Record<string, number>>((acc, p) => {
    acc[p.format] = (acc[p.format] ?? 0) + 1
    return acc
  }, {})

  if (mode === "live") {
    return (
      <div className="flex flex-col h-screen bg-zinc-950 text-zinc-100 font-mono text-sm">
        <Chains onBack={() => setMode("browser")} />
      </div>
    )
  }

  return (
    <div className="flex flex-col h-screen bg-zinc-950 text-zinc-100 font-mono text-sm">
      {/* Header */}
      <div className="flex items-center gap-4 px-4 py-2.5 border-b border-zinc-800 shrink-0">
        <span className="font-bold tracking-tight text-sm">Patchbay</span>

        <div className="flex text-xs border border-zinc-700 rounded overflow-hidden">
          <button className="px-3 py-1 bg-zinc-700 text-white cursor-default">
            Browser
          </button>
          <button
            onClick={() => setMode("live")}
            className="px-3 py-1 text-zinc-500 hover:text-zinc-300 transition-colors"
          >
            Live
          </button>
        </div>

        <div className="flex gap-3 text-xs text-zinc-500">
          {Object.entries(counts).sort().map(([fmt, n]) => (
            <span key={fmt}>
              <span className={FORMAT_COLORS[fmt] ?? "text-zinc-400"}>{fmt}</span>
              {" "}{n}
            </span>
          ))}
          {plugins.length > 0 && (
            <span className="text-zinc-700">total {plugins.length}</span>
          )}
        </div>

        {/* Global search */}
        <div ref={searchRef} className="flex-1 max-w-sm relative">
          <input
            type="text"
            value={searchQuery}
            onChange={e => { setSearchQuery(e.target.value); setSearchOpen(true) }}
            onFocus={() => setSearchOpen(true)}
            onKeyDown={e => { if (e.key === "Escape") { setSearchQuery(""); setSearchOpen(false) } }}
            placeholder="Search plugins, chains, presets…"
            className="w-full bg-zinc-800 border border-zinc-700 rounded px-3 py-1 text-xs placeholder-zinc-600 focus:outline-none focus:border-zinc-500"
          />
          {searchOpen && (searchQuery.trim().length > 0) && (
            <div className="absolute top-full left-0 right-0 mt-1 z-50 bg-zinc-900 border border-zinc-700 rounded shadow-xl overflow-y-auto max-h-80">
              <SearchResults
                hits={searchHits}
                onSelectPlugin={handleSearchSelectPlugin}
                onSelectChain={handleSearchSelectChain}
                onClose={() => { setSearchQuery(""); setSearchOpen(false) }}
              />
            </div>
          )}
        </div>

        {lastScan && (
          <span className="text-xs text-zinc-600">
            +{lastScan.plugins_found} found · {lastScan.plugins_skipped} skipped
          </span>
        )}

        <button
          onClick={exportDossier}
          disabled={exporting || plugins.length === 0}
          className="bg-zinc-800 hover:bg-zinc-700 disabled:opacity-40 border border-zinc-700 rounded px-3 py-1 text-xs transition-colors"
        >
          {exporting ? "Exporting…" : "Export"}
        </button>

        <button
          onClick={scan}
          disabled={scanning}
          className="bg-zinc-800 hover:bg-zinc-700 disabled:opacity-40 border border-zinc-700 rounded px-3 py-1 text-xs transition-colors"
        >
          {scanning ? "Scanning…" : "Scan"}
        </button>
      </div>

      {/* Status bars */}
      {scanError && (
        <div className="px-4 py-2 text-red-400 text-xs border-b border-zinc-800 shrink-0 flex items-center gap-3">
          <span>{scanError}</span>
          <button onClick={() => setScanError(null)} className="text-zinc-600 hover:text-zinc-400 ml-auto">✕</button>
        </div>
      )}
      {exportError && (
        <div className="flex items-center gap-3 px-4 py-2 text-red-400 text-xs border-b border-zinc-800 shrink-0">
          <span>Export failed: {exportError}</span>
          <button onClick={() => setExportError(null)} className="text-zinc-600 hover:text-zinc-400 ml-auto">✕</button>
        </div>
      )}
      {exportResult && (
        <div className="flex items-center gap-3 px-4 py-2 text-xs border-b border-zinc-800 bg-zinc-900/60 shrink-0">
          <span className="text-green-400">✓</span>
          <span className="text-zinc-400">Exported {exportResult.plugin_count} plugins</span>
          <button
            onClick={() => openPath(exportResult.html_path)}
            className="text-blue-400 hover:text-blue-300 underline"
          >
            Open HTML
          </button>
          <button
            onClick={() => openPath(exportResult.json_path)}
            className="text-blue-400 hover:text-blue-300 underline"
          >
            Open JSON
          </button>
          <button onClick={() => setExportResult(null)} className="ml-auto text-zinc-600 hover:text-zinc-400">✕</button>
        </div>
      )}

      {/* 3-panel body */}
      <div className="flex flex-1 overflow-hidden">
        {/* Left: Plugin list */}
        <div className="w-72 min-w-[200px] border-r border-zinc-800 flex flex-col overflow-hidden">
          <PluginFilters plugins={plugins} filters={filters} onChange={patchFilters} />
          {plugins.length === 0 ? (
            <div className="flex items-center justify-center flex-1 text-zinc-600 text-xs text-center px-6 leading-relaxed">
              Hit Scan to index your plugins
            </div>
          ) : (
            <PluginList
              plugins={plugins}
              filters={filters}
              selectedPlugin={selectedPlugin}
              onSelect={handleSelectPlugin}
            />
          )}
        </div>

        {/* Center: Chains + presets for selected plugin */}
        <div className="flex-1 border-r border-zinc-800 flex flex-col overflow-hidden min-w-0">
          <CenterPanel
            selectedPlugin={selectedPlugin}
            selectedChainId={selectedChainId}
            onSelectChain={setSelectedChainId}
          />
        </div>

        {/* Right: Chain / preset detail */}
        <div className="w-80 min-w-[240px] flex flex-col overflow-hidden border-l border-zinc-800">
          <RightPanel chainId={selectedChainId} onDeleteChain={handleDeleteChain} />
        </div>
      </div>
    </div>
  )
}
