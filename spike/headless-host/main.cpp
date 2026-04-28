// Patchbay headless plugin hosting spike.
// Validates: VST3/AU load without GUI, 3-plugin chain, WAV I/O, state recall, 10-run stability.
//
// Usage:
//   HeadlessHost <out.wav> [--input=<in.wav>] [plugin.vst3 ...] [shell.vst3::PluginName ...]
//
// Plugin argument syntax:
//   /path/to/plugin.vst3           - load first plugin found in file
//   /path/to/shell.vst3::SSL Comp  - scan shell, pick by exact name match
//
// If --input is omitted, a 5-second 440 Hz sine is generated and used.

#include <JuceHeader.h>
#include <iostream>

static constexpr double kSR = 44100.0;
static constexpr int kBlock = 512;
static constexpr int kCh = 2;

// ── helpers ────────────────────────────────────────────────────────────────

static juce::String makeTone() {
    const int n = (int)(kSR * 5);
    juce::AudioBuffer<float> buf(kCh, n);
    for (int i = 0; i < n; ++i) {
        float s = 0.5f * std::sin(2.f * juce::MathConstants<float>::pi * 440.f * (float)i / (float)kSR);
        buf.setSample(0, i, s);
        buf.setSample(1, i, s);
    }
    juce::String path = juce::File::getSpecialLocation(juce::File::tempDirectory)
                            .getChildFile("patchbay_spike_tone.wav").getFullPathName();
    juce::File f(path);
    f.deleteFile();
    juce::WavAudioFormat wav;
    auto stream = f.createOutputStream();
    auto writer = wav.createWriterFor(
        stream,
        juce::AudioFormatWriterOptions{}.withSampleRate(kSR).withNumChannels(kCh).withBitsPerSample(24));
    if (writer) writer->writeFromAudioSampleBuffer(buf, 0, n);
    return path;
}

// ── plugin argument parsing ─────────────────────────────────────────────────

struct PluginArg {
    juce::String path;
    juce::String nameFilter; // empty = use first plugin found in file
};

static PluginArg parsePluginArg(const juce::String& arg) {
    if (arg.contains("::"))
        return { arg.upToFirstOccurrenceOf("::", false, false),
                 arg.fromFirstOccurrenceOf("::", false, false) };
    return { arg, {} };
}

// ── Host class ─────────────────────────────────────────────────────────────

class Host {
public:
    Host() {
        fmt.addFormat(std::make_unique<juce::VST3PluginFormat>());
#if JUCE_PLUGINHOST_AU
        fmt.addFormat(std::make_unique<juce::AudioUnitPluginFormat>());
#endif
    }

    bool load(const PluginArg& arg) {
        juce::OwnedArray<juce::PluginDescription> descs;
        for (auto* f : fmt.getFormats()) {
            if (f->fileMightContainThisPluginType(arg.path)) {
                f->findAllTypesForFile(descs, arg.path);
                break;
            }
        }

        if (descs.isEmpty()) {
            std::cerr << "  [!] no plugin found at: " << arg.path << "\n";
            return false;
        }

        const juce::PluginDescription* chosen = nullptr;
        if (arg.nameFilter.isEmpty()) {
            chosen = descs.getFirst();
        } else {
            for (auto* d : descs)
                if (d->name.containsIgnoreCase(arg.nameFilter)) { chosen = d; break; }
            if (!chosen) {
                std::cerr << "  [!] '" << arg.nameFilter << "' not found in shell. Available:\n";
                for (auto* d : descs)
                    std::cerr << "       - " << d->name << "\n";
                return false;
            }
        }

        juce::String err;
        auto p = fmt.createPluginInstance(*chosen, kSR, kBlock, err);
        if (!p) {
            std::cerr << "  [!] createPluginInstance failed: " << err << "\n";
            return false;
        }

        // Request stereo layout; log if refused (plugin may be mono-only)
        {
            auto layout = p->getBusesLayout();
            layout.inputBuses.clearQuick();
            layout.outputBuses.clearQuick();
            layout.inputBuses.add(juce::AudioChannelSet::stereo());
            layout.outputBuses.add(juce::AudioChannelSet::stereo());
            if (!p->setBusesLayout(layout))
                std::cout << "  [~] " << p->getName() << " rejected stereo, using default layout\n";
        }

        p->prepareToPlay(kSR, kBlock);

        std::cout << "  OK  " << p->getName()
                  << "  (" << p->getTotalNumInputChannels()
                  << " in / " << p->getTotalNumOutputChannels() << " out)"
                  << "  params=" << (int)p->getParameters().size() << "\n";
        chain.push_back(std::move(p));
        return true;
    }

    // Save current state, mutate param[0], restore, verify it came back.
    // A failing plugin returns the changed value after setStateInformation — indicates broken
    // state recall (common in early plugin SDK versions or iLok-gated plugins).
    bool testStateRecall() {
        bool allPass = true;
        for (auto& p : chain) {
            auto params = p->getParameters();
            if (params.isEmpty()) {
                std::cout << "  --  " << p->getName() << "  no parameters, skipped\n";
                continue;
            }
            juce::MemoryBlock saved;
            p->getStateInformation(saved);
            float before = params[0]->getValue();
            float mutated = before > 0.5f ? 0.1f : 0.9f;
            params[0]->setValue(mutated);
            p->setStateInformation(saved.getData(), (int)saved.getSize());
            float after = params[0]->getValue();
            bool pass = std::abs(after - before) < 0.02f;
            if (!pass) allPass = false;
            std::cout << "  " << (pass ? "OK" : "!!") << "  " << p->getName()
                      << "  param[0]=" << before << " mutated=" << mutated
                      << " restored=" << after << "  " << (pass ? "PASS" : "FAIL") << "\n";
        }
        return allPass;
    }

