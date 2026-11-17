//! Runtime locations (same layout as the Swift `RuntimePaths`).
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Paths {
    /// contains kernel/, sandbox/, web/, skills/, prompts/
    pub resources: PathBuf,
    /// contains bin/python3
    pub python_root: PathBuf,
    /// ~/Library/Application Support/DataPilot
    pub app_support: PathBuf,
}

impl Paths {
    pub fn new(resources: PathBuf, python_root: PathBuf, app_support: PathBuf) -> Self {
        let c = |p: PathBuf| crate::platform::canonicalize(&p).unwrap_or(p);
        Paths { resources: c(resources), python_root: c(python_root), app_support }
    }
    pub fn python(&self) -> PathBuf { crate::platform::python_exe(&self.python_root) }
    pub fn kernel_script(&self) -> PathBuf { self.resources.join("kernel/kernel.py") }
    pub fn sessions_dir(&self) -> PathBuf { self.app_support.join("sessions") }
    /// DP_PROMPTS_DIR overrides the prompt folder (A/B evaluation of prompt variants).
    pub fn prompts(&self) -> PathBuf { std::env::var("DP_PROMPTS_DIR").map(PathBuf::from).unwrap_or_else(|_| self.resources.join("prompts")) }
    /// Shared tool contracts: bundled Resources/protocol/ or the repo's protocol/ (development).
    pub fn tool_contracts(&self) -> PathBuf {
        let bundled = self.resources.join("protocol/tools.snapshot.json");
        if bundled.exists() { return bundled; }
        self.resources.parent().map(|p| p.join("protocol/tools.snapshot.json")).unwrap_or(bundled)
    }

    /// Development layout: <repo>/Resources + <repo>/build/vendor/python (DP_REPO or the crate's parent).
    pub fn dev(app_support: PathBuf) -> Self {
        let repo = std::env::var("DP_REPO").map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf());
        Paths::new(repo.join("Resources"), repo.join("build/vendor/python"), app_support)
    }
}
