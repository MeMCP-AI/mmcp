//! Single group detail page showing its memories.
//!
//! Calls the `list_memories` server function (which hits
//! `POST /mcp/tool` with `{ tool: "list_memories", ... }`) and
//! renders the returned descriptors.

use leptos::prelude::*;
use leptos_router::hooks::use_params_map;

use crate::api::{self, MemoryDescriptor};
use crate::app::TokenSignal;

#[component]
pub fn GroupDetailPage() -> impl IntoView {
    let params = use_params_map();
    let group_id = Signal::derive(move || params.with(|p| p.get("id").unwrap_or_default()));
    let token = expect_context::<TokenSignal>();

    let memories = Resource::new(
        move || (group_id.get(), token.get()),
        |(id, t)| async move {
            if id.is_empty() {
                return Ok(Vec::new());
            }
            api::list_memories(t, id).await
        },
    );

    view! {
        <h2>"Group: " {move || group_id.get()}</h2>
        <p><a href="/">"← Back to groups"</a></p>
        <Suspense fallback=|| view! { <p>"Loading memories..."</p> }>
            {move || {
                memories.get().map(|result| match result {
                    Ok(mems) if mems.is_empty() => view! {
                        <p>"No memories in this group yet."</p>
                    }.into_any(),
                    Ok(mems) => view! {
                        <ul>
                            <For
                                each=move || mems.clone()
                                key=|m| m.id.clone()
                                children=|m: MemoryDescriptor| view! {
                                    <li>
                                        <strong>{m.name.clone()}</strong>
                                        " "
                                        <code style="color:#888;font-size:0.85em">
                                            {format!("[{}]", m.kind)}
                                        </code>
                                        {m.mandatory.then(|| view! {
                                            " "
                                            <span style="color:#b00;font-weight:600">"(mandatory)"</span>
                                        })}
                                        <br />
                                        <span style="color:#555">{m.description.clone()}</span>
                                    </li>
                                }
                            />
                        </ul>
                    }.into_any(),
                    Err(e) => view! {
                        <p style="color:red">{format!("Failed to load memories: {e}")}</p>
                    }.into_any(),
                })
            }}
        </Suspense>
    }
}
