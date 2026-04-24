use std::path::{Path, PathBuf};
use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ScanError {
    #[error("IO error at {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("Bad moduleinfo.json at {path}: {source}")]
    Json { path: PathBuf, source: serde_json::Error },
}

pub struct ScannedPlugin {
    pub name: String,
    pub vendor: Option<String>,
    pub version: Option<String>,
    pub category: Option<String>,
    pub class_id: Option<String>,
    pub path: PathBuf,
    pub format: PluginFormat,
}

#[derive(Debug, Clone, Copy)]
pub enum PluginFormat {
    Vst3,
}

impl PluginFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vst3 => "VST3",
        }
    }
}

// --- moduleinfo.json structures (VST3 SDK format) ---

#[derive(Deserialize)]
struct ModuleInfo {
    #[serde(rename = "Name")]
    name: Option<String>,
    #[serde(rename = "Version")]
    version: Option<String>,
    #[serde(rename = "Factory Info")]
    factory_info: Option<FactoryInfo>,
    #[serde(rename = "Classes")]
    classes: Option<Vec<ClassInfo>>,
}

#[derive(Deserialize)]
struct FactoryInfo {
    #[serde(rename = "Vendor")]
    vendor: Option<String>,
}

#[derive(Deserialize)]
struct ClassInfo {
    #[serde(rename = "CID")]
    cid: Option<String>,
    #[serde(rename = "Name")]
    name: Option<String>,
    #[serde(rename = "Vendor")]
    vendor: Option<String>,
    #[serde(rename = "Version")]
    version: Option<String>,
    // Sub Categories is the useful one ("Fx|Dynamics"), Category is always "Audio Module Class"
    #[serde(rename = "Sub Categories")]
    sub_categories: Option<Vec<String>>,
}

// --- Public API ---

pub fn default_vst3_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    #[cfg(target_os = "windows")]
    {
        let base = std::env::var("COMMONPROGRAMFILES")
            .unwrap_or_else(|_| r"C:\Program Files\Common Files".to_string());
        paths.push(PathBuf::from(base).join("VST3"));
    }

    #[cfg(target_os = "macos")]
    {
        paths.push(PathBuf::from("/Library/Audio/Plug-Ins/VST3"));
        if let Ok(home) = std::env::var("HOME") {
            paths.push(PathBuf::from(home).join("Library/Audio/Plug-Ins/VST3"));
        }
    }

    paths
}

/// Walk `paths`, find every `.vst3` bundle, parse metadata.
/// Returns (successful results, non-fatal errors) so callers see partial progress.
pub fn scan_vst3(paths: &[PathBuf]) -> (Vec<ScannedPlugin>, Vec<ScanError>) {
    let mut plugins = Vec::new();
    let mut errors = Vec::new();

    for root in paths {
        if !root.exists() {
            continue;
        }
        collect_bundles(root, &mut plugins, &mut errors);
    }

    (plugins, errors)
}

// --- Internals ---

fn collect_bundles(
    dir: &Path,
    plugins: &mut Vec<ScannedPlugin>,
    errors: &mut Vec<ScanError>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            errors.push(ScanError::Io { path: dir.to_path_buf(), source: e });
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e.eq_ignore_ascii_case("vst3")).unwrap_or(false) {
            match parse_vst3_bundle(&path) {
                Ok(p) => plugins.push(p),
                Err(e) => errors.push(e),
            }
        } else if path.is_dir() {
            // Vendors often nest plugins one level deep (e.g. "Fabfilter/FabFilter Pro-Q 3.vst3")
            collect_bundles(&path, plugins, errors);
        }
    }
}

