//! Browser-side bearer-token storage.
//!
//! We persist the login token in `window.localStorage` so reloads
//! keep the session across page navigations. On the SSR side every
//! helper is a no-op, which lets pages call the same API regardless
//! of render path.

/// localStorage key used to persist the bearer token issued by
/// `POST /auth/login`.
pub const TOKEN_STORAGE_KEY: &str = "mmcp_token";

/// Load the bearer token previously saved by [`save_token`].
///
/// Always `None` on the SSR side, since the browser's localStorage
/// doesn't exist there.
#[must_use]
pub fn load_token() -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        let storage = web_sys::window()?.local_storage().ok()??;
        storage.get_item(TOKEN_STORAGE_KEY).ok()?
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        None
    }
}

/// Persist `token` in localStorage. No-op on SSR.
pub fn save_token(token: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
            let _ = storage.set_item(TOKEN_STORAGE_KEY, token);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = token;
    }
}

/// Clear any persisted token. No-op on SSR.
pub fn clear_token() {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
            let _ = storage.remove_item(TOKEN_STORAGE_KEY);
        }
    }
}
