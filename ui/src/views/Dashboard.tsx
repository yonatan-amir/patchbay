import { type Plugin } from "../components/PluginFilters"

interface ExportResult {
  plugin_count: number
  json_path: string
  html_path: string
}

interface DashboardProps {
  plugins: Plugin[]
  scanning: boolean
  exporting: boolean
  exportResult: ExportResult | null
  exportError: string | null
  onScan: () => void
  onExport: () => void
  onOpenPath: (path: string) => void
  onDismissExport: () => void
}

interface VendorGroup {
  vendor: string
  count: number
  formats: string[]
  status: "ok" | "warning" | "error"
}

const FORMAT_COLORS: Record<string, string> = {
  VST3: "text-blue-400",
  AU: "text-green-400",
  VST2: "text-yellow-400",
  CLAP: "text-purple-400",
}

const FORMAT_BG: Record<string, string> = {
  VST3: "bg-blue-950/60",
  AU: "bg-green-950/60",
  VST2: "bg-yellow-950/60",
  CLAP: "bg-purple-950/60",
}

const STATUS_DOT: Record<string, string> = {
  ok: "bg-green-500",
  warning: "bg-yellow-500",
  error: "bg-red-500",
}

const STATUS_LABEL: Record<string, string> = {
  warning: "Legacy only",
  error: "Unidentified",
}

function deriveStatus(vendor: string, formats: string[]): "ok" | "warning" | "error" {
  if (vendor === "Unknown") return "error"
  if (formats.length > 0 && formats.every(f => f === "VST2")) return "warning"
  return "ok"
}

function groupByVendor(plugins: Plugin[]): VendorGroup[] {
  const map = new Map<string, { count: number; formats: Set<string> }>()
  for (const p of plugins) {
    const v = p.vendor ?? "Unknown"
    const entry = map.get(v) ?? { count: 0, formats: new Set<string>() }
    entry.count++
    entry.formats.add(p.format)
    map.set(v, entry)
  }
  return Array.from(map.entries())
    .map(([vendor, { count, formats }]) => {
      const fmtArr = Array.from(formats).sort()
      return { vendor, count, formats: fmtArr, status: deriveStatus(vendor, fmtArr) }
    })
    .sort((a, b) => b.count - a.count)
}

