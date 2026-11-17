#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod data;
mod dns;
mod modules;
mod resources;
mod settings;
mod tun;

use std::{
    collections::VecDeque,
    error::Error,
    path::PathBuf,
    time::{Duration, Instant},
};
use tao::{
    dpi::{LogicalPosition, LogicalSize, PhysicalPosition},
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    platform::windows::WindowBuilderExtWindows,
    window::{Fullscreen, Icon, Window, WindowBuilder},
};
use url::Url;
use webview2_com::{
    ContainsFullScreenElementChangedEventHandler,
    Microsoft::Web::WebView2::Win32::ICoreWebView2Settings8,
};
use windows_core::{BOOL, Interface};
use wry::{
    NewWindowResponse, Rect, WebContext, WebView, WebViewBuilder, WebViewBuilderExtWindows,
    WebViewExtWindows,
};

const YOUTUBE: &str = "https://www.youtube.com/";
const TOOLBAR: &str = include_str!("toolbar.html");
const MANAGER: &str = include_str!("manager.html");
const MANAGER_URL: &str = "https://manager.browser.local/";
const START: &str = include_str!("start.html");
const SHELL_URL: &str = "https://shell.browser.local/";
const START_URL: &str = "https://home.browser.local/";
const PAGE_SCRIPT: &str = include_str!("page.js");

enum Message {
    Toolbar(String),
    Manage(String),
    TunImported(Result<Vec<(String, serde_json::Value)>, &'static str>),
    TunPrepared(Result<serde_json::Value, &'static str>),
    Page(u64, String),
    Loaded(u64, String),
    Fullscreen(u64, bool),
}

fn local_html(html: &'static str) -> wry::http::Response<std::borrow::Cow<'static, [u8]>> {
    wry::http::Response::builder()
        .header("Content-Type", "text/html; charset=utf-8")
        .header("Content-Security-Policy", "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; font-src 'self'; base-uri 'none'; frame-src 'none'; form-action https://duckduckgo.com")
        .body(std::borrow::Cow::Borrowed(html.as_bytes()))
        .expect("valid local response")
}

fn local_asset(
    path: &str,
    html: &'static str,
) -> wry::http::Response<std::borrow::Cow<'static, [u8]>> {
    match path {
        "/" => local_html(html),
        "/terminal.ttf" => wry::http::Response::builder()
            .header("Content-Type", "font/ttf")
            .body(std::borrow::Cow::<[u8]>::Borrowed(include_bytes!(
                "assets/PixelifySans.ttf"
            )))
            .expect("valid font response"),
        _ => wry::http::Response::builder()
            .status(404)
            .body(std::borrow::Cow::<[u8]>::Borrowed(&[]))
            .expect("valid status"),
    }
}

fn app_icon() -> Result<Icon, tao::window::BadIcon> {
    const SIZE: u32 = 32;
    let mut pixels = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (x, y) = (f64::from(x) - 15.5, f64::from(y) - 15.5);
            let radius = x.hypot(y);
            let axis = x * 0.91 - y * 0.41;
            let normal = x * 0.41 + y * 0.91;
            let ring = (axis / 13.5).hypot(normal / 5.5);
            let pixel = if (x - 7.0).abs() + (y + 5.0).abs() < 3.3 {
                [250, 225, 169, 255]
            } else if (0.84..1.05).contains(&ring) && (radius > 8.5 || normal > 0.0) {
                [225, 181, 99, 255]
            } else if radius < 8.6 && (radius > 7.1 || (x as i32 + y as i32) % 4 == 0) {
                [181, 145, 85, 255]
            } else if x.abs().max(y.abs()) < 14.0 && x.abs() + y.abs() < 24.5 {
                [23, 26, 21, 255]
            } else {
                [0, 0, 0, 0]
            };
            pixels.extend(pixel);
        }
    }
    Icon::from_rgba(pixels, SIZE, SIZE)
}

fn web_url(input: &str) -> Option<Url> {
    let input = input.trim();
    if input.is_empty() || input.chars().any(char::is_whitespace) {
        return None;
    }
    let url = if input.contains("://") {
        Url::parse(input)
    } else {
        Url::parse(&format!("https://{input}"))
    }
    .ok()?;
    (matches!(url.scheme(), "https" | "http") && url.host().is_some()).then_some(url)
}

fn resize(window: &Window, toolbar: &WebView, page: &WebView, expanded: bool) {
    let size = window.inner_size().to_logical::<f64>(window.scale_factor());
    let full = window.fullscreen().is_some();
    let bar: f64 = if full {
        0.0
    } else if expanded {
        349.0
    } else {
        84.0
    };
    let bar = bar.min((size.height - 1.0).max(0.0));
    let bounds = |y: f64, height: f64| Rect {
        position: LogicalPosition::new(0.0, y).into(),
        size: LogicalSize::new(size.width.max(1.0), height.max(1.0)).into(),
    };
    let _ = toolbar.set_visible(!full);
    let _ = toolbar.set_bounds(bounds(0.0, bar));
    let _ = page.set_bounds(bounds(bar, size.height - bar));
}