fn parse_vst3_bundle(bundle: &Path) -> Result<ScannedPlugin, ScanError> {
    let moduleinfo_path = bundle.join("Contents").join("moduleinfo.json");

    if !moduleinfo_path.exists() {
        // Older VST3 plugins pre-date moduleinfo.json — use the bundle filename as fallback
        return Ok(ScannedPlugin {
            name: file_stem(bundle),
            vendor: None,
            version: None,
            category: None,
            class_id: None,
            path: bundle.to_path_buf(),
            format: PluginFormat::Vst3,
        });
    }

    let data = std::fs::read_to_string(&moduleinfo_path)
        .map_err(|e| ScanError::Io { path: moduleinfo_path.clone(), source: e })?;

    let info: ModuleInfo = serde_json::from_str(&data)
        .map_err(|e| ScanError::Json { path: moduleinfo_path, source: e })?;

    let first_class = info.classes.as_deref().and_then(|c| c.first());

    let name = first_class
        .and_then(|c| c.name.clone())
        .or_else(|| info.name.clone())
        .unwrap_or_else(|| file_stem(bundle));

    let vendor = first_class
        .and_then(|c| c.vendor.clone())
        .or_else(|| info.factory_info.as_ref().and_then(|f| f.vendor.clone()));

    let version = first_class
        .and_then(|c| c.version.clone())
        .or_else(|| info.version.clone());

    // Join sub-categories with "|" ("Fx|Dynamics") — ignore the generic "Audio Module Class" category
    let category = first_class.and_then(|c| {
        c.sub_categories
            .as_ref()
            .filter(|sc| !sc.is_empty())
            .map(|sc| sc.join("|"))
    });

    let class_id = first_class.and_then(|c| c.cid.clone());

    Ok(ScannedPlugin {
        name,
        vendor,
        version,
        category,
        class_id,
        path: bundle.to_path_buf(),
        format: PluginFormat::Vst3,
    })
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_bundle(dir: &Path, name: &str, moduleinfo: Option<&str>) -> PathBuf {
        let bundle = dir.join(format!("{name}.vst3"));
        fs::create_dir_all(bundle.join("Contents")).unwrap();
        if let Some(json) = moduleinfo {
            fs::write(bundle.join("Contents").join("moduleinfo.json"), json).unwrap();
        }
        bundle
    }

    #[test]
    fn parses_moduleinfo_json() {
        let tmp = TempDir::new().unwrap();
        make_bundle(tmp.path(), "TestPlugin", Some(r#"
        {
            "Name": "Test Plugin",
            "Version": "2.1.0",
            "Factory Info": { "Vendor": "Acme Audio" },
            "Classes": [{
                "CID": "AABBCCDD11223344AABBCCDD11223344",
                "Name": "Test Plugin",
                "Vendor": "Acme Audio",
                "Version": "2.1.0",
                "Sub Categories": ["Fx", "EQ"]
            }]
        }
        "#));

        let (plugins, errors) = scan_vst3(&[tmp.path().to_path_buf()]);
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
        assert_eq!(plugins.len(), 1);
        let p = &plugins[0];
        assert_eq!(p.name, "Test Plugin");
        assert_eq!(p.vendor.as_deref(), Some("Acme Audio"));
        assert_eq!(p.version.as_deref(), Some("2.1.0"));
        assert_eq!(p.category.as_deref(), Some("Fx|EQ"));
        assert_eq!(p.class_id.as_deref(), Some("AABBCCDD11223344AABBCCDD11223344"));
    }

    #[test]
    fn falls_back_to_bundle_name_when_no_moduleinfo() {
        let tmp = TempDir::new().unwrap();
        make_bundle(tmp.path(), "Legacy Plugin", None);

        let (plugins, errors) = scan_vst3(&[tmp.path().to_path_buf()]);
        assert!(errors.is_empty());
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "Legacy Plugin");
        assert!(plugins[0].vendor.is_none());
    }

    #[test]
    fn skips_nonexistent_paths() {
        let (plugins, errors) =
            scan_vst3(&[PathBuf::from("/does/not/exist/VST3")]);
        assert!(plugins.is_empty());
        assert!(errors.is_empty());
    }

    #[test]
    fn recurses_into_vendor_subdirectory() {
        let tmp = TempDir::new().unwrap();
        let vendor_dir = tmp.path().join("Fabfilter");
        fs::create_dir_all(&vendor_dir).unwrap();
        make_bundle(&vendor_dir, "Pro-Q 3", Some(r#"
        {
            "Name": "Pro-Q 3",
            "Factory Info": { "Vendor": "FabFilter" },
            "Classes": [{ "CID": "AABB", "Name": "Pro-Q 3", "Sub Categories": ["Fx", "EQ"] }]
        }
        "#));

        let (plugins, errors) = scan_vst3(&[tmp.path().to_path_buf()]);
        assert!(errors.is_empty());
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].name, "Pro-Q 3");
    }
}
