use objc2_app_kit::NSWorkspace;

use super::{FrontmostApplication, FrontmostApplicationKind};

const CHROMIUM_BROWSER_BUNDLE_IDS: &[&str] = &[
    "com.google.Chrome",
    "com.google.Chrome.beta",
    "com.google.Chrome.dev",
    "com.google.Chrome.canary",
    "org.chromium.Chromium",
    "com.microsoft.edgemac",
    "com.microsoft.edgemac.Beta",
    "com.microsoft.edgemac.Dev",
    "com.microsoft.edgemac.Canary",
    "com.brave.Browser",
    "com.brave.Browser.beta",
    "com.brave.Browser.nightly",
    "company.thebrowser.Browser",
    "com.vivaldi.Vivaldi",
    "com.operasoftware.Opera",
];

pub(super) fn is_chromium_browser_bundle_id(identifier: &str) -> bool {
    CHROMIUM_BROWSER_BUNDLE_IDS.contains(&identifier)
}

pub(super) fn classify_bundle_id(identifier: Option<&str>) -> FrontmostApplicationKind {
    match identifier {
        Some(identifier) if is_chromium_browser_bundle_id(identifier) => {
            FrontmostApplicationKind::ChromiumBrowser
        }
        _ => FrontmostApplicationKind::Other,
    }
}

pub(super) struct NativeFrontmostApplication;

impl FrontmostApplication for NativeFrontmostApplication {
    fn kind(&self) -> FrontmostApplicationKind {
        let identifier = NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .and_then(|application| application.bundleIdentifier())
            .map(|identifier| identifier.to_string());

        classify_bundle_id(identifier.as_deref())
    }
}
