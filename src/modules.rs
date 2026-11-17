use std::{fs, io::Read, path::Path};

use serde_json::Value;
use url::Url;

pub fn shortcuts(directory: &Path) -> Vec<(String, String, Url)> {
    let Ok(files) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut modules: Vec<_> = files
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| {
            let id = entry.file_name().into_string().ok()?;
            let file = fs::File::open(entry.path()).ok()?;
            if !file.metadata().ok()?.is_file() {
                return None;
            }
            let mut content = String::new();
            file.take(4097).read_to_string(&mut content).ok()?;
            if content.len() > 4096 {
                return None;
            }
            let manifest: Value = serde_json::from_str(&content).ok()?;
            if manifest["type"].as_str()? != "shortcut" {
                return None;
            }
            let title = manifest["title"].as_str()?.trim();
            if title.is_empty() || title.len() > 100 {
                return None;
            }
            let url = Url::parse(manifest["url"].as_str()?).ok()?;
            matches!(url.scheme(), "https" | "http").then(|| (id, title.to_owned(), url))
        })
        .take(50)
        .collect();
    modules.sort_by(|a, b| a.1.cmp(&b.1));
    modules
}

pub fn install(directory: &Path, manifest: &Value) -> Result<(), &'static str> {
    use std::io::Write;
    let id = manifest["id"]
        .as_str()
        .filter(|id| {
            (2..=40).contains(&id.len())
                && id.as_bytes()[0].is_ascii_lowercase()
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
        .ok_or("Укажи id из строчных латинских букв, цифр и дефисов.")?;
    let title = manifest["title"]
        .as_str()
        .map(str::trim)
        .filter(|title| !title.is_empty() && title.len() <= 100)
        .ok_or("Укажи название модуля.")?;
    let url = manifest["url"]
        .as_str()
        .and_then(|value| Url::parse(value).ok())
        .filter(|url| {
            matches!(url.scheme(), "https" | "http")
                && url.username().is_empty()
                && url.password().is_none()
        })
        .ok_or("Укажи корректный HTTP или HTTPS адрес.")?;
    if manifest["type"] != "shortcut" {
        return Err("Тип манифеста должен быть shortcut.");
    }
    let body = serde_json::json!({"type":"shortcut", "title":title, "url":url.as_str()});
    fs::create_dir_all(directory).map_err(|_| "Не удалось создать папку модулей.")?;
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(directory.join(format!("{id}.json")))
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                "Модуль с таким id уже существует."
            } else {
                "Не удалось сохранить модуль."
            }
        })?;
    file.write_all(body.to_string().as_bytes())
        .map_err(|_| "Не удалось записать манифест.")
}
