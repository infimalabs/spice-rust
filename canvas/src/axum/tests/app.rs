//! Immutable app response integration checks.

use super::*;

#[tokio::test(flavor = "current_thread")]
async fn etags_revalidate_without_retransmitting_the_asset() {
    let app = TestApp::new();
    let (server, _control) = Server::bind(generation(&app, Capabilities::default()))
        .await
        .expect("binds");
    let address = server.local_addr().expect("local address");
    let task = tokio::spawn(server.up());

    let initial = request_with_headers(
        address,
        &address.to_string(),
        "/",
        &[("If-None-Match", "\"not-this-asset\"")],
    )
    .await;
    assert!(initial.starts_with(b"HTTP/1.1 200 OK\r\n"));
    let etag = response_header(&initial, "etag")
        .expect("the app response carries an ETag")
        .to_owned();
    let content_type = response_header(&initial, "content-type")
        .expect("the app response carries its media type")
        .to_owned();
    let policy = response_header(&initial, "content-security-policy")
        .expect("the app shell carries its CSP")
        .to_owned();
    assert_eq!(response_header(&initial, "cache-control"), Some("no-cache"));

    let revalidated = request_with_headers(
        address,
        &address.to_string(),
        "/",
        &[("If-None-Match", &etag)],
    )
    .await;
    assert!(revalidated.starts_with(b"HTTP/1.1 304 Not Modified\r\n"));
    assert!(response_body(&revalidated).is_empty());
    assert_eq!(response_header(&revalidated, "etag"), Some(etag.as_str()));
    assert_eq!(
        response_header(&revalidated, "content-type"),
        Some(content_type.as_str())
    );
    assert_eq!(
        response_header(&revalidated, "content-security-policy"),
        Some(policy.as_str())
    );
    assert_eq!(
        response_header(&revalidated, "cache-control"),
        Some("no-cache")
    );

    let wasm_route = format!(
        "{}/{FIXTURE_GENERATION}/app_bg.wasm",
        crate::APP_GENERATION_ROUTE
    );
    let nonmatching = request_with_headers(
        address,
        &address.to_string(),
        &wasm_route,
        &[("If-None-Match", "\"not-this-asset\"")],
    )
    .await;
    assert!(nonmatching.starts_with(b"HTTP/1.1 200 OK\r\n"));
    assert_eq!(response_body(&nonmatching), b"test app app_bg.wasm");
    assert_eq!(
        response_header(&nonmatching, "content-type"),
        Some("application/wasm")
    );
    let wasm_etag = response_header(&nonmatching, "etag")
        .expect("the generation-qualified response carries an ETag")
        .to_owned();
    let wasm_revalidated = request_with_headers(
        address,
        &address.to_string(),
        &wasm_route,
        &[("If-None-Match", &wasm_etag)],
    )
    .await;
    assert!(wasm_revalidated.starts_with(b"HTTP/1.1 304 Not Modified\r\n"));
    assert!(response_body(&wasm_revalidated).is_empty());
    assert_eq!(
        response_header(&wasm_revalidated, "etag"),
        Some(wasm_etag.as_str())
    );
    assert_eq!(
        response_header(&wasm_revalidated, "content-type"),
        Some("application/wasm")
    );

    task.abort();
    assert!(task.await.expect_err("aborted server task").is_cancelled());
}
