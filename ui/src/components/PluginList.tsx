import { useRef, useMemo } from "react"
import { useVirtualizer } from "@tanstack/react-virtual"
import { cn } from "../lib/utils"
import type { Plugin, Filters } from "./PluginFilters"

interface Props {
  plugins: Plugin[]
  filters: Filters
  selectedPlugin: Plugin | null
  onSelect: (p: Plugin) => void
}

const FORMAT_COLORS: Record<string, string> = {
  VST3: "text-blue-400",
  AU: "text-green-400",
  VST2: "text-yellow-400",
  CLAP: "text-purple-400",
}

export function PluginList({ plugins, filters, selectedPlugin, onSelect }: Props) {
  const parentRef = useRef<HTMLDivElement>(null)

  const visible = useMemo(() => {
    const s = filters.search.toLowerCase()
    return plugins.filter(p => {
      if (filters.format && p.format !== filters.format) return false
      if (filters.vendor && p.vendor !== filters.vendor) return false
      if (filters.category && p.category !== filters.category) return false
      if (s) {
        const nameMatch = p.name.toLowerCase().includes(s)
        const vendorMatch = (p.vendor ?? "").toLowerCase().includes(s)
        if (!nameMatch && !vendorMatch) return false
      }
      return true
    })
  }, [plugins, filters])

  const virtualizer = useVirtualizer({
    count: visible.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => 34,
    overscan: 12,
  })

  if (visible.length === 0) {
    return (
      <div ref={parentRef} className="flex-1 flex items-center justify-center text-zinc-600 text-xs">
        No matches
      </div>
    )
  }

  return (
    <div ref={parentRef} className="flex-1 overflow-auto">
      <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
        {virtualizer.getVirtualItems().map(vr => {
          const p = visible[vr.index]
          const selected = selectedPlugin?.name === p.name && selectedPlugin?.format === p.format
          return (
            <div
              key={vr.key}
              data-index={vr.index}
              ref={virtualizer.measureElement}
              style={{ position: "absolute", top: vr.start, left: 0, right: 0 }}
              onClick={() => onSelect(p)}
              className={cn(
                "flex items-center gap-2 px-3 py-2 cursor-pointer border-b border-zinc-900",
                selected ? "bg-zinc-800" : "hover:bg-zinc-900/50"
              )}
            >
              <span className={cn("text-xs font-mono w-10 shrink-0", FORMAT_COLORS[p.format] ?? "text-zinc-500")}>
                {p.format}
              </span>
              <span className="text-sm truncate flex-1">{p.name}</span>
              {p.vendor && (
                <span className="text-xs text-zinc-600 truncate max-w-[90px] shrink-0">{p.vendor}</span>
              )}
            </div>
          )
        })}
      </div>
    </div>
  )
}
