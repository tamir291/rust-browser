use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

use gtk::prelude::*;
use serde_json::{Value, json};
use url::Url;
use wry::{PageLoadEvent, WebContext, WebView, WebViewBuilder, WebViewBuilderExtUnix};

const HOME: &str = "orbital://start/";
const START: &str = include_str!("../../src/start.html");
const FONT: &[u8] = include_bytes!("../../src/assets/PixelifySans.ttf");
const MAX_TABS: usize = 24;

struct Tab {
    id: u64,
    address: Rc<RefCell<String>>,
    web: WebView,
}

struct Browser {
    window: gtk::Window,
    address: gtk::Entry,
    notebook: gtk::Notebook,
    context: RefCell<WebContext>,
    tabs: RefCell<Vec<Tab>>,
    sequence: Cell<u64>,
    data: RefCell<Value>,
    data_path: PathBuf,
    smoke: bool,
    loaded_home: RefCell<HashSet<u64>>,
}

fn profile() -> PathBuf {
    if let Some(path) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(path).join("rust-browser");
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local/share/rust-browser")
}

fn read_data(path: &Path) -> Value {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| {
            json!({
                "bookmarks":[{"name":"YouTube","url":"https://www.youtube.com/"}], "history":[]
            })
        })
}

fn web_url(input: &str) -> bool {
    Url::parse(input)
        .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.host().is_some())
}

fn destination(input: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    if web_url(input) {
        return Some(input.into());
    }
    if input.contains('.') && !input.contains(char::is_whitespace) && !input.contains("://") {
        let candidate = format!("https://{input}");
        if web_url(&candidate) {
            return Some(candidate);
        }
    }
    let mut query = Url::parse("https://duckduckgo.com/").expect("fixed search URL");
    query.query_pairs_mut().append_pair("q", input);
    Some(query.into())
}

fn home_request(path: &str) -> wry::http::Response<Cow<'static, [u8]>> {
    use wry::http::Response;
    match path {
        "/" => Response::builder().header("Content-Type", "text/html; charset=utf-8")
            .header("Content-Security-Policy", "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; font-src 'self'; base-uri 'none'; form-action https://duckduckgo.com")
            .body(Cow::Borrowed(START.as_bytes())).expect("fixed response"),
        "/terminal.ttf" => Response::builder().header("Content-Type", "font/ttf")
            .body(Cow::Borrowed(FONT)).expect("fixed response"),
        _ => Response::builder().status(404).body(Cow::<[u8]>::Borrowed(&[])).expect("fixed response"),
    }
}

impl Browser {
    fn current(&self, action: impl FnOnce(&Tab)) {
        if let Some(tab) = self
            .notebook
            .current_page()
            .and_then(|index| self.tabs.borrow().get(index as usize).map(|tab| tab.id))
        {
            if let Some(tab) = self.tabs.borrow().iter().find(|item| item.id == tab) {
                action(tab);
            }
        }
    }

    fn add_tab(self: &Rc<Self>) -> wry::Result<()> {
        if self.tabs.borrow().len() >= MAX_TABS {
            return Ok(());
        }
        let id = self.sequence.get();
        let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let tab_label = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let label = gtk::Label::new(Some("Start"));
        label.set_max_width_chars(23);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let close = gtk::Button::with_label("×");
        close.set_relief(gtk::ReliefStyle::None);
        close.set_tooltip_text(Some("Close tab"));
        tab_label.pack_start(&label, true, true, 0);
        tab_label.pack_start(&close, false, false, 0);
        let addr = Rc::new(RefCell::new(HOME.to_owned()));
        let weak = Rc::downgrade(self);
        let page_address = addr.clone();

        let mut context = self.context.borrow_mut();
        let mut builder = WebViewBuilder::new_with_web_context(&mut context);
        if id == 0 {
            builder = builder.with_custom_protocol("orbital".into(), |_, request| {
                home_request(request.uri().path())
            });
        }
        let web = builder
            .with_navigation_handler(|url| url == HOME || web_url(&url))
            .with_on_page_load_handler(move |event, uri| {
                if !matches!(event, PageLoadEvent::Finished) {
                    return;
                }
                *page_address.borrow_mut() = uri.clone();
                let Some(browser) = weak.upgrade() else {
                    return;
                };
                let host = Url::parse(&uri)
                    .ok()
                    .and_then(|url| url.host_str().map(str::to_owned));
                label.set_text(if uri == HOME {
                    "Start"
                } else {
                    host.as_deref().unwrap_or("Page")
                });
                if uri == HOME {
                    browser.loaded_home.borrow_mut().insert(id);
                } else if web_url(&uri) {
                    browser.remember("history", host.as_deref().unwrap_or("Page"), &uri, 100);
                }
                browser.refresh();
            })
            .with_url(HOME)
            .build_gtk(&page)?;
        drop(context);

        let weak = Rc::downgrade(self);
        close.connect_clicked(move |_| {
            let Some(browser) = weak.upgrade() else {
                return;
            };
            let index = browser.tabs.borrow().iter().position(|tab| tab.id == id);
            let Some(index) = index else { return };
            if browser.tabs.borrow().len() == 1 {
                browser.current(|tab| {
                    let _ = tab.web.load_url(HOME);
                });
            } else {
                let removed = browser.tabs.borrow_mut().remove(index);
                browser.notebook.remove_page(Some(index as u32));
                drop(removed);
                browser.refresh();
            }
        });
        self.sequence.set(id + 1);
        self.tabs.borrow_mut().push(Tab {
            id,
            address: addr,
            web,
        });
        let index = self.notebook.append_page(&page, Some(&tab_label));
        page.show_all();
        tab_label.show_all();
        self.notebook.set_current_page(Some(index));
        self.refresh();
        Ok(())
    }

