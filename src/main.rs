mod auth;

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver};
use std::thread;

use log::{debug, info, LevelFilter};
use simple_logger::SimpleLogger;

use oauth2::url::Url;

use wry::application::accelerator::{Accelerator, SysMods};
use wry::application::event::{Event, WindowEvent};
use wry::application::event_loop::{ControlFlow, EventLoop, EventLoopProxy};
use wry::application::keyboard::KeyCode;
use wry::application::menu::{CustomMenuItem, MenuBar, MenuItem, MenuItemAttributes, MenuType};
use wry::application::window::{Window, WindowBuilder};
use wry::http::{Request, Response, ResponseBuilder};
use wry::webview::{RpcRequest, WebViewBuilder};
use wry::Value;

const INITIALIZATION_SCRIPT: &str = r#"
    window.addEventListener('DOMContentLoaded', function(event) {
        var url = window.location.toString();

        if (url.startsWith("https://auth.tesla.com/void/callback")) {
            location.replace("wry://index.html?access=loading...&refresh=loading...");
        }

        rpc.call('url', url);
    });
"#;

#[derive(Debug, Clone)]
enum CustomEvent {
    Tokens(auth::Tokens),
}

fn main() -> wry::Result<()> {
    SimpleLogger::new()
        .with_level(LevelFilter::Off)
        .with_module_level("reqwest", LevelFilter::Debug)
        .with_module_level("tesla_auth", LevelFilter::Debug)
        .init()
        .unwrap();

    let event_loop = EventLoop::<CustomEvent>::with_user_event();
    let event_proxy = event_loop.create_proxy();

    let (tx, rx) = channel();

    let handler = move |_window: &Window, req: RpcRequest| {
        if req.method == "url" {
            let url = parse_url(req.params.unwrap());
            tx.send(url).unwrap();
        }

        None
    };

    let mut client = auth::Client::new();
    let auth_url = client.authorization_url();

    thread::spawn(move || {
        handle_url_changes(rx, client, event_proxy);
    });

    let (menu, quit_item) = build_menu();

    let window = WindowBuilder::new()
        .with_title("Tesla Auth")
        .with_menu(menu)
        .build(&event_loop)?;

    let webview = WebViewBuilder::new(window)?
        .with_initialization_script(INITIALIZATION_SCRIPT)
        .with_custom_protocol("wry".into(), protocol_handler)
        .with_url(auth_url.as_str())?
        .with_rpc_handler(handler)
        .build()?;

    debug!("Opening {} ...", auth_url);

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *control_flow = ControlFlow::Exit,
            Event::UserEvent(CustomEvent::Tokens(tokens)) => {
                info!("Received tokens: {:#?}", tokens);

                let url = format!(
                    "location.replace('wry://index.html?access={}&refresh={}');",
                    tokens.access, tokens.refresh
                );

                webview.evaluate_script(&url).unwrap();
            }
            Event::MenuEvent {
                menu_id,
                origin: MenuType::MenuBar,
                ..
            } => {
                if menu_id == quit_item.clone().id() {
                    *control_flow = ControlFlow::Exit;
                }
                println!("Clicked on {:?}", menu_id);
            }
            _ => (),
        }
    });
}

fn build_menu() -> (MenuBar, CustomMenuItem) {
    let mut menu_bar_menu = MenuBar::new();
    let mut menu = MenuBar::new();

    menu.add_native_item(MenuItem::About("Todos".to_string()));
    menu.add_native_item(MenuItem::Services);
    menu.add_native_item(MenuItem::Separator);
    menu.add_native_item(MenuItem::Hide);
    let quit_item = menu.add_item(
        MenuItemAttributes::new("Quit")
            .with_accelerators(&Accelerator::new(SysMods::Cmd, KeyCode::KeyQ)),
    );
    menu.add_native_item(MenuItem::Copy);
    menu.add_native_item(MenuItem::Paste);

    menu_bar_menu.add_submenu("First menu", true, menu);

    (menu_bar_menu, quit_item)
}

fn handle_url_changes(
    rx: Receiver<Url>,
    mut client: auth::Client,
    event_proxy: EventLoopProxy<CustomEvent>,
) {
    let mut tokens_retrieved = false;

    while let Ok(url) = rx.recv() {
        if !auth::is_redirect_url(&url) || tokens_retrieved {
            debug!("URL changed: {}", &url);
            continue;
        }

        let query: HashMap<_, _> = url.query_pairs().collect();

        let state = query.get("state").expect("No state parameter found");
        let code = query.get("code").expect("No code parameter found");

        client.verify_csrf_state(state.to_string());

        let tokens = client.retrieve_tokens(code);

        tokens_retrieved = true;

        event_proxy.send_event(CustomEvent::Tokens(tokens)).unwrap();
    }
}

fn protocol_handler(request: &Request) -> wry::Result<Response> {
    let url: Url = request.uri().parse()?;

    match url.domain() {
        Some("index.html") => {
            let query = url.query_pairs().collect::<HashMap<_, _>>();

            let (access, refresh) = (query.get("access").unwrap(), query.get("refresh").unwrap());

            let content = include_str!("../views/index.html")
                .replace("{access_token}", access)
                .replace("{refresh_token}", refresh);

            ResponseBuilder::new()
                .mimetype("text/html")
                .body(content.as_bytes().to_vec())
        }

        domain => unimplemented!("Cannot open {:?}", domain),
    }
}

fn parse_url(params: Value) -> Url {
    let args = serde_json::from_value::<Vec<String>>(params).unwrap();
    let url = args.first().unwrap();
    Url::parse(url).expect("Invalid URL")
}
