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

fn embedded_generation() -> wire::Digest {
    wire::Digest::compute(wire::Domain::DistArtifact, b"embedded generation")
}

/// A complete generation archived the way staging writes it, held for the
/// `'static` lifetime an executable's linked archive has.
static EMBEDDED_ARCHIVE: std::sync::LazyLock<Vec<u8>> = std::sync::LazyLock::new(|| {
    use wire::manifest::renderer::{self, File, Kind, Member};
    let renderer_document = b"<script type=\"importmap\">{}</script>".to_vec();
    let declaration = renderer::encode(&[Member {
        digest: wire::Digest::compute(wire::Domain::InstallArtifact, &renderer_document),
        file: File::new(Kind::Document, 0).expect("index is in bounds"),
    }])
    .expect("declaration encodes");
    let generation = embedded_generation();
    let mut files = APP
        .assets()
        .iter()
        .map(|asset| {
            let bytes = if asset.path() == "/" {
                format!(
                    "<main id=\"app\" data-app-generation=\"{generation}\" {}=\"{declaration}\"></main>{INLINE_THEME_OPEN}theme();{INLINE_MODULE_CLOSE}{INLINE_MODULE_OPEN}start();{INLINE_MODULE_CLOSE}",
                    renderer::ATTRIBUTE
                )
            } else {
                format!("embedded {}", asset.file()).repeat(64)
            };
            (
                crate::rack::Namespace::App.member(asset.file()),
                bytes.into_bytes(),
            )
        })
        .collect::<Vec<_>>();
    files.push((
        crate::rack::Namespace::App.member("renderer-000.html"),
        renderer_document,
    ));
    crate::rack::Rack::write(
        files
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice())),
    )
    .expect("fixture archive writes")
});

#[tokio::test(flavor = "current_thread")]
async fn embedded_archive_serves_its_stored_gzip_slices_and_refuses_what_it_cannot() {
    let held = embedded_generation();
    let archive: &'static [u8] = &EMBEDDED_ARCHIVE;
    let stored = crate::rack::Rack::admit(archive).expect("fixture archive admits");
    let app = CanvasAssets::EmbeddedArchive(
        crate::canvas::EmbeddedArchive::admit(archive).expect("fixture archive indexes"),
    );
    let (server, _control) = Server::bind(Generation::new(
        epoch(),
        Capabilities::default(),
        app_version(&app),
        app,
    ))
    .await
    .expect("binds");
    let address = server.local_addr().expect("local address");
    let host = address.to_string();
    let task = tokio::spawn(server.up());

    let wasm = format!("{}/{held}/app_bg.wasm", crate::APP_GENERATION_ROUTE);
    for (route, member, accept) in [
        ("/", wire::manifest::APP_HTML, Some("br, gzip;q=0.5")),
        (wasm.as_str(), "app_bg.wasm", None),
    ] {
        let headers = accept.map(|accept| ("Accept-Encoding", accept));
        let response = request_with_headers(address, &host, route, headers.as_slice()).await;
        assert!(response.starts_with(b"HTTP/1.1 200 OK\r\n"), "{route}");
        let gzip = stored
            .member(&crate::rack::Namespace::App.member(member))
            .expect("member is archived")
            .gzip();
        assert_eq!(
            response_body(&response),
            gzip,
            "{route} is the stored slice"
        );
        assert_eq!(
            response_header(&response, "content-length"),
            Some(gzip.len().to_string().as_str())
        );
        assert_eq!(response_header(&response, "content-encoding"), Some("gzip"));
        assert_eq!(response_header(&response, "vary"), Some("accept-encoding"));
        assert_eq!(
            response_header(&response, "etag"),
            Some(format!("\"{held}/{member}\"").as_str())
        );
    }
    let shell = request(address, &host, "/").await;
    assert!(response_header(&shell, "content-security-policy").is_some());

    for refused in ["identity", "gzip;q=0, *", "br"] {
        let response =
            request_with_headers(address, &host, &wasm, &[("Accept-Encoding", refused)]).await;
        assert!(
            response.starts_with(b"HTTP/1.1 406 Not Acceptable\r\n"),
            "{refused}"
        );
    }

    let retired = wire::Digest::compute(wire::Domain::DistArtifact, b"retired generation");
    let gone = request(
        address,
        &host,
        &format!("{}/{retired}/app.module", crate::APP_GENERATION_ROUTE),
    )
    .await;
    assert!(gone.starts_with(b"HTTP/1.1 410 Gone\r\n"));
    assert_eq!(
        response_header(&gone, wire::hello::APP_GENERATION_HEADER),
        Some(held.as_str())
    );
    assert_eq!(response_header(&gone, "cache-control"), Some("no-store"));

    task.abort();
    assert!(task.await.expect_err("aborted server task").is_cancelled());
}

#[tokio::test(flavor = "current_thread")]
async fn development_directory_answers_gone_for_a_generation_it_no_longer_holds() {
    let app = TestApp::new();
    let (server, _control) = Server::bind(generation(&app, Capabilities::default()))
        .await
        .expect("binds");
    let address = server.local_addr().expect("local address");
    let task = tokio::spawn(server.up());

    let retired = wire::Digest::compute(wire::Domain::DistArtifact, b"pruned generation");
    let gone = request(
        address,
        &address.to_string(),
        &format!("{}/{retired}/app_bg.wasm", crate::APP_GENERATION_ROUTE),
    )
    .await;
    assert!(gone.starts_with(b"HTTP/1.1 410 Gone\r\n"));
    assert_eq!(
        response_header(&gone, wire::hello::APP_GENERATION_HEADER),
        Some(FIXTURE_GENERATION)
    );

    task.abort();
    assert!(task.await.expect_err("aborted server task").is_cancelled());
}