    fn refresh(&self) {
        let tabs = self.tabs.borrow();
        let active = self.notebook.current_page().map(|index| index as usize);
        let ids: Vec<_> = tabs.iter().map(|tab| tab.id).collect();
        let fleet = json!(ids);
        let data = self.data.borrow();
        for (index, tab) in tabs.iter().enumerate() {
            let uri = tab.address.borrow();
            if Some(index) == active && !self.address.has_focus() {
                self.address
                    .set_text(if uri.as_str() == HOME { "" } else { &uri });
            }
            if uri.as_str() == HOME {
                let _ = tab.web.evaluate_script(&format!(
                    "window.setBookmarks?.({});window.setFleet?.({fleet});window.setStartActive?.({})",
                    data["bookmarks"], Some(index) == active,
                ));
            }
        }
    }

    fn remember(&self, list: &str, name: &str, url: &str, limit: usize) {
        let mut data = self.data.borrow_mut();
        let mut rows = data[list].as_array().cloned().unwrap_or_default();
        rows.retain(|row| row["url"] != url);
        rows.insert(0, json!({"name": name, "url": url}));
        rows.truncate(limit);
        data[list] = json!(rows);
        let temporary = self.data_path.with_extension("pending");
        if fs::write(&temporary, data.to_string()).is_ok() {
            let _ = fs::rename(temporary, &self.data_path);
        }
    }

