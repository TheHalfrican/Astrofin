use cef::*;
use std::os::raw::c_int;
use std::sync::Arc;

use crate::client::Inner;

mod context_menu;
mod dialog;
mod display;
mod keyboard;
mod lifespan;
mod load;
mod os_ffi;
mod process_message;
mod render;
use context_menu::JfnContextMenuHandlerBuilder;
use dialog::JfnDialogHandlerBuilder;
use display::JfnDisplayHandlerBuilder;
use keyboard::JfnKeyboardHandlerBuilder;
use lifespan::JfnLifeSpanHandlerBuilder;
use load::JfnLoadHandlerBuilder;
use render::JfnRenderHandlerBuilder;

pub fn make_client(inner: Arc<Inner>) -> Client {
    JfnClientBuilder::new(inner)
}

wrap_client! {
    pub struct JfnClientBuilder {
        inner: Arc<Inner>,
    }

    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(JfnRenderHandlerBuilder::new(self.inner.clone()))
        }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(JfnLifeSpanHandlerBuilder::new(self.inner.clone()))
        }
        fn load_handler(&self) -> Option<LoadHandler> {
            Some(JfnLoadHandlerBuilder::new(self.inner.clone()))
        }
        fn context_menu_handler(&self) -> Option<ContextMenuHandler> {
            Some(JfnContextMenuHandlerBuilder::new(self.inner.clone()))
        }
        fn dialog_handler(&self) -> Option<DialogHandler> {
            Some(JfnDialogHandlerBuilder::new(self.inner.clone()))
        }
        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(JfnDisplayHandlerBuilder::new(self.inner.clone()))
        }
        fn keyboard_handler(&self) -> Option<KeyboardHandler> {
            Some(JfnKeyboardHandlerBuilder::new(self.inner.clone()))
        }
        fn on_process_message_received(
            &self,
            browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _source_process: ProcessId,
            message: Option<&mut ProcessMessage>,
        ) -> c_int {
            process_message::on_process_message_received(&self.inner, browser, message)
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn make_client_builds_a_client_that_offers_every_handler() {
        let inner = Inner::new_detached();
        let client = make_client(Arc::clone(&inner));
        // The vtable is built in Rust; none of these slots may be empty or
        // CEF silently loses the callback.
        assert!(client.render_handler().is_some());
        assert!(client.life_span_handler().is_some());
        assert!(client.load_handler().is_some());
        assert!(client.context_menu_handler().is_some());
        assert!(client.dialog_handler().is_some());
        assert!(client.display_handler().is_some());
        assert!(client.keyboard_handler().is_some());
    }

    #[test]
    fn make_client_shares_the_inner_it_was_given() {
        let inner = Inner::new_detached();
        let before = Arc::strong_count(&inner);
        let client = make_client(Arc::clone(&inner));
        assert!(Arc::strong_count(&inner) > before);
        drop(client);
    }
}
