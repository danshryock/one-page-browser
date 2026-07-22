use gtk::Container;
use wry::{WebView, WebViewBuilder, WebViewBuilderExtUnix};

use crate::RenderEngine;

pub struct WryEngine {
    webview: WebView,
}

impl WryEngine {
    pub fn new<W: gtk::glib::IsA<Container>>(
        container: &W,
        initial_url: &str,
        on_title_changed: impl Fn(String) + 'static,
    ) -> anyhow::Result<Self> {
        let webview = WebViewBuilder::new()
            .with_url(initial_url)
            .with_document_title_changed_handler(move |title| on_title_changed(title))
            .build_gtk(container)?;
        Ok(Self { webview })
    }
}

impl RenderEngine for WryEngine {
    fn navigate(&self, url: &str) -> anyhow::Result<()> {
        self.webview.load_url(url)?;
        Ok(())
    }

    fn current_url(&self) -> anyhow::Result<String> {
        Ok(self.webview.url()?)
    }

    fn go_back(&self) -> anyhow::Result<()> {
        self.webview.evaluate_script("window.history.back()")?;
        Ok(())
    }

    fn go_forward(&self) -> anyhow::Result<()> {
        self.webview.evaluate_script("window.history.forward()")?;
        Ok(())
    }

    fn reload(&self) -> anyhow::Result<()> {
        self.webview.reload()?;
        Ok(())
    }
}
