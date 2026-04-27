use patchbay_core::scanner;

fn main() {
    let paths = scanner::default_vst3_paths();
    println!("Scanning paths:");
    for p in &paths {
        println!("  {}", p.display());
    }
    println!();

    let (plugins, skipped, errors) = scanner::scan_vst3(&paths, &std::collections::HashMap::new());

    println!("Found {} plugins ({} skipped — unchanged since last scan):\n", plugins.len(), skipped);
    for p in &plugins {
        println!(
            "  [{:6}] {:<40} | vendor: {:<30} | version: {}",
            p.format.as_str(),
            p.name,
            p.vendor.as_deref().unwrap_or("—"),
            p.version.as_deref().unwrap_or("—"),
        );
    }

    if !errors.is_empty() {
        println!("\n{} non-fatal errors:", errors.len());
        for e in &errors {
            println!("  {e}");
        }
    }

    let with_vendor = plugins.iter().filter(|p| p.vendor.is_some()).count();
    let with_version = plugins.iter().filter(|p| p.version.is_some()).count();
    println!(
        "\nSummary: {} total | {} with vendor ({:.0}%) | {} with version ({:.0}%)",
        plugins.len(),
        with_vendor,
        100.0 * with_vendor as f64 / plugins.len().max(1) as f64,
        with_version,
        100.0 * with_version as f64 / plugins.len().max(1) as f64,
    );
}
