//! Top-level Leptos app component with router.

use leptos::prelude::*;
use leptos_meta::*;
use leptos_router::components::*;
use leptos_router::path;

use crate::pages;

/// Root component rendered on every page.
#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    view! {
        <Title text="mmcp" />

        <Router>
            <header>
                <h1>"mmcp"</h1>
                <nav>
                    <a href="/">"Groups"</a>
                    " | "
                    <a href="/login">"Login"</a>
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
