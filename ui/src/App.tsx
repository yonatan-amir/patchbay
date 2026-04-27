import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import PluginDetail from "./views/PluginDetail";

interface Plugin {
  name: string;
  vendor: string | null;
  format: string;
  category: string | null;
}

interface ScanResult {
  plugins_found: number;
  plugins_skipped: number;
  errors: string[];
}

const FORMAT_COLORS: Record<string, string> = {
  VST3: "text-blue-400",
  AU:   "text-green-400",
  VST2: "text-yellow-400",
  CLAP: "text-purple-400",
};

export default function App() {
  const [plugins, setPlugins] = useState<Plugin[]>([]);
  const [scanning, setScanning] = useState(false);
  const [lastScan, setLastScan] = useState<ScanResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [selectedPlugin, setSelectedPlugin] = useState<string | null>(null);

  useEffect(() => {
    invoke<Plugin[]>("list_plugins")
      .then(setPlugins)
      .catch(() => {});
  }, []);

  async function scan() {
    setScanning(true);
    setError(null);
    try {
      const result = await invoke<ScanResult>("scan_plugins");
      setLastScan(result);
      const updated = await invoke<Plugin[]>("list_plugins");
      setPlugins(updated);
    } catch (e) {
      setError(String(e));
    } finally {
      setScanning(false);
    }
  }

  if (selectedPlugin !== null) {
    return (
      <div className="flex flex-col h-screen bg-zinc-950 text-zinc-100 font-mono text-sm">
        <PluginDetail name={selectedPlugin} onBack={() => setSelectedPlugin(null)} />
      </div>
    );
  }

  const visible = filter
    ? plugins.filter(p =>
        p.name.toLowerCase().includes(filter.toLowerCase()) ||
        (p.vendor ?? "").toLowerCase().includes(filter.toLowerCase()) ||
        p.format.toLowerCase().includes(filter.toLowerCase())
      )
    : plugins;

  const counts = plugins.reduce<Record<string, number>>((acc, p) => {
    acc[p.format] = (acc[p.format] ?? 0) + 1;
    return acc;
  }, {});

  return (
    <div className="flex flex-col h-screen bg-zinc-950 text-zinc-100 font-mono text-sm">
      {/* Header */}
      <div className="flex items-center gap-4 px-4 py-3 border-b border-zinc-800">
        <span className="font-bold tracking-tight text-base">Patchbay</span>

        <div className="flex gap-3 text-xs text-zinc-500">
          {Object.entries(counts).sort().map(([fmt, n]) => (
            <span key={fmt}>
              <span className={FORMAT_COLORS[fmt] ?? "text-zinc-400"}>{fmt}</span>
              {" "}{n}
            </span>
          ))}
          {plugins.length > 0 && (
            <span className="text-zinc-600">total {plugins.length}</span>
          )}
        </div>

        <div className="flex-1" />

        {lastScan && (
          <span className="text-xs text-zinc-600">
            +{lastScan.plugins_found} found · {lastScan.plugins_skipped} skipped
          </span>
        )}

        <input
          type="text"
          placeholder="filter..."
          value={filter}
          onChange={e => setFilter(e.target.value)}
          className="bg-zinc-900 border border-zinc-700 rounded px-2 py-1 text-xs w-40 focus:outline-none focus:border-zinc-500"
        />

        <button
          onClick={scan}
          disabled={scanning}
          className="bg-zinc-800 hover:bg-zinc-700 disabled:opacity-40 border border-zinc-700 rounded px-3 py-1 text-xs transition-colors"
        >
          {scanning ? "Scanning…" : "Scan"}
        </button>
      </div>

      {/* Error */}
      {error && (
        <div className="px-4 py-2 text-red-400 text-xs border-b border-zinc-800">
          {error}
        </div>
      )}

      {/* Table */}
      <div className="flex-1 overflow-auto">
        {visible.length === 0 ? (
          <div className="flex items-center justify-center h-full text-zinc-600 text-xs">
            {plugins.length === 0 ? "Hit Scan to index your plugins" : "No matches"}
          </div>
        ) : (
          <table className="w-full border-collapse">
            <thead className="sticky top-0 bg-zinc-950 border-b border-zinc-800">
              <tr className="text-zinc-500 text-xs">
                <th className="text-left px-4 py-2 font-normal w-8">#</th>
                <th className="text-left px-4 py-2 font-normal">Name</th>
                <th className="text-left px-4 py-2 font-normal">Vendor</th>
                <th className="text-left px-4 py-2 font-normal w-16">Format</th>
                <th className="text-left px-4 py-2 font-normal">Category</th>
              </tr>
            </thead>
            <tbody>
              {visible.map((p, i) => (
                <tr
                  key={i}
                  onClick={() => setSelectedPlugin(p.name)}
                  className="border-b border-zinc-900 hover:bg-zinc-900/50 cursor-pointer"
                >
                  <td className="px-4 py-1.5 text-zinc-600">{i + 1}</td>
                  <td className="px-4 py-1.5">{p.name}</td>
                  <td className="px-4 py-1.5 text-zinc-400">{p.vendor ?? ""}</td>
                  <td className="px-4 py-1.5">
                    <span className={FORMAT_COLORS[p.format] ?? "text-zinc-400"}>
                      {p.format}
                    </span>
                  </td>
                  <td className="px-4 py-1.5 text-zinc-500">{p.category ?? ""}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
