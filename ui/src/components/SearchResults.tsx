import { useMemo } from "react"

export interface SearchHit {
  result_type: "plugin" | "chain" | "preset" | "note"
  id: number
  name: string
  subtitle: string
}

interface Props {
  hits: SearchHit[]
  onSelectPlugin: (name: string) => void
  onSelectChain: (id: number) => void
  onClose: () => void
}

const TYPE_LABEL: Record<string, string> = {
  plugin: "Plugins",
  chain: "Chains",
  preset: "Presets",
  note: "Notes",
}

const TYPE_ORDER = ["plugin", "chain", "preset", "note"]

export function SearchResults({ hits, onSelectPlugin, onSelectChain, onClose }: Props) {
  const groups = useMemo(() => {
    const map = new Map<string, SearchHit[]>()
    for (const hit of hits) {
      const group = map.get(hit.result_type) ?? []
      group.push(hit)
      map.set(hit.result_type, group)
    }
    return TYPE_ORDER.filter(t => map.has(t)).map(t => ({ type: t, items: map.get(t)! }))
  }, [hits])

  function handleClick(hit: SearchHit) {
    if (hit.result_type === "plugin" || hit.result_type === "note") {
      onSelectPlugin(hit.name)
    } else if (hit.result_type === "chain") {
      onSelectChain(hit.id)
    }
    onClose()
  }

  if (hits.length === 0) {
    return (
      <div className="px-4 py-3 text-xs text-zinc-500">No results</div>
    )
  }

  return (
    <div>
      {groups.map(({ type, items }) => (
        <div key={type}>
          <div className="px-3 py-1 text-[10px] font-semibold uppercase tracking-wider text-zinc-500 bg-zinc-900/60 border-b border-zinc-800">
            {TYPE_LABEL[type] ?? type} <span className="text-zinc-600 font-normal">{items.length}</span>
          </div>
          {items.map(hit => (
            <button
              key={`${hit.result_type}-${hit.id}`}
              onClick={() => handleClick(hit)}
              className="w-full flex items-baseline gap-2 px-3 py-1.5 text-left hover:bg-zinc-800 transition-colors"
            >
              <span className="text-xs text-zinc-100 truncate min-w-0">{hit.name}</span>
              {hit.subtitle && (
                <span className="text-[11px] text-zinc-500 shrink-0">{hit.subtitle}</span>
              )}
            </button>
          ))}
        </div>
      ))}
    </div>
  )
}
