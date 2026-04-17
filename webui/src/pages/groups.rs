//! Groups listing page.
//!
//! Fetches `/sync/manifest` on mount via the `list_manifest` server
//! function, using the bearer token from context if present. Groups
//! link to their detail pages by UUID.

use leptos::prelude::*;

use crate::api::{self, RemoteGroup};
use crate::app::TokenSignal;

#[component]
pub fn GroupsPage() -> impl IntoView {
    let token = expect_context::<TokenSignal>();

    // Resource re-runs whenever the token changes so the listing
    // refreshes on login/logout without a full reload.
    let groups = Resource::new(
        move || token.get(),
        |t| async move { api::list_manifest(t).await },
    );

    view! {
        <h2>"Groups"</h2>
        <Suspense fallback=|| view! { <p>"Loading groups..."</p> }>
            {move || {
                groups.get().map(|result| match result {
                    Ok(groups) if groups.is_empty() => view! {
                        <p>"No groups visible to this account yet."</p>
                    }.into_any(),
                    Ok(groups) => view! {
                        <ul>
                            <For
                                each=move || groups.clone()
                                key=|g| g.group_id.clone()
                                children=|g: RemoteGroup| view! {
                                    <li>
                                        <a href=format!("/groups/{}", g.group_id)>
                                            {g.slug.clone()}
                                        </a>
                                        " "
                                        <code style="color:#888;font-size:0.85em">
                                            {g.head_commit.chars().take(7).collect::<String>()}
                                        </code>
                                    </li>
                                }
                            />
                        </ul>
                    }.into_any(),
                    Err(e) => view! {
                        <p style="color:red">{format!("Failed to load groups: {e}")}</p>
                    }.into_any(),
                })
            }}
        </Suspense>
    }
}
