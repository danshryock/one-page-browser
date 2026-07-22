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

        // WebKitGTK's fire-and-forget `evaluate_script` (no callback, used by
        // go_back/go_forward) is unreliable on a freshly-built webview — it
        // can silently fail to execute unless preceded by a callback-based
        // evaluation, and that priming call itself only works reliably once
        // at least one GTK main-loop iteration has run after `build_gtk`
        // (draining whatever realize/map events that call queued). Skipping
        // either half of this reproduced the bug ~50-100% of the time in
        // testing; doing both fixed it 10/10. Root cause is unclear — this
        // is a cheap, empirically-verified workaround, not a full fix upstream.
        while gtk::events_pending() {
            gtk::main_iteration_do(false);
        }
        webview.evaluate_script_with_callback("true", |_| {})?;

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