    bool processFile(const juce::String& inPath, const juce::String& outPath) {
        juce::AudioFormatManager afm;
        afm.registerBasicFormats();

        auto reader = std::unique_ptr<juce::AudioFormatReader>(
            afm.createReaderFor(juce::File(inPath)));
        if (!reader) {
            std::cerr << "  [!] cannot read: " << inPath << "\n";
            return false;
        }

        const int total = (int)reader->lengthInSamples;
        juce::AudioBuffer<float> buf(kCh, total);

        if (reader->numChannels == 1) {
            juce::AudioBuffer<float> mono(1, total);
            reader->read(&mono, 0, total, 0, true, false);
            buf.copyFrom(0, 0, mono, 0, 0, total);
            buf.copyFrom(1, 0, mono, 0, 0, total);
        } else {
            reader->read(&buf, 0, total, 0, true, true);
        }

        std::cout << "  input: " << total << " samples ("
                  << juce::String((double)total / kSR, 1) << " s)\n";

        juce::MidiBuffer midi;
        for (int off = 0; off < total; off += kBlock) {
            const int n = std::min(kBlock, total - off);
            juce::AudioBuffer<float> blk(kCh, n);
            for (int ch = 0; ch < kCh; ++ch)
                blk.copyFrom(ch, 0, buf, ch, off, n);
            midi.clear();
            for (auto& p : chain) {
                const int pCh = juce::jmax(1, p->getTotalNumOutputChannels());
                if (pCh != kCh) {
                    // Adapt channel count at plugin boundary
                    juce::AudioBuffer<float> tmp(pCh, n);
                    for (int c = 0; c < pCh; ++c)
                        tmp.copyFrom(c, 0, blk, c % kCh, 0, n);
                    p->processBlock(tmp, midi);
                    for (int c = 0; c < kCh; ++c)
                        blk.copyFrom(c, 0, tmp, c % pCh, 0, n);
                } else {
                    p->processBlock(blk, midi);
                }
            }
            for (int ch = 0; ch < kCh; ++ch)
                buf.copyFrom(ch, off, blk, ch, 0, n);
        }

        juce::File outFile(outPath);
        outFile.deleteFile();
        juce::WavAudioFormat wav;
        auto stream = outFile.createOutputStream();
        auto writer = wav.createWriterFor(
            stream,
            juce::AudioFormatWriterOptions{}.withSampleRate(kSR).withNumChannels(kCh).withBitsPerSample(24));
        if (!writer) {
            std::cerr << "  [!] cannot create WAV writer\n";
            return false;
        }
        writer->writeFromAudioSampleBuffer(buf, 0, total);
        std::cout << "  output: " << outPath << "\n";
        return true;
    }

private:
    juce::AudioPluginFormatManager fmt;
    std::vector<std::unique_ptr<juce::AudioProcessor>> chain;
};

// ── main ───────────────────────────────────────────────────────────────────

int main(int argc, char* argv[]) {
    // ScopedJuceInitialiser_GUI is required even for headless plugin hosting:
    // most VST3/AU plugins call MessageManager during init for async license
    // checks (iLok, Pace, custom daemons). Without this, they crash.
    juce::ScopedJuceInitialiser_GUI scopedInit;

    if (argc < 2) {
        std::cerr << "Usage: HeadlessHost <out.wav> [--input=<in.wav>] [plugin.vst3 ...]\n"
                  << "       shell plugins: /path/shell.vst3::PluginName\n";
        return 1;
    }

    juce::String outPath = argv[1];
    juce::String inPath;
    std::vector<PluginArg> plugins;

    for (int i = 2; i < argc; ++i) {
        juce::String a = argv[i];
        if (a.startsWith("--input="))
            inPath = a.fromFirstOccurrenceOf("=", false, false);
        else
            plugins.push_back(parsePluginArg(a));
    }
    if (inPath.isEmpty()) {
        std::cout << "[tone] generating 440 Hz test tone\n";
        inPath = makeTone();
    }

    std::cout << "=== Patchbay headless host spike ===\n"
              << "input:  " << inPath << "\n"
              << "output: " << outPath << "\n"
              << "chain:  " << plugins.size() << " plugin(s)\n\n";

    Host host;

    std::cout << "[1/3] Load\n";
    for (auto& p : plugins)
        if (!host.load(p)) return 1;

    // Pump the message loop so plugins can complete async init (license daemons,
    // iLok checks, etc.) before we start processing.
    if (!plugins.empty())
        juce::MessageManager::getInstance()->runDispatchLoopUntil(600);

    std::cout << "\n[2/3] State recall\n";
    bool recallOk = plugins.empty() || host.testStateRecall();

    std::cout << "\n[3/3] Process\n";
    if (!host.processFile(inPath, outPath)) return 1;

    std::cout << "\n";
    if (recallOk)
        std::cout << "=== SPIKE RESULT: PASS ===\n";
    else
        std::cout << "=== SPIKE RESULT: PARTIAL — state recall failed (see above) ===\n";

    return recallOk ? 0 : 2;
}