export function Dashboard({
  plugins,
  scanning,
  exporting,
  exportResult,
  exportError,
  onScan,
  onExport,
  onOpenPath,
  onDismissExport,
}: DashboardProps) {
  const vendors = groupByVendor(plugins)

  const formatCounts = plugins.reduce<Record<string, number>>((acc, p) => {
    acc[p.format] = (acc[p.format] ?? 0) + 1
    return acc
  }, {})

  const warnings = vendors.filter(v => v.status !== "ok")

  if (plugins.length === 0) {
    return (
      <div className="flex-1 flex flex-col items-center justify-center gap-5 text-zinc-600">
        <div className="text-center space-y-1">
          <p className="text-sm text-zinc-400">No plugins indexed yet</p>
          <p className="text-xs">Run a scan to build your library dashboard</p>
        </div>
        <button
          onClick={onScan}
          disabled={scanning}
          className="bg-zinc-800 hover:bg-zinc-700 disabled:opacity-40 border border-zinc-700 rounded px-4 py-2 text-xs text-zinc-300 transition-colors"
        >
          {scanning ? "Scanning…" : "Scan now"}
        </button>
      </div>
    )
  }

  return (
    <div className="flex-1 overflow-y-auto px-6 py-5 space-y-5">

      {/* Summary bar */}
      <div className="flex items-center gap-5 flex-wrap">
        <div className="flex items-baseline gap-2">
          <span className="text-3xl font-bold tabular-nums">{plugins.length}</span>
          <span className="text-xs text-zinc-500">plugins</span>
        </div>

        <div className="w-px h-7 bg-zinc-800" />

        <div className="flex items-baseline gap-2">
          <span className="text-3xl font-bold tabular-nums">{vendors.length}</span>
          <span className="text-xs text-zinc-500">vendors</span>
        </div>

        <div className="w-px h-7 bg-zinc-800" />

        <div className="flex gap-4">
          {Object.entries(formatCounts)
            .sort(([a], [b]) => a.localeCompare(b))
            .map(([fmt, n]) => (
              <div key={fmt} className="flex items-baseline gap-1.5">
                <span className={`text-xl font-bold tabular-nums ${FORMAT_COLORS[fmt] ?? "text-zinc-400"}`}>
                  {n}
                </span>
                <span className="text-xs text-zinc-500">{fmt}</span>
              </div>
            ))}
        </div>

        <div className="ml-auto flex gap-2 shrink-0">
          <button
            onClick={onExport}
            disabled={exporting || plugins.length === 0}
            className="bg-zinc-800 hover:bg-zinc-700 disabled:opacity-40 border border-zinc-700 rounded px-3 py-1.5 text-xs transition-colors"
          >
            {exporting ? "Exporting…" : "Export dossier"}
          </button>
          <button
            onClick={onScan}
            disabled={scanning}
            className="bg-zinc-800 hover:bg-zinc-700 disabled:opacity-40 border border-zinc-700 rounded px-3 py-1.5 text-xs transition-colors"
          >
            {scanning ? "Scanning…" : "Re-scan"}
          </button>
        </div>
      </div>

      {/* Export feedback */}
      {exportResult && (
        <div className="flex items-center gap-3 px-4 py-2.5 bg-zinc-900 border border-zinc-800 rounded text-xs">
          <span className="text-green-400">✓</span>
          <span className="text-zinc-400">Exported {exportResult.plugin_count} plugins</span>
          <button
            onClick={() => onOpenPath(exportResult.html_path)}
            className="text-blue-400 hover:text-blue-300 underline"
          >
            Open HTML
          </button>
          <button
            onClick={() => onOpenPath(exportResult.json_path)}
            className="text-blue-400 hover:text-blue-300 underline"
          >
            Open JSON
          </button>
          <button
            onClick={onDismissExport}
            className="ml-auto text-zinc-600 hover:text-zinc-400"
          >
            ✕
          </button>
        </div>
      )}
      {exportError && (
        <div className="px-4 py-2.5 bg-zinc-900 border border-red-900/50 rounded text-xs text-red-400">
          Export failed: {exportError}
        </div>
      )}

      {/* Warnings banner */}
      {warnings.length > 0 && (
        <div className="flex items-center gap-2 px-4 py-2.5 bg-yellow-950/20 border border-yellow-900/30 rounded text-xs">
          <span className="w-2 h-2 rounded-full bg-yellow-500 shrink-0" />
          <span className="text-yellow-400 font-medium">
            {warnings.length} vendor{warnings.length > 1 ? "s" : ""} need attention
          </span>
          <span className="text-yellow-700">—</span>
          <span className="text-yellow-700">{warnings.map(w => w.vendor).join(", ")}</span>
        </div>
      )}

      {/* Vendor grid */}
      <div>
        <h2 className="text-[10px] font-medium text-zinc-600 uppercase tracking-widest mb-3">
          Vendors
        </h2>
        <div className="grid gap-3" style={{ gridTemplateColumns: "repeat(auto-fill, minmax(210px, 1fr))" }}>
          {vendors.map(v => (
            <div
              key={v.vendor}
              className="bg-zinc-900 border border-zinc-800 rounded-lg p-4 flex flex-col gap-2.5"
            >
              <div className="flex items-start justify-between gap-2">
                <span className="font-medium text-sm leading-tight break-words min-w-0">
                  {v.vendor}
                </span>
                <div
                  className={`mt-1 w-2 h-2 rounded-full shrink-0 ${STATUS_DOT[v.status]}`}
                  title={v.status === "ok" ? "OK" : STATUS_LABEL[v.status]}
                />
              </div>

              <div className="flex items-baseline gap-1.5">
                <span className="text-2xl font-bold tabular-nums">{v.count}</span>
                <span className="text-xs text-zinc-500">
                  plugin{v.count !== 1 ? "s" : ""}
                </span>
              </div>

              <div className="flex flex-wrap gap-1">
                {v.formats.map(f => (
                  <span
                    key={f}
                    className={`text-[10px] font-semibold px-1.5 py-0.5 rounded ${FORMAT_COLORS[f] ?? "text-zinc-400"} ${FORMAT_BG[f] ?? "bg-zinc-800"}`}
                  >
                    {f}
                  </span>
                ))}
              </div>

              {v.status !== "ok" && (
                <p className="text-[10px] text-zinc-600">{STATUS_LABEL[v.status]}</p>
              )}
            </div>
          ))}
        </div>
      </div>
    </div>
  )
}
