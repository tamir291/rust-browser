use std::{
    ffi::c_void,
    fs,
    io::{self, Write},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    ptr, thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};

const CORE: &str = "sing-box-1.14.4-windows-amd64";
const NO_WINDOW: u32 = 0x08000000;
const NEW_GROUP: u32 = 0x00000200;
const INVALID: &str = "Некорректные параметры TUN.";

pub fn defaults() -> Value {
    json!({"autostart":false, "dns":"cloudflare", "ipv6":true, "mtu":1420, "stack":"system"})
}

pub fn presets(protocol: &str, transport: &str) -> Option<Value> {
    let mut node = match protocol {
        "vless" => {
            json!({"type":"vless","server":"server.example","server_port":443,"uuid":"11111111-1111-4111-8111-111111111111","tls":{"enabled":true,"server_name":"server.example"}})
        }
        "hysteria2" => {
            json!({"type":"hysteria2","server":"server.example","server_port":443,"password":"change-me","tls":{"enabled":true,"server_name":"server.example"}})
        }
        "tuic" => {
            json!({"type":"tuic","server":"server.example","server_port":443,"uuid":"11111111-1111-4111-8111-111111111111","password":"change-me","congestion_control":"bbr","tls":{"enabled":true,"server_name":"server.example","alpn":["h3"]}})
        }
        "naive" => {
            json!({"type":"naive","server":"server.example","server_port":443,"username":"change-me","password":"change-me","tls":{"enabled":true,"server_name":"server.example"}})
        }
        "trojan" => {
            json!({"type":"trojan","server":"server.example","server_port":443,"password":"change-me","tls":{"enabled":true,"server_name":"server.example"}})
        }
        _ => return None,
    };
    if matches!(protocol, "vless" | "trojan") {
        if let Some(value) = transport_value(transport, "/", "tunnel", "") {
            node["transport"] = value;
        }
    }
    if protocol == "naive" && transport == "quic" {
        node["quic"] = true.into();
    }
    Some(node)
}

fn transport_value(kind: &str, path: &str, service: &str, host: &str) -> Option<Value> {
    let mut value = match kind {
        "ws" => json!({"type":"ws","path":path}),
        "grpc" => json!({"type":"grpc","service_name":service}),
        "http" | "h2" => json!({"type":"http","path":path}),
        "httpupgrade" => json!({"type":"httpupgrade","path":path}),
        "quic" => json!({"type":"quic"}),
        _ => return None,
    };
    if !host.is_empty() {
        match kind {
            "ws" => value["headers"] = json!({"Host":host}),
            "http" | "h2" => value["host"] = json!([host]),
            "httpupgrade" => value["host"] = json!(host),
            _ => (),
        }
    }
    Some(value)
}

fn external_file_option(value: &Value) -> bool {
    match value {
        Value::Object(fields) => fields
            .iter()
            .any(|(key, value)| key.ends_with("_path") || external_file_option(value)),
        Value::Array(items) => items.iter().any(external_file_option),
        _ => false,
    }
}

