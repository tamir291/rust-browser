use serde_json::{Value, json};
use std::{fs, net::IpAddr, path::Path};
use url::{Host, Url};

pub fn defaults() -> Value {
    json!({
        "http1": false, "quic": false, "protection": false,
        "network": {"locked": false, "proxy": "", "dns": {"mode": "upstream", "address": "1.1.1.1:853", "server_name": "cloudflare-dns.com", "family": "dual", "require_ad": false}},
        "appearance": {"preset": "amber", "motion": true, "accent": "", "surface": "", "font": "pixel"},
        "extensions": {"disabled": []}
    })
}

#[derive(Clone)]
pub enum Route {
    System,
    Proxy(String),
    DnsProxy(String),
    Offline,
}

impl Route {
    pub fn read(settings: &Value) -> Self {
        let network = &settings["network"];
        if let Some(network) = settings.get("network") {
            if !network.is_object()
                || network
                    .get("locked")
                    .is_some_and(|value| !value.is_boolean())
                || network.get("proxy").is_some_and(|value| !value.is_string())
            {
                return Self::Offline;
            }
        }
        let locked = network["locked"].as_bool().unwrap_or(false);
        let proxy = network["proxy"].as_str().unwrap_or("").trim();
        if proxy.is_empty() {
            return if locked { Self::Offline } else { Self::System };
        }
        proxy_endpoint(proxy).map_or(Self::Offline, Self::Proxy)
    }

    pub fn apply(&self, flags: &mut String) {
        if let Self::Proxy(address) | Self::DnsProxy(address) = self {
            let proxy_ip = Url::parse(address)
                .ok()
                .and_then(|url| url.host_str().map(str::to_owned))
                .unwrap_or_default();
            flags.push_str(&format!(
                " --proxy-server={address} --proxy-bypass-list=<-loopback> --host-resolver-rules=\"MAP * ~NOTFOUND, EXCLUDE {proxy_ip}\" --force-webrtc-ip-handling-policy=disable_non_proxied_udp --disable-quic"
            ));
        }
        if matches!(self, Self::Offline) {
            flags.push_str(" --proxy-server=http://127.0.0.1:1 --host-resolver-rules=\"MAP * ~NOTFOUND\" --disable-quic");
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::System => "СИСТЕМА",
            Self::Proxy(_) => "ПРОКСИ",
            Self::DnsProxy(_) => "DNS / TLS",
            Self::Offline => "БЛОК",
        }
    }
}

fn proxy_endpoint(input: &str) -> Option<String> {
    let url = Url::parse(input).ok()?;
    if !matches!(url.scheme(), "http" | "socks5")
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return None;
    }
    let host = match url.host()? {
        Host::Ipv4(ip) => ip.to_string(),
        Host::Ipv6(ip) => format!("[{ip}]"),
        Host::Domain(ip) => ip.parse::<IpAddr>().ok()?.to_string(),
    };
    let port = url.port_or_known_default().filter(|port| *port != 0)?;
    Some(format!("{}://{host}:{port}", url.scheme()))
}

pub fn load(path: &Path) -> Value {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str::<Value>(&text)
            .ok()
            .filter(Value::is_object)
            .unwrap_or_else(|| {
                let mut fallback = defaults();
                fallback["network"]["locked"] = true.into();
                fallback
            }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => defaults(),
        Err(_) => {
            let mut fallback = defaults();
            fallback["network"]["locked"] = true.into();
            fallback
        }
    }
}

pub fn validate(value: &Value) -> Result<Value, &'static str> {
    if !value.is_object() {
        return Err("Нужен JSON-объект.");
    }
    if let Some(network) = value.get("network") {
        if !network.is_object()
            || network
                .get("locked")
                .is_some_and(|value| !value.is_boolean())
            || network.get("proxy").is_some_and(|value| !value.is_string())
        {
            return Err("Некорректная конфигурация сети.");
        }
    }
    let mut result = defaults();
    for key in ["http1", "quic", "protection"] {
        result[key] = value[key].as_bool().unwrap_or(false).into();
    }
    let locked = value["network"]["locked"].as_bool().unwrap_or(false);
    let raw = value["network"]["proxy"].as_str().unwrap_or("").trim();
    let proxy = if raw.is_empty() {
        String::new()
    } else {
        proxy_endpoint(raw)
            .ok_or("Укажи http://IP:порт или socks5://IP:порт без логина и пароля.")?
    };
    let dns = &value["network"]["dns"];
    let policy = crate::dns::Config::parse(dns)?;
    if policy.is_some() && !proxy.starts_with("socks5://") {
        return Err("Для собственного DNS по TLS укажи SOCKS5-прокси.");
    }
    let dns = policy
        .map(|config| config.settings())
        .unwrap_or_else(|| defaults()["network"]["dns"].clone());
    result["network"] =
        json!({"locked": locked || policy_enabled(&dns), "proxy": proxy, "dns": dns});
    let preset = match value["appearance"]["preset"].as_str() {
        Some("phosphor") => "phosphor",
        Some("mono") => "mono",
        _ => "amber",
    };
    result["appearance"] = json!({"preset": preset, "motion": value["appearance"]["motion"].as_bool().unwrap_or(true)});
    result["appearance"]["font"] = match value["appearance"]["font"].as_str() {
        Some("console") => "console",
        Some("typewriter") => "typewriter",
        Some("compact") => "compact",
        _ => "pixel",
    }
    .into();
    for key in ["accent", "surface"] {
        let color = value["appearance"][key].as_str().unwrap_or("").trim();
        if !color.is_empty() && !hex_color(color) {
            return Err("Цвет должен быть задан в формате #RRGGBB.");
        }
        result["appearance"][key] = color.into();
    }
    if let Some(disabled) = value["extensions"]["disabled"].as_array() {
        result["extensions"]["disabled"] = Value::Array(
            disabled
                .iter()
                .filter_map(|id| id.as_str())
                .filter(|id| id.len() <= 200)
                .take(100)
                .map(Value::from)
                .collect(),
        );
    }
    Ok(result)
}

fn policy_enabled(dns: &Value) -> bool {
    dns["mode"] == "tls"
}

pub fn font(value: &Value) -> &'static str {
    match value["appearance"]["font"].as_str() {
        Some("console") => "Lucida Console",
        Some("typewriter") => "Courier New",
        Some("compact") => "Consolas",
        _ => "Pixelify",
    }
}

fn hex_color(color: &str) -> bool {
    color.len() == 7
        && color.starts_with('#')
        && color.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
}

pub fn theme(value: &Value) -> &str {
    if let Some(color) = value["appearance"]["accent"]
        .as_str()
        .filter(|color| hex_color(color))
    {
        return color;
    }
    match value["appearance"]["preset"].as_str() {
        Some("phosphor") => "#a8c89d",
        Some("mono") => "#c4c7c4",
        _ => "#dfbc72",
    }
}

pub fn surface(value: &Value) -> &str {
    if let Some(color) = value["appearance"]["surface"]
        .as_str()
        .filter(|color| hex_color(color))
    {
        return color;
    }
    match value["appearance"]["preset"].as_str() {
        Some("phosphor") => "#0f160f",
        Some("mono") => "#151515",
        _ => "#101310",
    }
}

pub fn save(path: &Path, value: &Value) -> std::io::Result<()> {
    let temporary = path.with_extension("json.pending");
    let output = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    fs::write(&temporary, output)?;
    fs::rename(temporary, path)
}
