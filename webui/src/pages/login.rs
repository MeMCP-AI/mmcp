//! Password login page.

use leptos::prelude::*;

#[component]
pub fn LoginPage() -> impl IntoView {
    let (handle, set_handle) = signal(String::new());
    let (password, set_password) = signal(String::new());
    let (error_msg, set_error_msg) = signal(Option::<String>::None);
    let (success_msg, set_success_msg) = signal(Option::<String>::None);

    let on_submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let h = handle.get();
        let p = password.get();
        leptos::task::spawn_local(async move {
            set_error_msg.set(None);
            set_success_msg.set(None);
            match do_login(&h, &p).await {
                Ok(msg) => set_success_msg.set(Some(msg)),
                Err(e) => set_error_msg.set(Some(e)),
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
            <button type="submit">"Sign in"</button>
        </form>
        {move || error_msg.get().map(|msg| view! { <p style="color:red">{msg}</p> })}
        {move || success_msg.get().map(|msg| view! { <p style="color:green">{msg}</p> })}
    }
}

async fn do_login(handle: &str, password: &str) -> Result<String, String> {
    // Client-side: call the mmcp-server's /auth/login endpoint.
    // The server URL would normally come from config; hardcoded
    // for the scaffold.
    let _ = (handle, password);
    Err("Login not yet wired to a running mmcp-server".to_string())
}