struct Tab {
    id: u64,
    view: WebView,
    uri: String,
}

fn update_tabs(toolbar: &WebView, tabs: &[Tab], active: usize) {
    if tabs[active].uri == START_URL {
        let ids: Vec<_> = tabs.iter().map(|tab| tab.id).collect();
        let _ = tabs[active]
            .view
            .evaluate_script(&format!("window.setFleet?.({})", serde_json::json!(ids)));
    }
    let items: Vec<_> = tabs
        .iter()
        .map(|tab| {
            let name = Url::parse(&tab.uri)
                .ok()
                .filter(|url| url.as_str() != START_URL)
                .and_then(|url| url.host_str().map(str::to_owned))
                .unwrap_or_else(|| "Start".into());
            serde_json::json!([tab.id, name])
        })
        .collect();
    let _ = toolbar.evaluate_script(&format!(
        "setTabs({}, {})",
        serde_json::json!(items),
        tabs[active].id
    ));
    let address = if tabs[active].uri == START_URL {
        ""
    } else {
        &tabs[active].uri
    };
    let _ = toolbar.evaluate_script(&format!("setAddress({}, true)", serde_json::json!(address)));
}

fn create_page(
    window: &Window,
    toolbar: &WebView,
    proxy: &EventLoopProxy<Message>,
    route: &settings::Route,
    id: u64,
    start: Option<Url>,
    smoke: bool,
    protections: bool,
    visible: bool,
) -> Result<WebView, Box<dyn Error>> {
    let offline = matches!(route, settings::Route::Offline);
    let page_proxy = proxy.clone();
    let load_proxy = proxy.clone();
    let mut page_builder = WebViewBuilder::new()
        .with_environment(toolbar.environment())
        .with_visible(visible)
        .with_focused(false)
        .with_https_scheme(true)
        .with_custom_protocol("home".into(), |_, request| {
            local_asset(request.uri().path(), START)
        })
        .with_initialization_script(PAGE_SCRIPT)
        .with_navigation_handler(move |uri| {
            uri == START_URL
                || uri == "about:blank"
                || (!offline
                    && Url::parse(&uri).is_ok_and(|url| matches!(url.scheme(), "http" | "https")))
        })
        .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
        .with_ipc_handler(move |request| {
            if request.body().len() <= 24_000 {
                let _ = page_proxy.send_event(Message::Page(id, request.into_body()));
            }
        })
        .with_on_page_load_handler(move |event, uri| {
            if matches!(event, wry::PageLoadEvent::Finished) {
                let _ = load_proxy.send_event(Message::Loaded(id, uri));
            }
        });
    if let Some(start) = start {
        page_builder = page_builder.with_url(start.as_str());
    } else if smoke {
        page_builder =
            page_builder.with_html("<title>Local rendering test</title><h1>Rendering OK</h1>");
    } else {
        page_builder = page_builder.with_url("home://browser.local/");
    }
    let page = page_builder.build_as_child(window)?;
    let fullscreen_proxy = proxy.clone();
    // WebView2 sends this on the UI thread after a document enters or leaves HTML fullscreen.
    unsafe {
        for view in [&page] {
            let settings: ICoreWebView2Settings8 = view.webview().Settings()?.cast()?;
            settings.SetIsReputationCheckingRequired(protections)?;
        }
        let handler =
            ContainsFullScreenElementChangedEventHandler::create(Box::new(move |view, _| {
                if let Some(view) = view {
                    let mut active = BOOL::default();
                    view.ContainsFullScreenElement(&mut active)?;
                    let _ = fullscreen_proxy.send_event(Message::Fullscreen(id, active.as_bool()));
                }
                Ok(())
            }));
        let mut token = 0;
        page.webview()
            .add_ContainsFullScreenElementChanged(&handler, &mut token)?;
    }
    Ok(page)
}

