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

interface ExportResult {
  plugin_count: number;
  json_path: string;
  html_path: string;
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
  const [exporting, setExporting] = useState(false);
  const [exportResult, setExportResult] = useState<ExportResult | null>(null);
  const [exportError, setExportError] = useState<string | null>(null);

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

  async function exportDossier() {
    setExporting(true);
    setExportError(null);
    setExportResult(null);
    try {
      const result = await invoke<ExportResult>("export_library_dossier");
      setExportResult(result);
    } catch (e) {
      setExportError(String(e));
    } finally {
      setExporting(false);
    }
  }

  async function openPath(path: string) {
    try {
      await invoke("open_path", { path });
    } catch (_) {}
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
          onClick={exportDossier}
          disabled={exporting || plugins.length === 0}
          className="bg-zinc-800 hover:bg-zinc-700 disabled:opacity-40 border border-zinc-700 rounded px-3 py-1 text-xs transition-colors"
        >
          {exporting ? "Exporting…" : "Export Dossier"}
        </button>

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

      {/* Export error */}
      {exportError && (
        <div className="flex items-center gap-3 px-4 py-2 text-red-400 text-xs border-b border-zinc-800">
          <span>Export failed: {exportError}</span>
          <button onClick={() => setExportError(null)} className="text-zinc-600 hover:text-zinc-400">✕</button>
        </div>
      )}

      {/* Export success banner */}
      {exportResult && (
        <div className="flex items-center gap-3 px-4 py-2 text-xs border-b border-zinc-800 bg-zinc-900/60">
          <span className="text-green-400">✓</span>
          <span className="text-zinc-400">
            Exported {exportResult.plugin_count} plugins to{" "}
            <span className="text-zinc-300 font-medium">{exportResult.html_path}</span>
          </span>
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
