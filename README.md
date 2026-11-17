# Rust Browser

Windows prototype using Rust, Tao and WebView2. The local build tools and
browser profile are under this directory.

- Build with `Build.cmd`. Launch using `Open on DISPLAY2.cmd`.
- YouTube on DISPLAY2.cmd opens YouTube in HTTP/1.1 mode using the same profile. Close existing browser windows before changing modes.
- For an alternate start page, pass `--url=https://www.youtube.com/`.
- To opt in to QUIC, close the browser and pass `--quic` on the next launch.
  `--online-protection` opts into WebView2 SmartScreen reputation checks.
  --http1 disables both HTTP/2 and QUIC for network diagnostics.
- Click the CPU and RAM readout to see the process table and resource chart.
  CPU uses one logical core as 100%. RAM is the sum of process working sets
  and may include shared pages more than once. Network figures come from page
  Resource Timing, which can omit cross-origin byte counts, uploads and
  WebView2 background connections.
- `python tests/smoke_display2.py` exercises the browser with a temporary
  loopback site and a separate test profile. The visual test requires DISPLAY2.

The rendering prototype uses the system WebView2 runtime. Microsoft documents
required runtime diagnostics outside the app's control. Profile persistence,
network behavior and comparative video performance should be evaluated
independently. Cookies persist as allowed by each site.

The Modules button discovers local shortcut manifests on demand. To install one,
put a JSON file in `profile/modules` next to the launcher profile. Example:

```json
{"type":"shortcut","title":"Rust documentation","url":"https://doc.rust-lang.org/"}
```

The prototype module type adds navigation commands. General module APIs are
pending.
