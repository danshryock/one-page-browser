use crate::HOME_URL;

pub struct SearchEngine {
    pub name: String,
    /// Query URL with `{query}` where the URL-encoded search text goes.
    pub query_url_template: String,
}

pub struct Settings {
    pub start_page: String,
    pub search_engines: Vec<SearchEngine>,
    /// Name of the entry in `search_engines` that's preferred — a name
    /// rather than an index so it doesn't go stale if the list is ever
    /// edited (e.g. by a future settings UI).
    pub default_search_engine: String,
    /// How many pages may stay loaded at once. `None` means unlimited.
    pub max_loaded_pages: Option<usize>,
}

impl Settings {
    pub fn default_search_engine(&self) -> Option<&SearchEngine> {
        self.search_engines.iter().find(|e| e.name == self.default_search_engine)
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            start_page: HOME_URL.to_string(),
            search_engines: vec![
                SearchEngine {
                    name: "Google".to_string(),
                    query_url_template: "https://www.google.com/search?q={query}".to_string(),
                },
                SearchEngine {
                    name: "DuckDuckGo".to_string(),
                    query_url_template: "https://duckduckgo.com/?q={query}".to_string(),
                },
                SearchEngine {
                    name: "Bing".to_string(),
                    query_url_template: "https://www.bing.com/search?q={query}".to_string(),
                },
            ],
            default_search_engine: "Google".to_string(),
            max_loaded_pages: Some(10),
        }
    }
}
