//! Top-level Leptos app component with router.

use leptos::prelude::*;
use leptos_meta::*;
use leptos_router::components::*;
use leptos_router::path;

use crate::auth;
use crate::pages;

/// Global bearer-token signal shared across pages through Leptos
/// context. `None` means "not logged in".
///
/// We expose the full `RwSignal` rather than a `Signal<Option<String>>`
/// so the login page can write to it without reaching into a
/// separate setter.
pub type TokenSignal = RwSignal<Option<String>>;

/// Root component rendered on every page.
#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    // NOTE(gg 2026-04-17): on SSR `load_token()` returns `None`; the
    // client then rehydrates from localStorage on mount. Good enough
    // until we move the token into an HttpOnly cookie.
    let token: TokenSignal = RwSignal::new(auth::load_token());
    provide_context(token);

    view! {
        <Title text="mmcp" />

        <Router>
            <header>
                <h1>"mmcp"</h1>
                <nav>
                    <a href="/">"Groups"</a>
                    " | "
                    <a href="/login">"Login"</a>
                    {move || token.get().map(|_| view! {
                        " | "
                        <a href="#" on:click=move |ev| {
                            ev.prevent_default();
                            auth::clear_token();
                            token.set(None);
                        }>"Logout"</a>
                    })}
                </nav>
            </header>
            <main>
                <Routes fallback=|| "Page not found.">
                    <Route path=path!("/") view=pages::groups::GroupsPage />
                    <Route path=path!("/login") view=pages::login::LoginPage />
                    <Route path=path!("/groups/:id") view=pages::group_detail::GroupDetailPage />
                </Routes>
            </main>
        </Router>
    }
}
