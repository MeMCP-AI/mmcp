//! Single group detail page showing its memories.

use leptos::prelude::*;
use leptos_router::hooks::use_params_map;

#[component]
pub fn GroupDetailPage() -> impl IntoView {
    let params = use_params_map();
    let group_id = move || {
        params.with(|p| p.get("id").unwrap_or_default())
    };

    view! {
        <h2>"Group: " {group_id}</h2>
        <p>"Memory listing will be populated from /mcp/tool once connected to a running mmcp-server."</p>
    }
}
