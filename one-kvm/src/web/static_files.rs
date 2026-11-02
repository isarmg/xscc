use axum::{
    body::Body,
    http::{header, HeaderMap, Response, StatusCode, Uri},
    routing::get,
    Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use bytes::Bytes;
use rust_embed::Embed;
use sha2::{Digest, Sha256};
use std::{borrow::Cow, path::PathBuf, sync::OnceLock};

#[derive(Embed)]
#[folder = "web/dist"]
pub struct StaticAssets;

struct Asset {
    body: Bytes,
    hash: [u8; 32],
}

fn frontend_dir() -> Option<&'static PathBuf> {
    static DIRECTORY: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIRECTORY
        .get_or_init(|| std::env::var_os("ONE_KVM_FRONTEND_DIR").map(PathBuf::from))
        .as_ref()
}

fn asset(path: &str) -> Option<Asset> {
    if let Some(base) = frontend_dir() {
        let base = base.canonicalize().ok()?;
        let file = base.join(path).canonicalize().ok()?;
        if !file.starts_with(&base) {
            return None;
        }
        let bytes = std::fs::read(file).ok()?;
        return Some(Asset {
            hash: Sha256::digest(&bytes).into(),
            body: bytes.into(),
        });
    }
    let asset = StaticAssets::get(path)?;
    Some(Asset {
        hash: asset.metadata.sha256_hash(),
        body: match asset.data {
            Cow::Borrowed(bytes) => Bytes::from_static(bytes),
            Cow::Owned(bytes) => bytes.into(),
        },
    })
}

pub fn static_file_router<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route("/", get(index_handler))
        .fallback_service(get(static_handler))
}

async fn index_handler(headers: HeaderMap) -> Response<Body> {
    serve("index.html", &headers).unwrap_or_else(not_found)
}

async fn static_handler(uri: Uri, headers: HeaderMap) -> Response<Body> {
    let path = uri.path().trim_start_matches('/');
    serve(path, &headers)
        .or_else(|| {
            if !path.contains('.') {
                serve("index.html", &headers)
            } else {
                None
            }
        })
        .unwrap_or_else(not_found)
}

fn accepts_gzip(headers: &HeaderMap) -> bool {
    let mut wildcard = false;
    for value in headers.get_all(header::ACCEPT_ENCODING) {
        let Ok(value) = value.to_str() else { continue };
        for entry in value.split(',') {
            let mut parts = entry.trim().split(';');
            let encoding = parts.next().unwrap_or_default().trim();
            let quality = parts
                .find_map(|part| part.trim().strip_prefix("q="))
                .map_or(Some(1.0), |q| q.parse::<f32>().ok());
            let enabled = quality.is_some_and(|q| q > 0.0 && q <= 1.0);
            if encoding.eq_ignore_ascii_case("gzip") {
                return enabled;
            }
            if encoding == "*" {
                wildcard = enabled;
            }
        }
    }
    wildcard
}

fn serve(path: &str, headers: &HeaderMap) -> Option<Response<Body>> {
    let html = path.ends_with(".html");
    let compressed = if !html && accepts_gzip(headers) {
        asset(&format!("{path}.gz"))
    } else {
        None
    };
    let gzip = compressed.is_some();
    let Asset { body, hash } = compressed.or_else(|| asset(path))?;
    let length = body.len();
    let etag = format!("\"{}\"", URL_SAFE_NO_PAD.encode(hash));
    let unchanged = !html
        && headers
            .get(header::IF_NONE_MATCH)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|value| {
                value.split(',').any(|value| {
                    let value = value.trim();
                    value == "*" || value.strip_prefix("W/").unwrap_or(value) == etag
                })
            });
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let mut response = Response::new(if unchanged {
        Body::empty()
    } else {
        Body::from(body)
    });
    *response.status_mut() = if unchanged {
        StatusCode::NOT_MODIFIED
    } else {
        StatusCode::OK
    };
    let output = response.headers_mut();
    output.insert(
        header::CONTENT_TYPE,
        mime.as_ref()
            .parse()
            .expect("invariant: MIME strings are valid headers"),
    );
    output.insert(
        header::ETAG,
        etag.parse()
            .expect("invariant: base64 digest is a valid header"),
    );
    output.insert(
        header::VARY,
        header::HeaderValue::from_static("Accept-Encoding"),
    );
    output.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static(if html {
            "no-store"
        } else {
            "public, max-age=0, must-revalidate"
        }),
    );
    output.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    output.insert(
        header::X_FRAME_OPTIONS,
        header::HeaderValue::from_static("SAMEORIGIN"),
    );
    output.insert(
        header::REFERRER_POLICY,
        header::HeaderValue::from_static("same-origin"),
    );
    if gzip {
        output.insert(
            header::CONTENT_ENCODING,
            header::HeaderValue::from_static("gzip"),
        );
    }
    if !unchanged {
        output.insert(
            header::CONTENT_LENGTH,
            length
                .to_string()
                .parse()
                .expect("invariant: decimal length is a valid header"),
        );
    }
    Some(response)
}

fn not_found() -> Response<Body> {
    let mut response = Response::new(Body::from(not_found_html()));
    *response.status_mut() = StatusCode::NOT_FOUND;
    response
}

pub fn not_found_html() -> String {
    "<!doctype html><title>404</title><h1>Not found</h1>".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_gzip_exclusion_overrides_wildcard() {
        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT_ENCODING, "*;q=1,gzip;q=0".parse().unwrap());
        assert!(!accepts_gzip(&headers));
        headers.insert(header::ACCEPT_ENCODING, "gzip;q=0.5".parse().unwrap());
        assert!(accepts_gzip(&headers));
    }
}
