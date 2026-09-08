//! Switching the web-mode port bridge off, in its own process: `configure(None)`
//! closes every listener in the process, which would fail any other test
//! sharing the binary.

use std::time::Duration;

use axum::http::header;
use axum::Router;
use codeg_lib::web::browser_bridge::{self, BridgeConfig, BridgeGrant};

fn configure() {
    browser_bridge::configure(Some(BridgeConfig {
        bind_host: "127.0.0.1".to_string(),
        ports: vec![0],
        public_host: None,
        reserved: vec![1],
    }));
}

async fn spawn_upstream() -> u16 {
    let app = Router::new().route("/hello", axum::routing::get(|| async { "hello" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    port
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .unwrap()
}

fn cookie_for(grant: &BridgeGrant) -> String {
    let cap = grant
        .entry_path
        .strip_prefix(browser_bridge::ENTER_PREFIX)
        .unwrap();
    format!("codeg-bridge-{}={cap}", grant.bridge_port)
}

fn base(grant: &BridgeGrant) -> String {
    format!("http://127.0.0.1:{}", grant.bridge_port)
}

#[tokio::test]
async fn switching_the_bridge_off_closes_everything_and_refuses_new_opens() {
    configure();
    let upstream = spawn_upstream().await;
    let grant = browser_bridge::open(upstream, "tab-off").await.unwrap();
    browser_bridge::configure(None);
    assert_eq!(browser_bridge::listener_count(), 0);
    assert!(matches!(
        browser_bridge::open(upstream, "tab-off-2").await,
        Err(browser_bridge::BridgeError::Disabled)
    ));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(client()
        .get(format!("{}/hello", base(&grant)))
        .header(header::COOKIE, cookie_for(&grant))
        .send()
        .await
        .is_err());
    // Back on: opens work again.
    configure();
    let again = browser_bridge::open(upstream, "tab-on").await.unwrap();
    assert_ne!(again.bridge_port, 0);
    browser_bridge::close("tab-on");
}

