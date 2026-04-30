import { useMemo } from "react"

export interface Plugin {
  name: string
  vendor: string | null
  format: string
  category: string | null
}

export interface Filters {
  search: string
  format: string | null
  vendor: string | null
  category: string | null
}

interface Props {
  plugins: Plugin[]
  filters: Filters
  onChange: (f: Partial<Filters>) => void
}

const FORMAT_COLORS: Record<string, string> = {
  VST3: "text-blue-400",
  AU: "text-green-400",
  VST2: "text-yellow-400",
  CLAP: "text-purple-400",
}

export function PluginFilters({ plugins, filters, onChange }: Props) {
  const formats = useMemo(() => {
    return Array.from(new Set(plugins.map(p => p.format))).sort()
  }, [plugins])

  const vendors = useMemo(() => {
    return Array.from(new Set(plugins.map(p => p.vendor).filter((v): v is string => v !== null))).sort()
  }, [plugins])

  const categories = useMemo(() => {
    return Array.from(new Set(plugins.map(p => p.category).filter((c): c is string => c !== null))).sort()
  }, [plugins])

  return (
    <div className="flex flex-col gap-2 px-3 py-2.5 border-b border-zinc-800 shrink-0">
      <input
        type="text"
        placeholder="Search plugins…"
        value={filters.search}
        onChange={e => onChange({ search: e.target.value })}
        className="w-full bg-zinc-900 border border-zinc-700 rounded px-2.5 py-1.5 text-xs focus:outline-none focus:border-zinc-500 placeholder:text-zinc-600"
      />
      {formats.length > 0 && (
        <div className="flex gap-1 flex-wrap">
          {formats.map(fmt => (
            <button
              key={fmt}
              onClick={() => onChange({ format: filters.format === fmt ? null : fmt })}
              className={`text-xs px-2 py-0.5 rounded border transition-colors ${
                filters.format === fmt
                  ? `bg-zinc-800 border-zinc-600 ${FORMAT_COLORS[fmt] ?? "text-zinc-300"}`
                  : "border-zinc-800 text-zinc-600 hover:border-zinc-700 hover:text-zinc-400"
              }`}
            >
              {fmt}
            </button>
          ))}
        </div>
      )}
      <div className="flex gap-2">
        <select
          value={filters.vendor ?? ""}
          onChange={e => onChange({ vendor: e.target.value || null })}
          className="flex-1 bg-zinc-900 border border-zinc-700 rounded px-2 py-1 text-xs focus:outline-none focus:border-zinc-500 text-zinc-300 min-w-0"
        >
          <option value="">All vendors</option>
          {vendors.map(v => <option key={v} value={v}>{v}</option>)}
        </select>
        <select
          value={filters.category ?? ""}
          onChange={e => onChange({ category: e.target.value || null })}
          className="flex-1 bg-zinc-900 border border-zinc-700 rounded px-2 py-1 text-xs focus:outline-none focus:border-zinc-500 text-zinc-300 min-w-0"
        >
          <option value="">All categories</option>
          {categories.map(c => <option key={c} value={c}>{c}</option>)}
        </select>
      </div>
    </div>
  )
}
