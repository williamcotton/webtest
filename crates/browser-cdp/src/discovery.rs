use std::path::PathBuf;

pub fn find_system_chrome() -> Option<PathBuf> {
    system_chrome_candidates()
        .iter()
        .map(PathBuf::from)
        .find(|candidate| candidate.is_file())
}

fn system_chrome_candidates() -> &'static [&'static str] {
    #[cfg(target_os = "macos")]
    const CANDIDATES: &[&str] = &[
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ];
    #[cfg(target_os = "linux")]
    const CANDIDATES: &[&str] = &[
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
    ];
    #[cfg(target_os = "windows")]
    const CANDIDATES: &[&str] = &[];
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    const CANDIDATES: &[&str] = &[];

    CANDIDATES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_export_and_platform_candidate_order_are_stable() {
        let _: fn() -> Option<PathBuf> = crate::find_system_chrome;
        #[cfg(target_os = "macos")]
        assert_eq!(
            system_chrome_candidates(),
            [
                "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
                "/Applications/Chromium.app/Contents/MacOS/Chromium"
            ]
        );
        #[cfg(target_os = "linux")]
        assert_eq!(
            system_chrome_candidates(),
            [
                "/usr/bin/google-chrome",
                "/usr/bin/google-chrome-stable",
                "/usr/bin/chromium",
                "/usr/bin/chromium-browser"
            ]
        );
        #[cfg(target_os = "windows")]
        assert!(system_chrome_candidates().is_empty());
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        assert!(system_chrome_candidates().is_empty());
    }
}
