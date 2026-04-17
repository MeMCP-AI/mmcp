//! SSR entry point for the mmcp WebUI.

#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() {
    use axum::Router;
    use leptos::prelude::*;
    use leptos_axum::{LeptosRoutes, generate_route_list};
    use mmcp_webui::app::App;

    let conf = get_configuration(None).unwrap();
    let leptos_options = conf.leptos_options;
    let addr = leptos_options.site_addr;
    let routes = generate_route_list(App);

    let shell = move |options: LeptosOptions| {
        let opt1 = options.clone();
        let opt2 = options;
        view! {
            <!DOCTYPE html>
            <html lang="en">
                <head>
                    <meta charset="utf-8" />
                    <meta name="viewport" content="width=device-width, initial-scale=1" />
                    <leptos_meta::MetaTags />
                    <AutoReload options=opt1 />
                    <HydrationScripts options=opt2 />
                    <link rel="stylesheet" href="/pkg/style.css" />
                </head>
                <body>
                    <App />
                </body>
            </html>
        }
    };

    let opts_for_routes = leptos_options.clone();
    let app = Router::new()
        .leptos_routes(&leptos_options, routes, move || {
            shell(opts_for_routes.clone())
        })
        .fallback(leptos_axum::file_and_error_handler(shell))
        .with_state(leptos_options);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    println!("mmcp webui listening on http://{addr}");
    axum::serve(listener, app.into_make_service())
        .await
        .unwrap();
}

#[cfg(not(feature = "ssr"))]
fn main() {}