pub fn config(options: &Value) -> Result<Value, &'static str> {
    if external_file_option(&options["node"]) {
        return Err(
            "Для TLS используй встроенные данные сертификата. Ссылки на локальные файлы в узле отключены.",
        );
    }
    let mut node = options["node"].as_object().cloned().ok_or(INVALID)?;
    let protocol = node.get("type").and_then(Value::as_str).ok_or(INVALID)?;
    if !matches!(
        protocol,
        "vless" | "hysteria2" | "tuic" | "naive" | "trojan"
    ) || node
        .get("server")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
        || !node
            .get("server_port")
            .and_then(Value::as_u64)
            .is_some_and(|port| (1..=65535).contains(&port))
    {
        return Err(INVALID);
    }
    // The elevated core owns routing and listeners. User-editable fields belong to this outbound.
    for field in [
        "detour",
        "domain_resolver",
        "routing_mark",
        "bind_interface",
        "inet4_bind_address",
        "inet6_bind_address",
    ] {
        node.remove(field);
    }
    node.insert("tag".into(), "vpn".into());
    node.insert("domain_resolver".into(), "bootstrap".into());
    let mtu = options["mtu"].as_u64().unwrap_or(1420);
    if !(1280..=9000).contains(&mtu) {
        return Err("MTU должен быть от 1280 до 9000.");
    }
    let ipv6 = options["ipv6"].as_bool().unwrap_or(true);
    let stack = options["stack"].as_str().unwrap_or("system");
    if !matches!(stack, "system" | "mixed" | "gvisor") {
        return Err(INVALID);
    }
    let (default_ip, default_name) = match options["dns"].as_str().unwrap_or("cloudflare") {
        "quad9" => ("9.9.9.9", "dns.quad9.net"),
        "google" => ("8.8.8.8", "dns.google"),
        "cloudflare" | "custom" => ("1.1.1.1", "cloudflare-dns.com"),
        _ => return Err(INVALID),
    };
    let ip = options["dns_ip"]
        .as_str()
        .filter(|text| !text.is_empty())
        .unwrap_or(default_ip);
    let name = options["dns_name"]
        .as_str()
        .filter(|text| !text.is_empty())
        .unwrap_or(default_name);
    if ip.parse::<std::net::IpAddr>().is_err()
        || !name.contains('.')
        || name.len() > 253
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
    {
        return Err("DNS требует числовой IP и имя TLS-сервера.");
    }
    let mode = options["dns_transport"].as_str().unwrap_or("https");
    if !matches!(mode, "https" | "tls") {
        return Err(INVALID);
    }
    let port = if mode == "https" { 443 } else { 853 };
    let addresses = json!(["172.31.253.1/30", "fd8a:9b31:253::1/126"]);
    let strategy = if ipv6 { "prefer_ipv4" } else { "ipv4_only" };
    let mut rules = vec![
        json!({"port":53,"action":"hijack-dns"}),
        json!({"action":"sniff"}),
        json!({"protocol":"dns","action":"hijack-dns"}),
    ];
    if !ipv6 {
        rules.push(json!({"ip_version":6,"action":"reject"}));
    }
    let mut result = json!({
        "log":{"disabled":true},
        "inbounds":[{"type":"tun","tag":"tun","interface_name":"RustBrowserTun","address":addresses,
            "mtu":mtu,"auto_route":true,"strict_route":true,"dns_mode":"hijack","stack":stack,
            "route_address":["0.0.0.0/1","128.0.0.0/1","::/1","8000::/1"]}],
        "outbounds":[Value::Object(node)],
        "dns":{"servers":[
            {"type":mode,"tag":"protected","server":ip,"server_port":port,
                "detour":"vpn","tls":{"enabled":true,"server_name":name}},
            {"type":mode,"tag":"bootstrap","server":ip,"server_port":port,
                "tls":{"enabled":true,"server_name":name}}
        ],"final":"protected","strategy":strategy},
        "route":{"auto_detect_interface":true,"default_domain_resolver":"bootstrap","final":"vpn","rules":rules}
    });
    if let Some(adapter) = options["interface"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
    {
        if adapter.len() > 128 || adapter.chars().any(char::is_control) {
            return Err("Некорректное имя интерфейса.");
        }
        result["route"]["auto_detect_interface"] = false.into();
        result["route"]["default_interface"] = adapter.into();
    }
    Ok(result)
}

fn core() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .map(|parent| parent.join("toolchain").join(CORE).join("sing-box.exe"))
        .find(|path| path.is_file())
}

pub fn available() -> bool {
    core().is_some()
}

fn launch_core(mode: &str, config: &Value, flags: u32) -> io::Result<Child> {
    let exe = core().ok_or_else(|| io::Error::other("sing-box missing"))?;
    let mut child = Command::new(&exe)
        .args([mode, "-c", "stdin"])
        .current_dir(exe.parent().unwrap_or(Path::new(".")))
        .creation_flags(flags)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let mut input = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("stdin unavailable"))?;
    let data = serde_json::to_vec(config).map_err(io::Error::other)?;
    if let Err(error) = input.write_all(&data) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    Ok(child)
}

pub fn check(options: &Value) -> Result<(), &'static str> {
    let config = config(options)?;
    let mut child = launch_core("check", &config, NO_WINDOW)
        .map_err(|_| "Не удалось запустить проверку sing-box.")?;
    let status = child
        .wait()
        .map_err(|_| "Не удалось проверить конфигурацию.")?;
    if status.success() {
        Ok(())
    } else {
        Err("sing-box отклонил конфигурацию. Проверь протокол, транспорт и параметры TLS.")
    }
}

