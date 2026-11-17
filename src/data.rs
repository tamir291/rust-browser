use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub struct Data {
    path: PathBuf,
    pub bookmarks: Vec<Value>,
    pub history: Vec<String>,
}

impl Data {
    pub fn load(profile: &Path) -> Self {
        let path = profile.join("browser-data.json");
        let disk: Value = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            path,
            bookmarks: disk["bookmarks"].as_array().cloned().unwrap_or_else(|| {
                vec![json!({"url":"https://www.youtube.com/","name":"YouTube"})]
            }),
            history: disk["history"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect(),
        }
    }

    pub fn record(&mut self, query: String) {
        if self.history.first() != Some(&query) {
            self.history.retain(|entry| entry != &query);
            self.history.insert(0, query);
            self.history.truncate(150);
            self.save();
        }
    }

    pub fn toggle_bookmark(&mut self, url: &url::Url) {
        if let Some(index) = self
            .bookmarks
            .iter()
            .position(|entry| entry["url"] == url.as_str())
        {
            self.bookmarks.remove(index);
        } else if self.bookmarks.len() < 24 {
            self.bookmarks
                .push(json!({"url":url.as_str(), "name":url.host_str().unwrap_or_default()}));
        }
        self.save();
    }

    pub fn save(&self) {
        if let Err(error) = fs::write(
            &self.path,
            json!({"bookmarks": self.bookmarks, "history": self.history}).to_string(),
        ) {
            eprintln!("Could not save browser data: {error}");
        }
    }
}

pub fn search_url(query: &str) -> url::Url {
    let mut url = url::Url::parse("https://duckduckgo.com/").expect("fixed URL");
    url.query_pairs_mut().append_pair("q", query);
    url
}

pub fn profile_name(name: &str) -> Option<&str> {
    let name = name.trim();
    (!name.is_empty()
        && name.chars().count() <= 30
        && name
            .chars()
            .all(|char| char.is_alphanumeric() || matches!(char, '-' | '_' | ' ')))
    .then_some(name)
}
