use std::{collections::HashMap, time::Instant};

use serde_json::{Value, json};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_PROCESS_KIND, COREWEBVIEW2_PROCESS_KIND_BROWSER, COREWEBVIEW2_PROCESS_KIND_GPU,
    COREWEBVIEW2_PROCESS_KIND_RENDERER, COREWEBVIEW2_PROCESS_KIND_UTILITY,
    ICoreWebView2Environment8,
};
use windows::Win32::{
    Foundation::{CloseHandle, FILETIME},
    System::{
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
        Threading::{GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    },
};
use windows_core::Interface;
use wry::{WebView, WebViewExtWindows};

#[derive(Default)]
pub struct Sampler {
    previous: HashMap<u32, u64>,
    sampled_at: Option<Instant>,
}

impl Sampler {
    pub fn sample(&mut self, page: &WebView) -> Value {
        let now = Instant::now();
        let elapsed = self
            .sampled_at
            .replace(now)
            .map(|time| now.duration_since(time).as_secs_f64());
        let mut current = HashMap::new();
        let mut rows = Vec::new();
        let mut total_cpu = 0.0;
        let mut total_ram = 0;
        for (pid, role) in processes(page) {
            let Some((cpu_time, rss)) = process_usage(pid) else {
                continue;
            };
            current.insert(pid, cpu_time);
            let cpu = elapsed
                .zip(self.previous.get(&pid))
                .map_or(0.0, |(seconds, previous)| {
                    cpu_time.saturating_sub(*previous) as f64 / (seconds.max(0.001) * 100_000.0)
                });
            total_cpu += cpu;
            total_ram += rss;
            rows.push(json!([pid, role, cpu, rss]));
        }
        self.previous = current;
        json!({"cpu": total_cpu, "ram": total_ram, "rows": rows})
    }
}

fn processes(page: &WebView) -> Vec<(u32, &'static str)> {
    let mut output = vec![(std::process::id(), "Shell")];
    // The environment belongs to this browser profile and reports its own child processes.
    unsafe {
        let Ok(env) = page.environment().cast::<ICoreWebView2Environment8>() else {
            return output;
        };
        let Ok(infos) = env.GetProcessInfos() else {
            return output;
        };
        let mut count = 0;
        if infos.Count(&mut count).is_err() {
            return output;
        }
        for index in 0..count {
            let Ok(info) = infos.GetValueAtIndex(index) else {
                continue;
            };
            let mut pid = 0;
            let mut kind = COREWEBVIEW2_PROCESS_KIND::default();
            if info.ProcessId(&mut pid).is_err() || info.Kind(&mut kind).is_err() || pid <= 0 {
                continue;
            }
            let role = match kind {
                COREWEBVIEW2_PROCESS_KIND_BROWSER => "Browser",
                COREWEBVIEW2_PROCESS_KIND_GPU => "GPU",
                COREWEBVIEW2_PROCESS_KIND_RENDERER => "Renderer",
                COREWEBVIEW2_PROCESS_KIND_UTILITY => "Utility",
                _ => "Other",
            };
            output.push((pid as u32, role));
        }
    }
    output
}

fn process_usage(pid: u32) -> Option<(u64, usize)> {
    // Close the process handle after reading both counters, even if one query fails.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let [mut creation, mut exit, mut kernel, mut user] = [FILETIME::default(); 4];
        let mut memory = PROCESS_MEMORY_COUNTERS::default();
        let size = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        memory.cb = size;
        let times = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user);
        let usage = GetProcessMemoryInfo(handle, &mut memory, size);
        let _ = CloseHandle(handle);
        times.ok()?;
        usage.ok()?;
        let ticks =
            |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
        Some((ticks(kernel) + ticks(user), memory.WorkingSetSize))
    }
}