pub fn load(profile: &Path) -> Value {
    fs::read(profile.join("tun-settings.dpapi"))
        .ok()
        .and_then(|data| protect(&data, false).ok())
        .and_then(|data| serde_json::from_slice(&data).ok())
        .unwrap_or_else(defaults)
}

pub fn save(profile: &Path, value: &Value) -> io::Result<()> {
    let path = profile.join("tun-settings.dpapi");
    let temporary = profile.join("tun-settings.pending");
    let data = serde_json::to_vec(value).map_err(io::Error::other)?;
    fs::write(&temporary, protect(&data, true)?)?;
    fs::rename(temporary, path)
}

pub struct Session {
    path: PathBuf,
    started: Instant,
}

impl Session {
    pub fn start(profile: &Path, options: &Value) -> Result<Self, &'static str> {
        real_node(options)?;
        config(options)?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = profile.join(format!("tun-run-{}-{stamp}", std::process::id()));
        fs::create_dir(&path).map_err(|_| "Не удалось создать сеанс TUN.")?;
        let data = serde_json::to_vec(options).map_err(|_| INVALID)?;
        let encrypted = protect(&data, true).map_err(|_| "Не удалось защитить конфигурацию.")?;
        fs::write(path.join("config.dpapi"), encrypted)
            .map_err(|_| "Не удалось записать конфигурацию.")?;
        let exe = std::env::current_exe().map_err(|_| "Не найден исполняемый файл.")?;
        let args = format!("--tun-worker \"{}\" {}", path.display(), std::process::id());
        let result = unsafe {
            ShellExecuteW(
                0,
                wide("runas").as_ptr(),
                wide(&exe.to_string_lossy()).as_ptr(),
                wide(&args).as_ptr(),
                ptr::null(),
                0,
            )
        };
        if result <= 32 {
            let _ = fs::remove_file(path.join("config.dpapi"));
            let _ = fs::remove_dir(&path);
            return Err("Windows не запустила TUN с правами администратора.");
        }
        Ok(Self {
            path,
            started: Instant::now(),
        })
    }

    pub fn state(&self) -> String {
        fs::read_to_string(self.path.join("status")).unwrap_or_else(|_| {
            if self.started.elapsed() < Duration::from_secs(40) {
                "starting"
            } else {
                "error"
            }
            .into()
        })
    }

    pub fn stop(&self) {
        let _ = fs::write(self.path.join("stop"), b"1");
    }
}

pub fn real_node(options: &Value) -> Result<(), &'static str> {
    let host = options["node"]["server"].as_str().unwrap_or("").trim();
    if host.is_empty()
        || host
            .to_ascii_lowercase()
            .trim_end_matches('.')
            .ends_with(".example")
    {
        return Err("Вставь подписку или ссылку на рабочий узел. Пресет содержит пример адреса.");
    }
    Ok(())
}

fn physical_interface() -> Option<String> {
    let script = concat!(
        "$n=@(Get-NetAdapter -Physical | Where-Object Status -eq Up | ForEach-Object ifIndex);",
        "$r=Get-NetRoute -AddressFamily IPv4 -DestinationPrefix '0.0.0.0/0' | ",
        "Where-Object { $n -contains $_.InterfaceIndex } | ",
        "Sort-Object @{Expression={$_.RouteMetric + (Get-NetIPInterface -InterfaceIndex $_.InterfaceIndex -AddressFamily IPv4).InterfaceMetric}} | ",
        "Select-Object -First 1 -ExpandProperty InterfaceAlias;",
        "[Console]::OutputEncoding=[Text.Encoding]::UTF8; if($r){[Console]::Write($r)}"
    );
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(NO_WINDOW)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let name = String::from_utf8(output.stdout).ok()?;
    (output.status.success() && !name.trim().is_empty()).then(|| name.trim().to_owned())
}

fn with_interface(mut options: Value) -> Value {
    if options["interface"].as_str().is_none_or(str::is_empty) {
        if let Some(interface) = physical_interface() {
            options["interface"] = interface.into();
        }
    }
    options
}

pub fn prepare(options: Value) -> Result<Value, &'static str> {
    real_node(&options)?;
    let options = with_interface(options);
    probe(&options)?;
    Ok(options)
}

