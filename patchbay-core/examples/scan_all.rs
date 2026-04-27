use std::collections::HashMap;
use patchbay_core::scanner;

fn main() {
    let empty: HashMap<String, i64> = HashMap::new();

    println!("=== VST3 ===");
    let (plugins, _, errors) = scanner::scan_vst3(&scanner::default_vst3_paths(), &empty);
    println!("  found: {}", plugins.len());
    for e in &errors { eprintln!("  err: {e}"); }

    println!("\n=== VST2 ===");
    let vst2_probe = scanner::find_vst2_probe();
    let (plugins, _, errors) = scanner::scan_vst2(&scanner::default_vst2_paths(), vst2_probe.as_deref(), &empty);
    println!("  found: {}  (probe: {})", plugins.len(), if vst2_probe.is_some() { "yes" } else { "no" });
    for e in &errors { eprintln!("  err: {e}"); }

    println!("\n=== CLAP ===");
    let clap_probe = scanner::find_clap_probe();
    let (plugins, _, errors) = scanner::scan_clap(&scanner::default_clap_paths(), clap_probe.as_deref(), &empty);
    println!("  found: {}  (probe: {})", plugins.len(), if clap_probe.is_some() { "yes" } else { "no" });
    for e in &errors { eprintln!("  err: {e}"); }

    println!("\n=== AU ===");
    let (plugins, _, errors) = scanner::scan_au();
    let bundles: std::collections::HashSet<_> = plugins.iter().map(|p| &p.path).collect();
    println!("  found: {}  (from {} bundles)", plugins.len(), bundles.len());
    for e in &errors { eprintln!("  err: {e}"); }
}