fn run() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let second = args.iter().any(|arg| arg == "--second-monitor");
    let smoke = args.iter().any(|arg| arg == "--smoke");
    let video_probe = smoke && args.iter().any(|arg| arg == "--video-probe");

    let profile = args
        .iter()
        .find_map(|arg| arg.strip_prefix("--profile="))
        .map(PathBuf::from)
        .unwrap_or(
            std::env::current_exe()?
                .parent()
                .ok_or("Missing executable folder")?
                .join("profile"),
        );
    std::fs::create_dir_all(&profile)?;
    use std::os::windows::fs::OpenOptionsExt;
    let _profile_lock = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(profile.join("profile.lock"))?;
    let settings_path = profile.join("preferences.json");
    let mut preferences = settings::load(&settings_path);
    if !settings_path.exists() {
        settings::save(&settings_path, &preferences)?;
    }
    let mut route = settings::Route::read(&preferences);
    let relay = match dns::Config::parse(&preferences["network"]["dns"]) {
        Ok(None) => None,
        Ok(Some(config)) => {
            let candidate = if let settings::Route::Proxy(endpoint) = &route {
                endpoint
                    .strip_prefix("socks5://")
                    .and_then(|value| value.parse().ok())
            } else {
                None
            };
            match candidate.and_then(|upstream| dns::Relay::start(upstream, config).ok()) {
                Some(relay) => {
                    route = settings::Route::DnsProxy(relay.proxy_url());
                    Some(relay)
                }
                None => {
                    route = settings::Route::Offline;
                    None
                }
            }
        }
        Err(_) => {
            route = settings::Route::Offline;
            None
        }
    };
    let enabled = |key: &str| preferences[key].as_bool().unwrap_or(false);
    let http1 = args.iter().any(|arg| arg == "--http1") || enabled("http1");
    let quic = !http1 && (args.iter().any(|arg| arg == "--quic") || enabled("quic"));
    let protections = args.iter().any(|arg| arg == "--online-protection") || enabled("protection");
    let start = args
        .iter()
        .find_map(|arg| arg.strip_prefix("--url="))
        .and_then(web_url);

    let event_loop = EventLoopBuilder::<Message>::with_user_event().build();
    let icon = app_icon()?;
    let mut builder = WindowBuilder::new()
        .with_window_icon(Some(icon.clone()))
        .with_taskbar_icon(Some(icon))
        .with_title("Rust Browser")
        .with_visible(false)
        .with_focused(false)
        .with_min_inner_size(LogicalSize::new(530, 320))
        .with_inner_size(LogicalSize::new(1030, 740));
    if second {
        let monitor = event_loop
            .available_monitors()
            .find(|monitor| {
                monitor
                    .name()
                    .as_deref()
                    .is_some_and(|name| name.ends_with("DISPLAY2"))
            })
            .ok_or("DISPLAY2 is unavailable")?;
        let origin = monitor.position();
        builder = builder.with_position(PhysicalPosition::new(origin.x + 35, origin.y + 35));
    }
    let window = builder.build(&event_loop)?;
    let module_directory = profile.join("modules");
    let avatar_path = profile.join("avatar");
    let mut avatar_index = std::fs::read_to_string(&avatar_path)
        .ok()
        .and_then(|value| value.trim().parse::<u8>().ok())
        .filter(|index| *index < 4)
        .unwrap_or(0);
    let mut data = data::Data::load(&profile);
    let parent = profile.parent().unwrap_or(&profile);
    let profiles = if parent.ends_with("browser-profiles") {
        parent.to_path_buf()
    } else {
        parent.join("browser-profiles")
    };
    let default_profile = profiles.parent().unwrap_or(&profiles).join("profile");
    let mut context = WebContext::new(Some(profile.clone()));
    let mut flags = String::from(
        "--disable-background-networking --disable-domain-reliability --disable-breakpad --no-default-browser-check",
    );
    if !quic {
        flags.push_str(" --disable-quic");
    }
    if http1 {
        flags.push_str(" --disable-http2");
    }
    if !protections {
        flags.push_str(" --disable-features=msSmartScreenProtection");
    }

    route.apply(&mut flags);
    let toolbar_proxy = event_loop.create_proxy();
    let toolbar = WebViewBuilder::new_with_web_context(&mut context)
        .with_additional_browser_args(flags.clone())
        .with_https_scheme(true)
        .with_custom_protocol("shell".into(), |_, request| {
            local_asset(request.uri().path(), TOOLBAR)
        })
        .with_url("shell://browser.local/")
        .with_navigation_handler(|uri| uri == SHELL_URL)
        .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
        .with_default_context_menus(false)
        .with_ipc_handler(move |request| {
            let _ = toolbar_proxy.send_event(Message::Toolbar(request.into_body()));
        })
        .build_as_child(&window)?;

    unsafe {
        let settings: ICoreWebView2Settings8 = toolbar.webview().Settings()?.cast()?;
        settings.SetIsReputationCheckingRequired(protections)?;
    }
    let proxy = event_loop.create_proxy();
    let page = create_page(
        &window,
        &toolbar,
        &proxy,
        &route,
        0,
        start,
        smoke,
        protections,
        true,
    )?;
    resize(&window, &toolbar, &page, false);
    let mut tabs = vec![Tab {
        id: 0,
        view: page,
        uri: String::new(),
    }];
    let mut active = 0;
    let mut next_id = 1;

    if second
        && !window
            .current_monitor()
            .and_then(|monitor| monitor.name())
            .is_some_and(|name| name.ends_with("DISPLAY2"))
    {
        return Err("Refusing to show a test window outside DISPLAY2".into());
    }
    window.set_visible(true);
    println!(
        "window monitor: {:?}",
        window.current_monitor().and_then(|monitor| monitor.name())
    );

    let mut manager: Option<(Window, WebView)> = None;
    let mut tun_options = tun::load(&profile);
    let mut tun_session: Option<tun::Session> = None;
    let mut tun_state = String::from("stopped");
    let mut open_tun_page = false;
    if tun_options["autostart"] == true && tun::available() {
        tun_state = "preparing".into();
        let sender = proxy.clone();
        let options = tun_options.clone();
        std::thread::spawn(move || {
            let _ = sender.send_event(Message::TunPrepared(tun::prepare(options)));
        });
    }
    let mut expanded = false;
    let mut shortcuts = Vec::new();
    let mut resources = resources::Sampler::default();
    let mut rate_samples = VecDeque::with_capacity(30);
    let mut previous_total = None;
    let mut next_poll = Instant::now();
    let exit_at = Instant::now() + Duration::from_secs(if video_probe { 25 } else { 12 });
    let mut smoke_fullscreen = false;

    event_loop.run(move |event, target, flow| {
        let _ = &relay;
        *flow = ControlFlow::WaitUntil(if smoke { next_poll.min(exit_at) } else { next_poll });
        let page = &tabs[active].view;
        let mark_bookmark = |bookmarks: &[serde_json::Value], uri: &str| {
            let marked = bookmarks.iter().any(|entry| entry["url"] == uri);
            let _ = toolbar.evaluate_script(&format!("setBookmarked({marked})"));
        };
        match event {
            Event::WindowEvent { window_id, event: WindowEvent::CloseRequested, .. }
                if manager.as_ref().is_some_and(|(window, _)| window.id() == window_id) => {
                    manager = None;
                }
            Event::WindowEvent { window_id, event: WindowEvent::CloseRequested, .. }
                if window_id == window.id() => *flow = ControlFlow::Exit,
            Event::WindowEvent { window_id, event: WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. }, .. } if window_id == window.id() =>
                resize(&window, &toolbar, &page, expanded),
            Event::UserEvent(Message::Loaded(id, uri)) => {
                let Some(index) = tabs.iter().position(|tab| tab.id == id) else { return };
                tabs[index].uri = uri.clone();
                if let Ok(url) = Url::parse(&uri) {
                    if url.host_str() == Some("duckduckgo.com") {
                        if let Some((_, query)) = url.query_pairs().find(|(key, _)| key == "q") {
                            data.record(query.into_owned());
                        }
                    }
                }
                if uri == START_URL {
                    let theme = settings::theme(&preferences);
                    let surface = settings::surface(&preferences);
                    let font = settings::font(&preferences);
                    let motion = preferences["appearance"]["motion"].as_bool().unwrap_or(true);
                    let _ = tabs[index].view.evaluate_script(&format!("window.setBookmarks?.({}); window.setStartActive?.({}); window.setAppearance?.({theme:?}, {surface:?}, {font:?}, {motion})", serde_json::json!(data.bookmarks), index == active));
                }
                if index != active { return; }
                update_tabs(&toolbar, &tabs, active);
                mark_bookmark(&data.bookmarks, &uri);
                let page = &tabs[active].view;
                let address = if uri == START_URL || uri.starts_with("home://") { "" } else { &uri };
                let text = serde_json::to_string(address).expect("serialize URI");
                let _ = toolbar.evaluate_script(&format!("setAddress({text})"));
                if expanded {
                    previous_total = None;
                    let _ = page.evaluate_script("window.__browserNetwork?.(true)");
                }
                if smoke && !smoke_fullscreen {
                    println!("page loaded{}", if uri.starts_with("http") { " (web)" } else { " (local)" });
                    window.set_fullscreen(Some(Fullscreen::Borderless(window.current_monitor())));
                    resize(&window, &toolbar, &page, expanded);
                    println!("fullscreen monitor: {:?}", window.current_monitor().and_then(|monitor| monitor.name()));
                    smoke_fullscreen = true;
                }
            }
            Event::UserEvent(Message::Toolbar(body)) => {
                let Ok(message) = serde_json::from_str::<serde_json::Value>(&body) else { return; };
                match message["action"].as_str() {
                    Some("go") => {
                        if let Some(url) = message["value"].as_str().and_then(web_url) {
                            let _ = page.load_url(url.as_str());
                        }
                    }
                    Some("youtube") => { let _ = page.load_url(YOUTUBE); }
                    Some("back") => { let _ = page.evaluate_script("history.back()"); }
                    Some("forward") => { let _ = page.evaluate_script("history.forward()"); }
                    Some("reload") => { let _ = page.evaluate_script("location.reload()"); }
                    Some("toolbar-ready") => {
                        update_tabs(&toolbar, &tabs, active);
                        let _ = toolbar.evaluate_script(&format!("setAvatar({avatar_index}); setRoute({:?}); setTheme({:?}, {:?}, {:?})", route.label(), settings::theme(&preferences), settings::surface(&preferences), settings::font(&preferences)));
                    }
                    Some("avatar") => {
                        if let Some(index) = message["value"].as_u64().filter(|index| *index < 4) {
                            avatar_index = index as u8;
                            if let Err(error) = std::fs::write(&avatar_path, index.to_string()) {
                                eprintln!("Could not save avatar: {error}");
                            }
                        }
                    }
                    Some("modules") => {
                        shortcuts = modules::shortcuts(&module_directory).into_iter().filter(|(id, _, _)| {
                            !preferences["extensions"]["disabled"].as_array().into_iter().flatten().any(|item| item == id.as_str())
                        }).collect();
                        let labels: Vec<_> = shortcuts.iter().map(|(_, label, _)| label).collect();
                        if let Ok(labels) = serde_json::to_string(&labels) {
                            let _ = toolbar.evaluate_script(&format!("setModules({labels})"));
                        }
                    }
                    Some("module-go") => {
                        if let Some((_, _, url)) = message["value"].as_u64()
                            .and_then(|index| usize::try_from(index).ok())
                            .and_then(|index| shortcuts.get(index)) {
                            let _ = page.load_url(url.as_str());
                        }
                    }
                    Some("new-tab" | "switch-tab" | "close-tab") => {
                        let action = message["action"].as_str().unwrap_or_default();
                        let previous = tabs[active].id;
                        let target = message["value"].as_u64()
                            .and_then(|id| tabs.iter().position(|tab| tab.id == id));
                        match (action, target) {
                            ("new-tab", _) if tabs.len() < 24 => {
                                match create_page(&window, &toolbar, &proxy, &route, next_id, None, false, protections, false) {
                                    Ok(view) => {
                                        tabs.push(Tab { id: next_id, view, uri: String::new() });
                                        active = tabs.len() - 1;
                                        next_id += 1;
                                    }
                                    Err(error) => eprintln!("Could not open tab: {error}"),
                                }
                            }
                            ("switch-tab", Some(index)) => active = index,
                            ("close-tab", Some(index)) if tabs.len() > 1 => {
                                tabs.remove(index);
                                active = tabs.iter().position(|tab| tab.id == previous)
                                    .unwrap_or(index.min(tabs.len() - 1));
                            }
                            _ => (),
                        }
                        for (index, tab) in tabs.iter().enumerate() {
                            let enabled = index == active;
                            let _ = tab.view.set_visible(enabled);
                            if tab.uri == START_URL {
                                let _ = tab.view.evaluate_script(&format!("window.setStartActive?.({enabled})"));
                            }
                            let _ = tab.view.evaluate_script(&format!("window.__browserNetwork?.({})", enabled && expanded));
                        }
                        previous_total = None;
                        resize(&window, &toolbar, &tabs[active].view, expanded);
                        update_tabs(&toolbar, &tabs, active);
                        mark_bookmark(&data.bookmarks, &tabs[active].uri);
                    }
                    Some("bookmark") => {
                        if let Ok(url) = Url::parse(&tabs[active].uri) {
                            if matches!(url.scheme(), "https" | "http") && url.as_str() != START_URL {
                                data.toggle_bookmark(&url);
                                let _ = toolbar.evaluate_script(&format!("setBookmarked({})", data.bookmarks.iter().any(|entry| entry["url"] == url.as_str())));
                                for tab in tabs.iter().filter(|tab| tab.uri == START_URL) {
                                    let _ = tab.view.evaluate_script(&format!("window.setBookmarks?.({})", serde_json::json!(data.bookmarks)));
                                }
                            }
                        }
                    }
                    Some("history") => {
                        let _ = toolbar.evaluate_script(&format!("setChoices('history', {})", serde_json::json!(data.history)));
                    }
                    Some("history-go") => {
                        if let Some(query) = message["value"].as_u64().and_then(|i| data.history.get(i as usize)) {
                            let _ = page.load_url(data::search_url(query).as_str());
                        }
                    }
                    Some("history-clear") => {
                        data.history.clear();
                        data.save();
                        let _ = toolbar.evaluate_script("setChoices('history', [])");
                    }
                    Some("preferences") => {
                        let value = &message["value"];
                        let mut settings = preferences.clone();
                        for key in ["http1", "quic", "protection"] {
                            settings[key] = value[key].as_bool().unwrap_or(false).into();
                        }
                        if settings::save(&settings_path, &settings).is_ok() {
                            preferences = settings;
                            let _ = toolbar.evaluate_script("preferencesSaved()");
                        }
                    }
                    Some("profiles") => {
                        let mut names: Vec<_> = std::fs::read_dir(&profiles).into_iter().flatten().flatten()
                            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                            .filter_map(|entry| entry.file_name().into_string().ok()).collect();
                        names.sort();
                        let current = profile.file_name().and_then(|name| name.to_str()).unwrap_or("Default");
                        let _ = toolbar.evaluate_script(&format!("setProfiles({}, {}, {})", serde_json::json!(names), serde_json::json!(current), std::fs::read_to_string(&settings_path).ok().and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok()).unwrap_or_default()));
                    }
                    Some("profile-open") => {
                        let name = message["value"].as_str().unwrap_or_default();
                        let path = if name == "@default" { Some(default_profile.clone()) } else {
                            data::profile_name(name).map(|name| profiles.join(name))
                        };
                        if let Some(path) = path.filter(|path| *path != profile) {
                            if let Ok(exe) = std::env::current_exe() {
                                let mut command = std::process::Command::new(exe);
                                command.arg(format!("--profile={}", path.display()));
                                if second { command.arg("--second-monitor"); }
                                use std::os::windows::process::CommandExt;
                                let _ = command.creation_flags(0x08000000).spawn();
                            }
                        }
                    }
                    Some("manager" | "tun-manager") => {
                        open_tun_page = message["action"] == "tun-manager";
                        if manager.is_none() {
                            let result = (|| -> Result<_, Box<dyn Error>> {
                                let mut builder = WindowBuilder::new().with_title("Control panel")
                                    .with_inner_size(LogicalSize::new(620, 590))
                                    .with_min_inner_size(LogicalSize::new(460, 380))
                                    .with_window_icon(Some(app_icon()?));
                                if let Some(monitor) = window.current_monitor() {
                                    let origin = monitor.position();
                                    builder = builder.with_position(PhysicalPosition::new(origin.x + 90, origin.y + 65));
                                }
                                let dialog = builder.build(target)?;
                                let manager_proxy = proxy.clone();
                                let view = WebViewBuilder::new().with_environment(toolbar.environment())
                                    .with_https_scheme(true)
                                    .with_custom_protocol("manager".into(), |_, req| local_asset(req.uri().path(), MANAGER))
                                    .with_url("manager://browser.local/")
                                    .with_navigation_handler(|uri| uri == MANAGER_URL)
                                    .with_new_window_req_handler(|_, _| NewWindowResponse::Deny)
                                    .with_ipc_handler(move |req| {
                                        if req.body().len() <= 32_000 {
                                            let _ = manager_proxy.send_event(Message::Manage(req.into_body()));
                                        }
                                    }).build(&dialog)?;
                                unsafe {
                                    let config: ICoreWebView2Settings8 = view.webview().Settings()?.cast()?;
                                    config.SetIsReputationCheckingRequired(protections)?;
                                }
                                Ok((dialog, view))
                            })();
                            match result {
                                Ok(pair) => manager = Some(pair),
                                Err(error) => eprintln!("Control panel: {error}"),
                            }
                        }
                        if let Some((dialog, view)) = &manager {
                            dialog.set_focus();
                            if open_tun_page {
                                let _ = view.evaluate_script("document.querySelector('[data-tab=tun]')?.click()");
                            }
                        }
                    }
                    Some("network") => {
                        expanded = !expanded;
                        rate_samples.clear();
                        previous_total = None;
                        next_poll = Instant::now();
                        let _ = toolbar.evaluate_script(&format!("setNetwork({expanded})"));
                        let _ = page.evaluate_script(&format!("window.__browserNetwork?.({expanded})"));
                        resize(&window, &toolbar, &page, expanded);
                    }
                    Some("fullscreen") => {
                        let full = window.fullscreen().is_none().then(|| Fullscreen::Borderless(window.current_monitor()));
                        window.set_fullscreen(full);
                        resize(&window, &toolbar, &page, expanded);
                    }
                    _ => (),
                }
            }
            Event::UserEvent(Message::Manage(body)) => {
                let Some((_, view)) = &manager else { return; };
                let Ok(message) = serde_json::from_str::<serde_json::Value>(&body) else { return; };
                let mut notice = None;
                if message["action"] == "state" {
                    let _ = view.evaluate_script(&format!("showTun({tun_options}); tunState({tun_state:?}, {})", tun::available()));
                    if open_tun_page {
                        let _ = view.evaluate_script("document.querySelector('[data-tab=tun]')?.click()");
                    }
                }
                if let Some(action) = message["action"].as_str().filter(|action| action.starts_with("tun-")) {
                    let value = &message["value"];
                    let result: Result<&str, &str> = match action {
                        "tun-preset" => {
                            if let Some(preset) = tun::presets(value["protocol"].as_str().unwrap_or_default(), value["transport"].as_str().unwrap_or_default()) {
                                let _ = view.evaluate_script(&format!("tunPreset({preset})"));
                                Ok("Пресет загружен.")
                            } else { Err("Неизвестный протокол.") }
                        }
                        "tun-import" => {
                            if let Some(text) = value.as_str() {
                                let text = text.to_owned();
                                let sender = proxy.clone();
                                std::thread::spawn(move || {
                                    let _ = sender.send_event(Message::TunImported(tun::import(&text)));
                                });
                            }
                            Ok("Импорт…")
                        }
                        "tun-check" => tun::check(value).map(|()| "Конфигурация принята sing-box."),
                        "tun-start" => {
                            if tun_session.is_some() || tun_state == "preparing" {
                                Err("Подключение уже выполняется.")
                            } else {
                                let mut options = value["options"].clone();
                                let input = value["input"].as_str().unwrap_or("").trim().to_owned();
                                if options["node"].is_null() && input.is_empty() {
                                    options["node"] = tun_options["node"].clone();
                                }
                                tun_state = "preparing".into();
                                let sender = proxy.clone();
                                std::thread::spawn(move || {
                                    let prepared = if input.is_empty() {
                                        tun::prepare(options)
                                    } else {
                                        tun::select_working(&input, options)
                                    };
                                    let _ = sender.send_event(Message::TunPrepared(prepared));
                                });
                                Ok("Проверяю доступность узла…")
                            }
                        }
                        "tun-save" => {
                            let operation = || {
                                tun::check(value)?;
                                tun::save(&profile, value).map_err(|_| "Не удалось безопасно сохранить параметры.")?;
                                Ok(())
                            };
                            match operation() {
                                Ok(()) => {
                                    tun_options = value.clone();
                                    Ok("Настройки TUN сохранены.")
                                }
                                Err(error) => Err(error),
                            }
                        }
                        "tun-stop" => {
                            if let Some(session) = &tun_session { session.stop(); }
                            tun_state = "stopping".into();
                            Ok("Остановка запрошена.")
                        }
                        _ => Err("Неизвестная команда TUN."),
                    };
                    let status = result.unwrap_or_else(|error| error);
                    let _ = view.evaluate_script(&format!("notice({}); tunState({tun_state:?}, {})", serde_json::json!(status), tun::available()));
                    return;
                }
                if message["action"] == "install" {
                    notice = Some(match modules::install(&module_directory, &message["value"]) {
                        Ok(()) => "Модуль установлен.",
                        Err(error) => error,
                    });
                }
                if message["action"] == "save" {
                    match settings::validate(&message["value"]) {
                        Ok(value) if settings::save(&settings_path, &value).is_ok() => {
                            preferences = value;
                            let theme = settings::theme(&preferences);
                    let surface = settings::surface(&preferences);
                    let font = settings::font(&preferences);
                            let motion = preferences["appearance"]["motion"].as_bool().unwrap_or(true);
                            let _ = toolbar.evaluate_script(&format!("setTheme({theme:?}, {surface:?}, {font:?})"));
                            for tab in tabs.iter().filter(|tab| tab.uri == START_URL) {
                                let _ = tab.view.evaluate_script(&format!("window.setAppearance?.({theme:?}, {surface:?}, {font:?}, {motion})"));
                            }
                            notice = Some("Сохранено. Изменения сети вступят в силу после перезапуска.");
                        }
                        Ok(_) => notice = Some("Не удалось записать настройки."),
                        Err(error) => notice = Some(error),
                    }
                }
                if notice.is_none() || notice.is_some_and(|text| text.starts_with("Сохранено") || text == "Модуль установлен.") {
                    let list: Vec<_> = modules::shortcuts(&module_directory).into_iter().map(|(id, title, _)| (id, title)).collect();
                    let script = format!("showState({}, {}); setTheme({:?}, {:?}, {:?})", preferences, serde_json::json!(list), settings::theme(&preferences), settings::surface(&preferences), settings::font(&preferences));
                    let _ = view.evaluate_script(&script);
                }
                if let Some(notice) = notice {
                    let _ = view.evaluate_script(&format!("notice({})", serde_json::json!(notice)));
                }
            }
            Event::UserEvent(Message::TunImported(result)) => {
                if let Some((_, view)) = &manager {
                    match result {
                        Ok(nodes) => {
                            let _ = view.evaluate_script(&format!("tunImported({}); notice('Импорт завершён.')", serde_json::json!(nodes)));
                        }
                        Err(error) => {
                            let _ = view.evaluate_script(&format!("notice({})", serde_json::json!(error)));
                        }
                    }
                }
            }
            Event::UserEvent(Message::TunPrepared(result)) => {
                let outcome = result.and_then(|options| {
                    tun::save(&profile, &options).map_err(|_| "Не удалось сохранить выбранный узел.")?;
                    tun_options = options;
                    tun::Session::start(&profile, &tun_options)
                });
                let notice = match outcome {
                    Ok(session) => {
                        tun_state = "starting".into();
                        tun_session = Some(session);
                        "Узел ответил. Windows запускает TUN."
                    }
                    Err(message) => {
                        tun_state = "error".into();
                        message
                    }
                };
                let _ = toolbar.evaluate_script(&format!("setTunStatus({tun_state:?})"));
                if let Some((_, view)) = &manager {
                    let _ = view.evaluate_script(&format!("showTun({tun_options}); tunState({tun_state:?}, {}); notice({})",
                        tun::available(), serde_json::json!(notice)));
                }
            }
            Event::UserEvent(Message::Fullscreen(id, fullscreen)) if id == tabs[active].id => {
                window.set_fullscreen(fullscreen.then(|| Fullscreen::Borderless(window.current_monitor())));
                resize(&window, &toolbar, &page, expanded);
            }
            Event::UserEvent(Message::Page(id, body)) if id == tabs[active].id => {
                let Ok(message) = serde_json::from_str::<serde_json::Value>(&body) else { return; };
                match message["action"].as_str() {
                    Some("exit-fullscreen") => {
                        if window.fullscreen().is_some() {
                            window.set_fullscreen(None);
                            resize(&window, &toolbar, &page, expanded);
                        }
                    }
                    Some("network-sample") if expanded => {
                        let Some(rows) = message["value"].as_array() else { return; };
                        let mut domains: Vec<_> = rows.iter().take(100).filter_map(|row| {
                            Some((row[0].as_str()?, row[1].as_u64()?, row[2].as_u64()?))
                        }).collect();
                        domains.sort_unstable_by_key(|row| std::cmp::Reverse(row.1));
                        let total = domains.iter().fold(0u64, |total, row| total.saturating_add(row.1));
                        let rate = previous_total.replace(total).map_or(0, |previous| total.saturating_sub(previous));
                        if rate_samples.len() == 30 { rate_samples.pop_front(); }
                        rate_samples.push_back(rate);
                        domains.truncate(20);
                        let stats = serde_json::json!({"rate": rate, "samples": rate_samples, "domains": domains});
                        let _ = toolbar.evaluate_script(&format!("updateNetwork({stats})"));
                    }
                    _ => (),
                }
            }
            Event::MainEventsCleared => {
                if smoke && Instant::now() >= exit_at {
                    *flow = ControlFlow::Exit;
                } else if Instant::now() >= next_poll {
                    if let Some(session) = &tun_session {
                        let state = session.state();
                        if state != tun_state {
                            tun_state = state;
                            let _ = toolbar.evaluate_script(&format!("setTunStatus({tun_state:?})"));
                            if let Some((_, view)) = &manager {
                                let _ = view.evaluate_script(&format!("tunState({tun_state:?}, true)"));
                            }
                        }
                        if matches!(tun_state.as_str(), "stopped" | "error" | "dns-error") {
                            tun_session = None;
                        }
                    }
                    if video_probe {
                        let _ = page.evaluate_script_with_callback(
                            r#"(() => {
                              const video = document.querySelector("video");
                              return {
                                title: document.title,
                                host: location.hostname,
                                frames: video?.getVideoPlaybackQuality().totalVideoFrames ?? 0,
                                playing: !!video && !video.paused,
                              };
                            })()"#,
                            |state| println!("video probe: {state}"),
                        );
                    }
                    if window.fullscreen().is_none() {
                        let stats = resources.sample(&page);
                        let _ = toolbar.evaluate_script(&format!("updateResources({stats})"));
                        if expanded {
                            let _ = page.evaluate_script("window.__browserNetworkSample?.()");
                        }
                    }
                    next_poll = Instant::now() + Duration::from_secs(if expanded { 1 } else { 2 });
                    *flow = ControlFlow::WaitUntil(if smoke { next_poll.min(exit_at) } else { next_poll });
                }
            }
            _ => (),
        }
    });
}