fn probe(options: &Value) -> Result<(), &'static str> {
    use std::net::{Ipv4Addr, TcpListener, TcpStream};
    real_node(options)?;
    let mut configuration = config(options)?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .map_err(|_| "Не удалось открыть локальную проверку.")?;
    let endpoint = listener
        .local_addr()
        .map_err(|_| "Не удалось узнать адрес проверки.")?;
    configuration["inbounds"] =
        json!([{"type":"mixed","tag":"probe","listen":"127.0.0.1","listen_port":endpoint.port()}]);
    configuration["route"]["rules"] = json!([]);
    // This core has a loopback listener only, so probing does not change Windows routes.
    drop(listener);
    let mut child = launch_core("run", &configuration, NO_WINDOW)
        .map_err(|_| "Не удалось запустить проверочный процесс sing-box.")?;
    let result = (|| {
        for _ in 0..25 {
            if child.try_wait().ok().flatten().is_some() {
                return Err("Проверочный процесс sing-box завершился. Проверь параметры узла.");
            }
            if TcpStream::connect_timeout(&endpoint, Duration::from_millis(100)).is_ok() {
                break;
            }
            thread::sleep(Duration::from_millis(65));
        }
        let proxy = format!("socks5h://{endpoint}");
        let output = Command::new("curl.exe")
            .args([
                "-q",
                "--silent",
                "--fail",
                "--noproxy",
                "",
                "--proxy",
                &proxy,
                "--connect-timeout",
                "3",
                "--max-time",
                "6",
                "--output",
                "NUL",
                "--write-out",
                "%{http_code}",
                "https://www.gstatic.com/generate_204",
            ])
            .creation_flags(NO_WINDOW)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map_err(|_| "Не удалось проверить соединение.")?;
        if output.status.success() && output.stdout == b"204" {
            Ok(())
        } else {
            Err("Узел не прошёл сетевую проверку. Маршруты Windows не менялись.")
        }
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}

pub fn select_working(input: &str, options: Value) -> Result<Value, &'static str> {
    let items = import(input)?;
    let options = with_interface(options);
    thread::scope(|scope| {
        for chunk in items.chunks(3).take(3) {
            let jobs: Vec<_> = chunk
                .iter()
                .map(|(label, node)| {
                    let mut candidate = options.clone();
                    candidate["node"] = node.clone();
                    candidate["display_name"] = label.clone().into();
                    scope.spawn(move || probe(&candidate).ok().map(|()| candidate))
                })
                .collect();
            if let Some(candidate) = jobs
                .into_iter()
                .filter_map(|job| job.join().ok().flatten())
                .next()
            {
                return Ok(candidate);
            }
        }
        Err("Ни один из проверенных узлов не ответил. Проверь подписку или выбери узел вручную.")
    })
}

pub fn worker(path: &Path, parent: u32) -> io::Result<()> {
    let result = run_worker(path, parent);
    let _ = fs::remove_file(path.join("config.dpapi"));
    let _ = fs::write(
        path.join("status"),
        match &result {
            Ok(()) => "stopped",
            Err(error) if error.kind() == io::ErrorKind::TimedOut => "dns-error",
            Err(_) => "error",
        },
    );
    result
}

fn run_worker(path: &Path, parent: u32) -> io::Result<()> {
    let parent = Handle(unsafe { OpenProcess(0x00100000, 0, parent) });
    if parent.0 == 0 {
        return Err(io::Error::last_os_error());
    }
    let lock = Handle(unsafe {
        CreateMutexW(ptr::null_mut(), 0, wide("Local\\RustBrowser-TUN").as_ptr())
    });
    if lock.0 == 0 || unsafe { WaitForSingleObject(lock.0, 0) } > 0x80 {
        return Err(io::Error::other("TUN already in use"));
    }
    let data = protect(&fs::read(path.join("config.dpapi"))?, false)?;
    let options: Value = serde_json::from_slice(&data).map_err(io::Error::other)?;
    real_node(&options).map_err(io::Error::other)?;
    let configuration = config(&options).map_err(io::Error::other)?;
    if unsafe { GetConsoleWindow() } == 0 {
        unsafe {
            AllocConsole();
        }
    }
    let console = unsafe { GetConsoleWindow() };
    if console != 0 {
        unsafe {
            ShowWindow(console, 0);
        }
    }
    let mut child = launch_core("run", &configuration, NEW_GROUP)?;
    let started = Instant::now();
    let mut reported = false;
    let mut last_check = Instant::now();
    let mut timed_out = false;
    loop {
        if child.try_wait()?.is_some() {
            return Err(io::Error::other("sing-box exited"));
        }
        if path.join("stop").exists() || unsafe { WaitForSingleObject(parent.0, 0) } != 0x102 {
            break;
        }
        if !reported && last_check.elapsed() >= Duration::from_secs(2) {
            if adapter_ready() {
                let _ = fs::write(path.join("status"), "running");
                reported = true;
            } else if started.elapsed() >= Duration::from_secs(12) {
                timed_out = true;
                break;
            }
            last_check = Instant::now();
        }
        thread::sleep(Duration::from_millis(250));
    }
    unsafe {
        GenerateConsoleCtrlEvent(1, child.id());
    }
    for _ in 0..20 {
        if child.try_wait()?.is_some() {
            return if timed_out {
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "TUN DNS not responding",
                ))
            } else {
                Ok(())
            };
        }
        thread::sleep(Duration::from_millis(150));
    }
    child.kill()?;
    child.wait()?;
    if timed_out {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "TUN DNS not responding",
        ))
    } else {
        Ok(())
    }
}

