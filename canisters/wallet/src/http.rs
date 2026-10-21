//! The wallet's pages, as decision 0001 says: files embedded in the release
//! with `include_bytes!`, certified in `init` and `post_upgrade`, and served
//! by `http_request`. Every response for a path, the 404 included, carries a
//! certificate and the security headers below.

use crate::HttpResponse;
use ic_asset_certification::{Asset, AssetConfig, AssetFallbackConfig, AssetRouter};
use ic_http_certification::{HttpRequest, StatusCode};
use serde_bytes::ByteBuf;
use std::cell::RefCell;

/// The Content-Security-Policy of every page. Nothing is allowed unless named:
/// scripts, styles, images and fonts come only from the wallet's own origin,
/// calls go only to it and to the IC API hosts the agent uses, and nothing may
/// frame the wallet, be framed by it, post a form or change its base URL.
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'none'; \
    script-src 'self'; \
    style-src 'self'; \
    img-src 'self' data:; \
    font-src 'self'; \
    connect-src 'self' https://icp-api.io https://icp0.io; \
    frame-src 'none'; \
    frame-ancestors 'none'; \
    object-src 'none'; \
    base-uri 'none'; \
    form-action 'none'; \
    upgrade-insecure-requests";

const INDEX_HTML: &[u8] = include_bytes!("../pages/index.html");
const NOT_FOUND_HTML: &[u8] = include_bytes!("../pages/404.html");
const HTML: &str = "text/html; charset=utf-8";

/// Served and certified with every page, so a replica can neither drop nor
/// change them.
pub fn security_headers() -> Vec<(String, String)> {
    [
        ("content-security-policy", CONTENT_SECURITY_POLICY),
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
        ("referrer-policy", "no-referrer"),
        (
            "strict-transport-security",
            "max-age=31536000; includeSubDomains",
        ),
        // Pages change only with a release; revalidate them on every visit.
        ("cache-control", "no-cache"),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_string(), value.to_string()))
    .collect()
}

thread_local! {
    static ROUTER: RefCell<AssetRouter<'static>> = RefCell::new(AssetRouter::default());
}

/// The embedded pages, certified: `/` and `/index.html` are the start page,
/// and any other path is the 404 page with status 404.
fn router() -> AssetRouter<'static> {
    let assets = [
        Asset::new("index.html", INDEX_HTML),
        Asset::new("404.html", NOT_FOUND_HTML),
    ];
    let configs = [
        AssetConfig::File {
            path: "index.html".to_string(),
            content_type: Some(HTML.to_string()),
            headers: security_headers(),
            fallback_for: vec![],
            aliased_by: vec!["/".to_string()],
            encodings: vec![],
        },
        AssetConfig::File {
            path: "404.html".to_string(),
            content_type: Some(HTML.to_string()),
            headers: security_headers(),
            fallback_for: vec![AssetFallbackConfig {
                scope: "/".to_string(),
                status_code: Some(StatusCode::NOT_FOUND),
            }],
            aliased_by: vec![],
            encodings: vec![],
        },
    ];
    let mut router = AssetRouter::default();
    router
        .certify_assets(assets, configs)
        .expect("the embedded pages certify");
    router
}

/// Certifies the embedded pages and sets the canister's certified data to
/// their root hash. `init` and `post_upgrade` call it, because the heap and
/// the certified data both start empty after an install or an upgrade.
pub fn certify() {
    let router = router();
    ic_cdk::api::certified_data_set(router.root_hash());
    ROUTER.with_borrow_mut(|current| *current = router);
}

/// The certified response for `request`, given the data certificate of this
/// query. A URL that cannot be read gets a 400 without a certificate, which
/// the gateway refuses to show; there is no page it could be certified for.
pub fn serve(data_certificate: &[u8], request: &HttpRequest) -> HttpResponse {
    ROUTER.with_borrow(
        |router| match router.serve_asset(data_certificate, request) {
            Ok(response) => HttpResponse {
                status_code: response.status_code().as_u16(),
                headers: response.headers().to_vec(),
                body: ByteBuf::from(response.body()),
            },
            Err(error) => HttpResponse {
                status_code: StatusCode::BAD_REQUEST.as_u16(),
                headers: security_headers(),
                body: ByteBuf::from(format!("cannot serve {}: {error}", request.url())),
            },
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(url: &str) -> HttpResponse {
        ROUTER.with_borrow_mut(|current| *current = router());
        serve(&[], &HttpRequest::get(url).build())
    }

    fn header<'a>(response: &'a HttpResponse, name: &str) -> Option<&'a str> {
        response
            .headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn the_start_page_answers_at_the_root_and_at_its_path() {
        for url in ["/", "/index.html", "/?from=link", "https://wallet.icp0.io/"] {
            let response = get(url);
            assert_eq!(response.status_code, 200, "{url}");
            assert_eq!(response.body.as_slice(), INDEX_HTML, "{url}");
            assert_eq!(header(&response, "content-type"), Some(HTML), "{url}");
        }
    }

    #[test]
    fn any_other_path_is_the_404_page() {
        for url in ["/claim", "/index.htm", "/a/b/c.js", "/404"] {
            let response = get(url);
            assert_eq!(response.status_code, 404, "{url}");
            assert_eq!(response.body.as_slice(), NOT_FOUND_HTML, "{url}");
        }
    }

    #[test]
    fn every_response_carries_a_certificate_and_the_security_headers() {
        for url in ["/", "/nothing-here"] {
            let response = get(url);
            assert!(header(&response, "ic-certificate").is_some(), "{url}");
            for (name, value) in security_headers() {
                assert_eq!(
                    header(&response, &name),
                    Some(value.as_str()),
                    "{url}: {name}"
                );
            }
        }
    }

    #[test]
    fn a_url_that_cannot_be_read_is_a_400_without_a_certificate() {
        let response = get("/%FF");
        assert_eq!(response.status_code, 400);
        assert!(header(&response, "ic-certificate").is_none());
        assert_eq!(
            header(&response, "content-security-policy"),
            Some(CONTENT_SECURITY_POLICY)
        );
    }

    /// The CSP would block them, so a page that has them is broken.
    #[test]
    fn the_pages_hold_no_inline_script_or_style() {
        for page in [INDEX_HTML, NOT_FOUND_HTML] {
            let page = std::str::from_utf8(page).unwrap().to_ascii_lowercase();
            for banned in ["<script", "<style", " style=", "javascript:"] {
                assert!(!page.contains(banned), "a page contains {banned:?}");
            }
            // Event handler attributes: ` on` and letters, then `=`.
            let handler = page.match_indices(" on").any(|(at, _)| {
                let rest = &page[at + 3..];
                let name = rest.bytes().take_while(u8::is_ascii_lowercase).count();
                name > 0 && rest[name..].starts_with('=')
            });
            assert!(!handler, "a page has an event handler attribute");
        }
    }
}
