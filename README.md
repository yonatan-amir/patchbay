# Patchbay

**The operating system for your plugin library.**

Patchbay scans your entire plugin library across every DAW, builds a unified searchable index, translates plugin chains between DAWs, and matches presets to a reference audio clip using AI. The scanner, preset browser, and Plugin Library Rescue are MIT-licensed and always free. Pro features are license-gated but source-visible.

[![CI](https://github.com/yonatan-amir/patchbay/actions/workflows/ci.yml/badge.svg)](https://github.com/yonatan-amir/patchbay/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

---

## The problem

Your plugins are scattered across five DAWs, three vendor browsers, and a folder of unnamed `.vstpreset` files. Native Instruments is in crisis. Waves keeps changing their licensing. Your Ableton chain sounds perfect — but your collaborator uses Logic.

Patchbay fixes this. One tool. All your plugins. All your DAWs.

---

## Features

### Plugin Library Rescue
Centralized recovery and migration tool for the most common plugin licensing disasters — NI Kontakt, iZotope, Waves, Plugin Alliance, and any iLok-dependent plugin. This is the first thing you see when you open Patchbay, and it's permanently maintained as a core feature.

### Unified Preset Browser
One search box across every plugin and DAW you own. Scans VST3, AU, AAX, VST2, and CLAP. Reads native preset formats for Ableton (`.adv`, `.adg`), Logic (`.pst`, AU state), Cubase (`.vstpreset`), Pro Tools, and Luna/UAD. Indexed locally in SQLite with timbral tags — no cloud, no account, instant search.

### Cross-DAW Chain Translation
Export a plugin chain from any DAW and reconstruct it in another. Phase 1 handles structural translation: same plugins, same parameter values, target DAW format. Phase 2 adds semantic matching for when a plugin isn't available on the other side.

### AI Sound Matching *(Phase 4)*
Drop an audio clip and get back the closest presets from your own library. Rust-native audio analysis — no Python sidecar, no GPU. Spectral centroid, low-mid energy, transient shape, and reverb tail extracted and matched against your indexed presets.

---

## Why open source

Plugin format parsers need constant maintenance. Vendors change their formats, add DRM, go bankrupt. The community is the only sustainable maintainer for a library like this. Closed-source alternatives (NI, Ableton, Splice) have a commercial incentive to keep you locked in — Patchbay's incentive is the opposite.

The scanner, preset index, browser, and Plugin Library Rescue are MIT-licensed. Pro features (cloud sync, chain export, AI matching) are license-gated but source-visible: you can read the code and fork if you need to.

---

## Tech stack

| Layer | Choice | Why |
|---|---|---|
| Desktop shell | Tauri 2 + Rust | ~8 MB binary, native performance, no Chromium |
| UI | React + TypeScript + shadcn/ui | Largest ecosystem, no churn risk |
| Local DB | SQLite + FTS5 | Zero ops, handles 1M+ presets, sync-ready schema from day 1 |
| Audio analysis | Rust-native (rustfft + symphonia) | No Python bundle, consistent with the rest of the stack |
| Vector search (Phase 4) | sqlite-vec | Drops into existing SQLite — no migration |
| DAW plugin (Phase 3+) | JUCE | Separate IPC module, VST3 + AU first, AAX last |
| Cloud (Phase 2+) | Cloudflare R2 + Clerk + Stripe | No egress fees; auth free to 10K MAU; billing handled |

---

## Roadmap

| Phase | Timeline | What ships |
|---|---|---|
| **1** | Months 1–2 | Plugin Library Rescue · VST3 + Ableton + Cubase scanner · Unified preset browser — **$49 one-time** |
| **2** | Months 3–4 | Logic + Pro Tools · Cloud sync · User accounts — **$9/mo** |
| **3** | Months 5–6 | Cross-DAW chain export (structural) · JUCE in-DAW companion (VST3 + AU) |
| **4** | Months 7–9 | AI sound matching Phase 1 · Rust-native analysis, no training required |
| **5** | Months 10+ | Generative chain synthesis · Semantic chain translation |

---

## Contributing

Contributions are welcome at any stage. The highest-leverage areas:

- **Format parsers** — `.vstpreset`, `.adv`, `.adg`, `.pst`, and any vendor-specific format. New formats, edge cases, post-update breakage.
- **Platform testing** — macOS AU scanning, Pro Tools AAX, Luna/UAD formats require hardware access we may not have.
- **Bug reports** — open an issue with OS, DAW version, plugin format, and a sample preset if possible.

### Development setup

Requirements: [Rust](https://rustup.rs/) stable · [Node.js](https://nodejs.org/) 20+ · [Tauri v2 prerequisites](https://v2.tauri.app/start/prerequisites/)

```sh
git clone https://github.com/yonatan-amir/patchbay
cd patchbay
npm run ui:install     # install frontend dependencies
npm run dev            # start Tauri dev server (Rust + React hot reload)
```

Run checks independently:

```sh
cargo check --workspace
cargo test --workspace
```

---

## License

MIT © Yonatan Amir — see [LICENSE](LICENSE).

The scanner, preset index, browser, and Plugin Library Rescue are free forever. Pro features are source-visible but require a license for use.