    fn show_list(self: &Rc<Self>, key: &str, title: &str) {
        let dialog = gtk::Dialog::with_buttons(
            Some(title),
            Some(&self.window),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            &[("Close", gtk::ResponseType::Close)],
        );
        dialog.set_default_size(560, 400);
        let scroll = gtk::ScrolledWindow::new(gtk::Adjustment::NONE, gtk::Adjustment::NONE);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 4);
        scroll.add(&list);
        dialog.content_area().pack_start(&scroll, true, true, 5);
        if let Some(rows) = self.data.borrow()[key].as_array() {
            for row in rows {
                let Some(uri) = row["url"].as_str().filter(|url| web_url(url)) else {
                    continue;
                };
                let url = uri.to_owned();
                let button = gtk::Button::with_label(row["name"].as_str().unwrap_or(uri));
                let weak = Rc::downgrade(self);
                let dialog_weak = dialog.downgrade();
                button.connect_clicked(move |_| {
                    if let Some(browser) = weak.upgrade() {
                        browser.current(|tab| {
                            let _ = tab.web.load_url(&url);
                        });
                    }
                    if let Some(dialog) = dialog_weak.upgrade() {
                        dialog.close();
                    }
                });
                list.pack_start(&button, false, false, 0);
            }
        }
        dialog.connect_response(|dialog, _| dialog.close());
        dialog.show_all();
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    gtk::init()?;
    let smoke = std::env::args().any(|argument| argument == "--smoke");
    let directory = profile();
    fs::create_dir_all(&directory)?;
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("Rectangle");
    let icon = gtk::gdk_pixbuf::PixbufLoader::new();
    icon.write(include_bytes!("../../src/assets/orbit.png"))?;
    icon.close()?;
    window.set_icon(icon.pixbuf().as_ref());
    window.set_default_size(1050, 740);
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    header.set_margin_top(7);
    header.set_margin_bottom(7);
    header.set_margin_start(7);
    header.set_margin_end(7);
    let address = gtk::Entry::new();
    address.set_placeholder_text(Some("Address or DuckDuckGo search"));
    let notebook = gtk::Notebook::new();
    notebook.set_scrollable(true);
    let data_path = directory.join("browser-data.json");
    let browser = Rc::new(Browser {
        window: window.clone(),
        address: address.clone(),
        notebook: notebook.clone(),
        context: RefCell::new(WebContext::new(Some(directory.join("webkit")))),
        tabs: RefCell::new(Vec::new()),
        sequence: Cell::new(0),
        data: RefCell::new(read_data(&data_path)),
        data_path,
        smoke,
        loaded_home: RefCell::new(HashSet::new()),
    });
    for (title, action) in [
        ("←", "back"),
        ("↻", "reload"),
        ("+", "new"),
        ("☆", "bookmark"),
        ("Bookmarks", "bookmarks"),
        ("History", "history"),
    ] {
        let button = gtk::Button::with_label(title);
        let weak = Rc::downgrade(&browser);
        button.connect_clicked(move |_| {
            let Some(browser) = weak.upgrade() else {
                return;
            };
            match action {
                "back" => browser.current(|tab| {
                    let _ = tab.web.evaluate_script("history.back()");
                }),
                "reload" => browser.current(|tab| {
                    let _ = tab.web.reload();
                }),
                "new" => {
                    if let Err(error) = browser.add_tab() {
                        eprintln!("Tab: {error}");
                    }
                }
                "bookmark" => browser.current(|tab| {
                    let uri = tab.address.borrow();
                    if web_url(&uri) {
                        browser.remember("bookmarks", &uri, &uri, 40);
                    }
                }),
                "bookmarks" => browser.show_list("bookmarks", "Bookmarks"),
                "history" => browser.show_list("history", "History"),
                _ => (),
            }
        });
        header.pack_start(&button, false, false, 0);
        if action == "reload" {
            header.pack_start(&address, true, true, 0);
        }
    }

    let weak = Rc::downgrade(&browser);
    address.connect_activate(move |entry| {
        if let Some((browser, url)) = weak.upgrade().zip(destination(&entry.text())) {
            browser.current(|tab| {
                let _ = tab.web.load_url(&url);
            });
            browser.window.set_focus(None::<&gtk::Widget>);
        }
    });
    let weak = Rc::downgrade(&browser);
    notebook.connect_switch_page(move |_, _, _| {
        let weak = weak.clone();
        gtk::glib::idle_add_local_once(move || {
            if let Some(browser) = weak.upgrade() {
                browser.refresh();
            }
        });
    });
    let css = gtk::CssProvider::new();
    css.load_from_data(b"window, notebook, headerbar { background: #101310; color: #ece9de; } button,entry {color:#e1c485;background:#191d17;border-color:#44473b;} tab:checked {background:#292a1b;}")?;
    if let Some(screen) = gtk::gdk::Screen::default() {
        gtk::StyleContext::add_provider_for_screen(
            &screen,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
    outer.pack_start(&header, false, false, 0);
    outer.pack_start(&notebook, true, true, 0);
    window.add(&outer);
    window.connect_delete_event(|_, _| {
        gtk::main_quit();
        gtk::glib::Propagation::Proceed
    });
    browser.add_tab()?;
    window.show_all();
    if smoke {
        let owner = Rc::downgrade(&browser);
        gtk::glib::timeout_add_seconds_local_once(5, move || {
            if let Some(browser) = owner.upgrade() {
                if browser.add_tab().is_err() {
                    browser.loaded_home.borrow_mut().clear();
                }
            }
        });
        gtk::glib::timeout_add_seconds_local_once(10, gtk::main_quit);
    }
    gtk::main();
    if browser.smoke
        && (browser.loaded_home.borrow().len() != 2 || browser.tabs.borrow().len() != 2)
    {
        return Err("Linux start page and two-tab smoke check failed".into());
    }
    Ok(())
}
