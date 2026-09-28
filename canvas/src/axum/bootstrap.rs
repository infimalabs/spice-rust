use super::*;

pub(super) async fn bootstrap(
    State(state): State<Arc<SurfaceState>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
) -> Response {
    if !state.access.admits_token(query.as_deref()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Ok(host) = admit_headers(&state, &headers, false) else {
        return StatusCode::FORBIDDEN.into_response();
    };
    let Some(capability) = state.capabilities.operator() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let app_generation = match state.app.current_generation() {
        Ok(generation) => generation,
        Err(error) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                format!("canvas app unavailable: {error}"),
            )
                .into_response();
        }
    };
    // The socket this names has to be one the asking client can reach, which
    // the published endpoint is not when a remote browser is on the other end
    // of a wildcard bind. It reached this surface somehow; that is the address.
    let mut version = state.version.clone();
    version.app = app_generation.identity().clone();
    let discovery = wire::CanvasBootstrap {
        endpoint: format!("http://{host}"),
        capability,
        epoch: state.epoch,
        version,
        operator: state.operator.clone(),
    };
    let Ok(bytes) = wire::encode(&discovery) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        wire::hello::CANVAS_VERSION_HEADER,
        HeaderValue::from_str(&discovery.version.canvas.to_string())
            .expect("the numeric Canvas version is a valid header value"),
    );
    response.headers_mut().insert(
        wire::hello::CANVAS_GENERATION_HEADER,
        HeaderValue::from_str(app_generation.identity().as_str())
            .expect("the canonical app generation is a valid header value"),
    );
    install_browser_headers(&mut response);
    response
}
