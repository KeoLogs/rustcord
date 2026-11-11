//! Process memory numbers and cache counts for Settings → Performance
//! (the dev memory panel) and `scripts/memprobe.ps1`.
//!
//! "Memory" in Task Manager is the private working set; Windows reports it
//! in `PROCESS_MEMORY_COUNTERS_EX2::PrivateWorkingSetSize`. Elsewhere the
//! closest numbers come from `/proc/self/status` (Linux) or
//! `memory_stats` (macOS). Nothing here trims or touches the working set;
//! it only reads.

/// One reading of the process's memory, in bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProcMem {
    /// Task Manager "Memory" (Windows); RssAnon on Linux.
    pub private_ws: Option<u64>,
    /// Working set / resident set size.
    pub working_set: u64,
    /// Private bytes (commit charge) on Windows; `None` elsewhere.
    pub private_bytes: Option<u64>,
}

#[cfg(windows)]
pub fn sample() -> Option<ProcMem> {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
        PROCESS_MEMORY_COUNTERS_EX2,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let process = unsafe { GetCurrentProcess() };
    let mut ex2 = PROCESS_MEMORY_COUNTERS_EX2 {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32,
        ..Default::default()
    };
    // SAFETY: the struct is plain data and `cb` says its size; EX2 starts
    // with the PROCESS_MEMORY_COUNTERS fields.
    let ok = unsafe {
        GetProcessMemoryInfo(
            process,
            (&raw mut ex2).cast::<PROCESS_MEMORY_COUNTERS>(),
            ex2.cb,
        )
    };
    if ok != 0 {
        return Some(ProcMem {
            private_ws: Some(ex2.PrivateWorkingSetSize as u64),
            working_set: ex2.WorkingSetSize as u64,
            private_bytes: Some(ex2.PrivateUsage as u64),
        });
    }
    // Older Windows 10 without EX2: no private working set.
    let mut ex = PROCESS_MEMORY_COUNTERS_EX {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    // SAFETY: as above.
    let ok = unsafe {
        GetProcessMemoryInfo(
            process,
            (&raw mut ex).cast::<PROCESS_MEMORY_COUNTERS>(),
            ex.cb,
        )
    };
    (ok != 0).then(|| ProcMem {
        private_ws: None,
        working_set: ex.WorkingSetSize as u64,
        private_bytes: Some(ex.PrivateUsage as u64),
    })
}

#[cfg(target_os = "linux")]
pub fn sample() -> Option<ProcMem> {
    parse_proc_status(&std::fs::read_to_string("/proc/self/status").ok()?)
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn sample() -> Option<ProcMem> {
    memory_stats::memory_stats().map(|m| ProcMem {
        private_ws: None,
        working_set: m.physical_mem as u64,
        private_bytes: None,
    })
}

/// VmRSS / RssAnon (kB) from `/proc/<pid>/status`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_proc_status(text: &str) -> Option<ProcMem> {
    let kb = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix(':'))
            .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
            .map(|v| v * 1024)
    };
    Some(ProcMem {
        private_ws: kb("RssAnon"),
        working_set: kb("VmRSS")?,
        private_bytes: None,
    })
}

/// What RustCord holds, for the dev memory panel. Filled by the app (cache
/// sizes from `State`) and `fill_egui` (textures and image loaders).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemCounts {
    pub textures: usize,
    pub texture_bytes: usize,
    /// The font atlas texture (egui's managed texture 0).
    pub font_atlas_bytes: usize,
    /// Downloaded image files kept by egui's bytes loaders.
    pub image_file_bytes: usize,
    /// Decoded images kept by egui's image loaders.
    pub image_decoded_bytes: usize,
    pub cached_channels: usize,
    pub cached_messages: usize,
    pub md_cache: usize,
    pub members: usize,
    pub presences: usize,
    pub profiles: usize,
    pub user_names: usize,
}

impl MemCounts {
    /// Texture and image loader numbers from egui.
    pub fn fill_egui(&mut self, ctx: &eframe::egui::Context) {
        {
            let tex = ctx.tex_manager();
            let tex = tex.read();
            self.textures = tex.allocated().len();
            self.texture_bytes = 0;
            self.font_atlas_bytes = 0;
            for (id, meta) in tex.allocated() {
                self.texture_bytes += meta.bytes_used();
                if *id == eframe::egui::TextureId::default() {
                    self.font_atlas_bytes = meta.bytes_used();
                }
            }
        }
        let loaders = ctx.loaders();
        self.image_file_bytes = loaders.bytes.lock().iter().map(|l| l.byte_size()).sum();
        self.image_decoded_bytes = loaders.image.lock().iter().map(|l| l.byte_size()).sum();
    }

    /// (label, value) rows for the panel.
    pub fn rows(&self) -> Vec<(&'static str, String)> {
        vec![
            (
                "Textures",
                format!("{} ({})", self.textures, mb(self.texture_bytes as u64)),
            ),
            ("Font atlas", mb(self.font_atlas_bytes as u64)),
            ("Image files held", mb(self.image_file_bytes as u64)),
            ("Decoded images held", mb(self.image_decoded_bytes as u64)),
            ("Cached channels", self.cached_channels.to_string()),
            ("Cached messages", self.cached_messages.to_string()),
            ("Parsed markdown", self.md_cache.to_string()),
            ("Member cache", self.members.to_string()),
            ("Presences", self.presences.to_string()),
            ("Profiles", self.profiles.to_string()),
            ("Known names", self.user_names.to_string()),
        ]
    }
}

/// Bytes as "12.3 MB".
pub fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_048_576.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_status() {
        let text = "Name:\trustcord\nVmRSS:\t   81920 kB\nRssAnon:\t   40960 kB\n";
        let m = parse_proc_status(text).unwrap();
        assert_eq!(m.working_set, 80 << 20);
        assert_eq!(m.private_ws, Some(40 << 20));
        assert_eq!(m.private_bytes, None);
        assert!(parse_proc_status("Name: x\n").is_none());
    }

    #[test]
    fn sample_reads_something() {
        let m = sample().expect("memory reading");
        assert!(m.working_set > 0);
    }

    #[test]
    fn rows_and_mb() {
        assert_eq!(mb(3 << 20), "3.0 MB");
        let c = MemCounts {
            textures: 2,
            texture_bytes: 1 << 20,
            cached_messages: 50,
            ..Default::default()
        };
        let rows = c.rows();
        assert_eq!(rows[0], ("Textures", "2 (1.0 MB)".to_string()));
        assert!(
            rows.iter()
                .any(|(k, v)| *k == "Cached messages" && v == "50")
        );
    }
}
