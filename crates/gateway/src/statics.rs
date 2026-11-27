//! Static files: `/hmi/`, `/studio/`, `/shared/` from the web root.

use axum::body::Body;
use axum::extract::{Path, Request};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;
use std::path::PathBuf;
use tower_http::services::ServeDir;

/// URL prefix → folder under the web root.
pub const MOUNTS: [&str; 3] = ["hmi", "studio", "shared"];

/// Rejects anything that could leave the served folder, before the file
/// service sees it: `..`, backslashes, drive letters, NUL, encoded separators.
/// The file service has its own checks; this is a second, simpler line.
pub(crate) fn is_safe(raw: &str) -> bool {
    let Ok(decoded) = percent_encoding::percent_decode_str(raw).decode_utf8() else {
        return false;
    };
    !decoded.contains('\\')
        && !decoded.contains(':')
        && !decoded.contains('\0')
        && decoded.split('/').all(|seg| seg != ".." && seg != ".")
}

pub(crate) fn routes(web_root: PathBuf) -> Router {
    let mut router = Router::new().route("/", get(|| async { Redirect::temporary("/hmi/") }));
    for mount in MOUNTS {
        let dir = web_root.join(mount);
        let index_dir = dir.clone();
        let target = format!("/{mount}/");
        router = router
            .route(&format!("/{mount}"), get(move || async move { Redirect::permanent(&target) }))
            .route(&format!("/{mount}/"), get(move |req: Request| serve(index_dir.clone(), String::new(), req)))
            .route(
                &format!("/{mount}/*path"),
                get(move |Path(path): Path<String>, req: Request| serve(dir.clone(), path, req)),
            );
    }
    router
}

async fn serve(dir: PathBuf, path: String, req: Request) -> Response {
    // `Path` hands us the decoded value; check the raw one too.
    let raw = req.uri().path().to_string();
    if !is_safe(&path) || !is_safe(&raw) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let mut inner = Request::new(Body::empty());
    *inner.method_mut() = req.method().clone();
    *inner.headers_mut() = req.headers().clone();
    let uri = format!("/{}", path.trim_start_matches('/'));
    match uri.parse() {
        Ok(u) => *inner.uri_mut() = u,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    }
    match ServeDir::new(dir).append_index_html_on_directories(true).try_call(inner).await {
        Ok(resp) => {
            let mut resp = resp.map(Body::new);
            // Project files change while engineering; make browsers revalidate.
            resp.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
            resp
        }
        Err(e) => {
            tracing::warn!("static file error: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_safe;

    #[test]
    fn traversal_is_rejected() {
        for bad in ["../x", "a/../../x", "%2e%2e/x", "a\\..\\x", "a%5c..%5cx", "c:/windows", "x%00", "./x"] {
            assert!(!is_safe(bad), "{bad}");
        }
        for ok in ["index.html", "js/app.js", "a..b/c.txt", "img/pipe%20h.svg"] {
            assert!(is_safe(ok), "{ok}");
        }
    }
}
