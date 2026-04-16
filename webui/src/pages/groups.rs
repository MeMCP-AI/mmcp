//! Groups listing page.

use leptos::prelude::*;

#[component]
pub fn GroupsPage() -> impl IntoView {
    view! {
        <h2>"Groups"</h2>
        <p>"Sign in to see your groups."</p>
        <ul>
            <li>
                <em>"Group listing will be populated from /sync/manifest once connected to a running mmcp-server."</em>
            </li>
        </ul>
    }
}