fn main() {
    let mut arguments = std::env::args_os().skip(1);
    if let Some(command) = arguments.next() {
        if command == "--tun-worker" {
            let path = arguments.next().map(PathBuf::from);
            let parent = arguments
                .next()
                .and_then(|value| value.to_str()?.parse().ok());
            let success = path
                .zip(parent)
                .is_some_and(|(path, parent)| tun::worker(&path, parent).is_ok());
            std::process::exit(if success { 0 } else { 1 });
        }
        if command == "--tun-check" {
            let input: Result<serde_json::Value, _> = serde_json::from_reader(std::io::stdin());
            let result = input
                .map_err(|_| "Некорректный JSON.")
                .and_then(|options| tun::check(&options));
            if let Err(error) = result {
                eprintln!("{error}");
                std::process::exit(1);
            }
            println!("TUN configuration accepted.");
            return;
        }
        if command == "--tun-test-subscription" {
            use std::io::Read;
            let mut input = String::new();
            let result = std::io::stdin()
                .take(32_000)
                .read_to_string(&mut input)
                .map_err(|_| "Не удалось прочитать ввод.")
                .and_then(|_| tun::select_working(&input, tun::defaults()));
            match result {
                Ok(options) => {
                    println!("Connected in route-free probe: {}", options["node"]["type"])
                }
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(1);
                }
            }
            return;
        }
    }
    if let Err(error) = run() {
        eprintln!("{error}");
    }
}
