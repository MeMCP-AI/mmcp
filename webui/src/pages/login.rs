//! Password login page.

use leptos::prelude::*;
use leptos_router::NavigateOptions;
use leptos_router::hooks::use_navigate;

use crate::api;
use crate::app::TokenSignal;
use crate::auth;

#[component]
pub fn LoginPage() -> impl IntoView {
    let (handle, set_handle) = signal(String::new());
    let (password, set_password) = signal(String::new());
    let (error_msg, set_error_msg) = signal(Option::<String>::None);
    let (pending, set_pending) = signal(false);
    let token = expect_context::<TokenSignal>();
    let navigate = use_navigate();

    let on_submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let h = handle.get();
        let p = password.get();
        let navigate = navigate.clone();
        leptos::task::spawn_local(async move {
            set_error_msg.set(None);
            set_pending.set(true);
            match api::login(h, p).await {
                Ok(ok) => {
                    auth::save_token(&ok.token);
                    token.set(Some(ok.token));
                    set_pending.set(false);
                    navigate("/", NavigateOptions::default());
                }
                Err(e) => {
                    set_pending.set(false);
                    set_error_msg.set(Some(e.to_string()));
                }
            }
        });
    };

    view! {
        <h2>"Login"</h2>
        <form on:submit=on_submit>
            <div>
                <label for="handle">"Handle"</label>
                <input
                    id="handle"
                    type="text"
                    prop:value=handle
                    on:input=move |ev| set_handle.set(event_target_value(&ev))
                />
            </div>
            <div>
                <label for="password">"Password"</label>
                <input
                    id="password"
                    type="password"
                    prop:value=password
                    on:input=move |ev| set_password.set(event_target_value(&ev))
                />
            </div>
            <button type="submit" prop:disabled=pending>
                {move || if pending.get() { "Signing in..." } else { "Sign in" }}
            </button>
        </form>
        {move || error_msg.get().map(|msg| view! { <p style="color:red">{msg}</p> })}
    }
}