fn adapter_ready() -> bool {
    let script = concat!(
        "$adapter=Get-NetAdapter -Name RustBrowserTun -ErrorAction SilentlyContinue;",
        "if($adapter.Status -ne 'Up'){exit 1};",
        "$routes=Find-NetRoute -RemoteIPAddress 172.31.253.2 -ErrorAction SilentlyContinue;",
        "if(!($routes | Where-Object InterfaceIndex -eq $adapter.ifIndex)){exit 1};",
        "$internet=Find-NetRoute -RemoteIPAddress 8.8.8.8 -ErrorAction SilentlyContinue;",
        "if(!($internet | Where-Object InterfaceIndex -eq $adapter.ifIndex)){exit 1};",
        "$answer=Resolve-DnsName -Name example.com -Type A -Server 172.31.253.2 -DnsOnly -NoHostsFile -QuickTimeout -ErrorAction SilentlyContinue;",
        "if($answer | Where-Object IPAddress){exit 0}; exit 1"
    );
    Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

struct Handle(isize);
impl Drop for Handle {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[repr(C)]
struct Blob {
    len: u32,
    ptr: *mut u8,
}

fn protect(input: &[u8], encrypt: bool) -> io::Result<Vec<u8>> {
    let source = Blob {
        len: u32::try_from(input.len()).map_err(io::Error::other)?,
        ptr: input.as_ptr().cast_mut(),
    };
    let mut output = Blob {
        len: 0,
        ptr: ptr::null_mut(),
    };
    let result = unsafe {
        if encrypt {
            CryptProtectData(
                &source,
                ptr::null(),
                ptr::null(),
                ptr::null_mut(),
                ptr::null_mut(),
                1,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &source,
                ptr::null_mut(),
                ptr::null(),
                ptr::null_mut(),
                ptr::null_mut(),
                1,
                &mut output,
            )
        }
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    let data = unsafe { std::slice::from_raw_parts(output.ptr, output.len as usize).to_vec() };
    unsafe {
        LocalFree(output.ptr.cast());
    }
    Ok(data)
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

#[link(name = "shell32")]
unsafe extern "system" {
    fn ShellExecuteW(
        window: isize,
        verb: *const u16,
        file: *const u16,
        parameters: *const u16,
        directory: *const u16,
        show: i32,
    ) -> isize;
}
#[link(name = "crypt32")]
unsafe extern "system" {
    fn CryptProtectData(
        src: *const Blob,
        desc: *const u16,
        entropy: *const Blob,
        reserved: *mut c_void,
        prompt: *mut c_void,
        flags: u32,
        dst: *mut Blob,
    ) -> i32;
    fn CryptUnprotectData(
        src: *const Blob,
        desc: *mut *mut u16,
        entropy: *const Blob,
        reserved: *mut c_void,
        prompt: *mut c_void,
        flags: u32,
        dst: *mut Blob,
    ) -> i32;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn LocalFree(ptr: *mut c_void) -> *mut c_void;
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
    fn CreateMutexW(attrs: *mut c_void, owner: i32, name: *const u16) -> isize;
    fn WaitForSingleObject(handle: isize, ms: u32) -> u32;
    fn CloseHandle(handle: isize) -> i32;
    fn GetConsoleWindow() -> isize;
    fn AllocConsole() -> i32;
    fn GenerateConsoleCtrlEvent(kind: u32, process_group: u32) -> i32;
}
#[link(name = "user32")]
unsafe extern "system" {
    fn ShowWindow(window: isize, show: i32) -> i32;
}

pub fn import(text: &str) -> Result<Vec<(String, Value)>, &'static str> {
    use base64::{Engine, engine::general_purpose};
    let text = text.trim();
    let owned;
    let text = if let Ok(url) = url::Url::parse(text) {
        if url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && !text.contains(char::is_whitespace)
        {
            let mut curl = Command::new("curl.exe")
                .args([
                    "-q",
                    "-K",
                    "-",
                    "--silent",
                    "--fail",
                    "--proto",
                    "=https",
                    "--max-time",
                    "15",
                    "--max-filesize",
                    "524288",
                ])
                .creation_flags(NO_WINDOW)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|_| "Не удалось открыть подписку.")?;
            let mut stdin = curl.stdin.take().ok_or("Не удалось открыть подписку.")?;
            writeln!(stdin, "url = \"{}\"", url).map_err(|_| "Не удалось открыть подписку.")?;
            drop(stdin);
            let output = curl
                .wait_with_output()
                .map_err(|_| "Не удалось прочитать подписку.")?;
            if !output.status.success() {
                return Err("HTTPS-подписка недоступна.");
            }
            owned = String::from_utf8(output.stdout)
                .map_err(|_| "Формат подписки не поддерживается.")?;
            owned.trim()
        } else {
            text
        }
    } else {
        text
    };
    let decoded;
    let text = if !text.contains("://") && !text.trim_start().starts_with(['{', '[']) {
        let compact: String = text
            .chars()
            .filter(|ch| !ch.is_ascii_whitespace())
            .collect();
        let bytes = [
            &general_purpose::STANDARD,
            &general_purpose::STANDARD_NO_PAD,
            &general_purpose::URL_SAFE,
            &general_purpose::URL_SAFE_NO_PAD,
        ]
        .into_iter()
        .find_map(|engine| engine.decode(&compact).ok())
        .ok_or("Формат подписки не поддерживается.")?;
        decoded = String::from_utf8(bytes).map_err(|_| "Формат подписки не поддерживается.")?;
        decoded.as_str()
    } else {
        text
    };
    let nodes: Vec<_> = if let Ok(value) = serde_json::from_str::<Value>(text) {
        let items = value.as_array().or_else(|| value["outbounds"].as_array());
        match items {
            Some(items) => items.iter().filter_map(outbound).take(100).collect(),
            None => outbound(&value).into_iter().collect(),
        }
    } else {
        text.lines().filter_map(parse_uri).take(100).collect()
    };
    if nodes.is_empty() {
        Err("В подписке нет совместимых узлов.")
    } else {
        Ok(nodes)
    }
}

fn outbound(value: &Value) -> Option<(String, Value)> {
    let protocol = value["type"].as_str()?;
    presets(protocol, "").filter(|_| value["server"].is_string())?;
    Some((
        value["tag"]
            .as_str()
            .unwrap_or(protocol)
            .chars()
            .take(80)
            .collect(),
        value.clone(),
    ))
}

fn parse_uri(text: &str) -> Option<(String, Value)> {
    let url = url::Url::parse(text.trim()).ok()?;
    let protocol = match url.scheme() {
        "hy2" | "hysteria2" => "hysteria2",
        "naive+https" | "naive+quic" | "naive" => "naive",
        "vless" => "vless",
        "tuic" => "tuic",
        "trojan" => "trojan",
        _ => return None,
    };
    let parse_decoded = |input: &str| {
        let encoded = input.replace('+', "%2B").replace('&', "%26");
        url::form_urlencoded::parse(encoded.as_bytes())
            .next()
            .map(|(value, _)| value.into_owned())
            .unwrap_or_default()
    };
    let host = url.host_str()?.trim_matches(['[', ']']);
    let param = |key: &str| {
        url.query_pairs()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.into_owned())
    };
    let transport = param("type")
        .or_else(|| param("transport"))
        .unwrap_or_default();
    if matches!(protocol, "vless" | "trojan")
        && !matches!(
            transport.as_str(),
            "" | "tcp" | "raw" | "ws" | "grpc" | "h2" | "http" | "httpupgrade" | "quic"
        )
    {
        return None;
    }
    let mut node = presets(protocol, &transport)?;
    node["server"] = host.into();
    node["server_port"] = url.port().unwrap_or(443).into();
    let user = parse_decoded(url.username());
    let password = url.password().map(parse_decoded).unwrap_or_default();
    match protocol {
        "vless" => node["uuid"] = user.into(),
        "tuic" => {
            node["uuid"] = user.into();
            node["password"] = password.into();
            if let Some(cc) = param("congestion_control") {
                node["congestion_control"] = cc.into();
            }
        }
        "naive" => {
            node["username"] = user.into();
            node["password"] = password.into();
            if url.scheme() == "naive+quic" {
                node["quic"] = true.into();
            }
        }
        _ => {
            node["password"] = if password.is_empty() {
                user
            } else {
                format!("{user}:{password}")
            }
            .into()
        }
    }
    if protocol == "vless" {
        if let Some(flow) = param("flow") {
            node["flow"] = flow.into();
        }
    }
    if protocol == "hysteria2" {
        if let (Some(kind), Some(password)) = (param("obfs"), param("obfs-password")) {
            node["obfs"] = json!({"type":kind,"password":password});
        }
    }
    node["tls"]["server_name"] = param("sni")
        .or_else(|| param("serverName"))
        .unwrap_or_else(|| host.into())
        .into();
    if let Some(alpn) = param("alpn") {
        node["tls"]["alpn"] = json!(alpn.split(',').collect::<Vec<_>>());
    }
    if let Some(fingerprint) = param("fp") {
        node["tls"]["utls"] = json!({"enabled":true,"fingerprint":fingerprint});
    }
    match param("security").as_deref() {
        Some("none") if matches!(protocol, "vless" | "trojan") => {
            node["tls"] = json!({"enabled":false})
        }
        Some("reality") => {
            let key = param("pbk")?;
            node["tls"]["reality"] = json!({"enabled":true,"public_key":key,"short_id":param("sid").unwrap_or_default()});
        }
        _ => (),
    }
    if matches!(protocol, "vless" | "trojan") {
        let path = param("path").unwrap_or_else(|| "/".into());
        let service = param("serviceName")
            .or_else(|| param("service_name"))
            .unwrap_or_default();
        let authority = param("host").unwrap_or_default();
        if let Some(value) = transport_value(&transport, &path, &service, &authority) {
            node["transport"] = value;
        }
    }
    let title = url
        .fragment()
        .map(parse_decoded)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{protocol} · {host}"));
    Some((title.chars().take(80).collect(), node))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_pass_the_windows_core_check() {
        assert!(
            available(),
            "install the optional sing-box Windows core before this test"
        );
        for (protocol, transport) in [
            ("vless", "tcp"),
            ("vless", "ws"),
            ("vless", "grpc"),
            ("trojan", "httpupgrade"),
            ("hysteria2", ""),
            ("tuic", ""),
            ("naive", "tcp"),
            ("naive", "quic"),
        ] {
            let mut options = defaults();
            options["node"] = presets(protocol, transport).unwrap();
            assert_eq!(check(&options), Ok(()), "{protocol}/{transport}");
            let config = config(&options).unwrap();
            assert_eq!(config["inbounds"][0]["strict_route"], true);
            assert_eq!(config["dns"]["servers"][0]["detour"], "vpn");
            assert_eq!(config["route"]["final"], "vpn");
        }
        let mut options = defaults();
        options["node"] = presets("vless", "tcp").unwrap();
        options["ipv6"] = false.into();
        assert_eq!(check(&options), Ok(()));
        assert_eq!(
            config(&options).unwrap()["route"]["rules"][3]["action"],
            "reject"
        );
    }

    #[test]
    fn import_parses_sharing_uris_and_dpapi_round_trips() {
        let node = import("vless://11111111-1111-4111-8111-111111111111@server.example:443?type=ws&path=%2Fsocket&sni=server.example#Test").unwrap();
        assert_eq!(node[0].0, "Test");
        assert_eq!(node[0].1["transport"]["path"], "/socket");
        let plaintext = serde_json::to_vec(&node[0].1).unwrap();
        let protected = protect(&plaintext, true).unwrap();
        assert_ne!(protected, plaintext);
        assert_eq!(protect(&protected, false).unwrap(), plaintext);
    }
}
