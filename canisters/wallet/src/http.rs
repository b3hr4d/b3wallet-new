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
/// Trusted Types are required and no policy may make them, so a browser that
/// enforces them refuses every string written as markup or script, and every
/// `javascript:` URL.
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
    require-trusted-types-for 'script'; \
    trusted-types 'none'; \
    upgrade-insecure-requests";

// Built by `ui/` (`npm run build`), which prerenders the pages and bundles
// the app they load; CI checks the files are that build's output.
const INDEX_HTML: &[u8] = include_bytes!("../pages/index.html");
const SEND_HTML: &[u8] = include_bytes!("../pages/send.html");
const APPROVALS_HTML: &[u8] = include_bytes!("../pages/approvals.html");
const RECEIVE_HTML: &[u8] = include_bytes!("../pages/receive.html");
const ACCOUNT_HTML: &[u8] = include_bytes!("../pages/account.html");
const MEMBERS_HTML: &[u8] = include_bytes!("../pages/members.html");
const SETTINGS_HTML: &[u8] = include_bytes!("../pages/settings.html");
const NOTIFICATIONS_HTML: &[u8] = include_bytes!("../pages/notifications.html");
const CLAIM_HTML: &[u8] = include_bytes!("../pages/claim.html");
const NOT_FOUND_HTML: &[u8] = include_bytes!("../pages/404.html");
const APP_JS: &[u8] = include_bytes!("../pages/app.js");
const APP_CSS: &[u8] = include_bytes!("../pages/app.css");
const HTML: &str = "text/html; charset=utf-8";
const JAVASCRIPT: &str = "text/javascript; charset=utf-8";
const CSS: &str = "text/css; charset=utf-8";

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

/// The embedded files, certified: `/` and `/index.html` are the start page,
/// `/send`, `/receive`, `/account`, `/approvals`, `/notifications`,
/// `/settings`, `/members` and `/claim` the other pages, each also at its
/// `.html` path, `/app.js` and `/app.css` the app they load, and any other
/// path is the 404 page with status 404.
fn router() -> AssetRouter<'static> {
    let files: [(&str, &[u8], &str, &[&str]); 12] = [
        ("index.html", INDEX_HTML, HTML, &["/"]),
        ("send.html", SEND_HTML, HTML, &["/send"]),
        ("receive.html", RECEIVE_HTML, HTML, &["/receive"]),
        ("account.html", ACCOUNT_HTML, HTML, &["/account"]),
        ("approvals.html", APPROVALS_HTML, HTML, &["/approvals"]),
        (
            "notifications.html",
            NOTIFICATIONS_HTML,
            HTML,
            &["/notifications"],
        ),
        ("settings.html", SETTINGS_HTML, HTML, &["/settings"]),
        ("members.html", MEMBERS_HTML, HTML, &["/members"]),
        ("claim.html", CLAIM_HTML, HTML, &["/claim"]),
        ("404.html", NOT_FOUND_HTML, HTML, &[]),
        ("app.js", APP_JS, JAVASCRIPT, &[]),
        ("app.css", APP_CSS, CSS, &[]),
    ];
    let assets = files.map(|(path, body, _, _)| Asset::new(path, body));
    let configs = files.map(|(path, _, content_type, aliases)| AssetConfig::File {
        path: path.to_string(),
        content_type: Some(content_type.to_string()),
        headers: security_headers(),
        fallback_for: match path {
            "404.html" => vec![AssetFallbackConfig {
                scope: "/".to_string(),
                status_code: Some(StatusCode::NOT_FOUND),
            }],
            _ => vec![],
        },
        aliased_by: aliases.iter().map(|alias| alias.to_string()).collect(),
        encodings: vec![],
    });
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
    fn the_other_pages_and_the_app_answer_at_their_paths() {
        for (url, body, content_type) in [
            ("/send", SEND_HTML, HTML),
            ("/send.html", SEND_HTML, HTML),
            ("/receive", RECEIVE_HTML, HTML),
            ("/receive.html", RECEIVE_HTML, HTML),
            ("/receive?account=2", RECEIVE_HTML, HTML),
            ("/account", ACCOUNT_HTML, HTML),
            ("/account.html", ACCOUNT_HTML, HTML),
            ("/account?id=1", ACCOUNT_HTML, HTML),
            ("/approvals", APPROVALS_HTML, HTML),
            ("/approvals.html", APPROVALS_HTML, HTML),
            ("/notifications", NOTIFICATIONS_HTML, HTML),
            ("/notifications.html", NOTIFICATIONS_HTML, HTML),
            ("/settings", SETTINGS_HTML, HTML),
            ("/settings.html", SETTINGS_HTML, HTML),
            ("/members", MEMBERS_HTML, HTML),
            ("/members.html", MEMBERS_HTML, HTML),
            ("/claim", CLAIM_HTML, HTML),
            ("/claim.html", CLAIM_HTML, HTML),
            ("/claim?x=1", CLAIM_HTML, HTML),
            ("/app.js", APP_JS, JAVASCRIPT),
            ("/app.css", APP_CSS, CSS),
        ] {
            let response = get(url);
            assert_eq!(response.status_code, 200, "{url}");
            assert_eq!(response.body.as_slice(), body, "{url}");
            assert_eq!(
                header(&response, "content-type"),
                Some(content_type),
                "{url}"
            );
        }
    }

    #[test]
    fn any_other_path_is_the_404_page() {
        for url in [
            "/claims",
            "/claim/",
            "/send/1",
            "/receive/1",
            "/account/1",
            "/accounts",
            "/member",
            "/settings/members",
            "/notification",
            "/approval",
            "/index.htm",
            "/a/b/c.js",
            "/404",
            "/assets/app.js",
        ] {
            let response = get(url);
            assert_eq!(response.status_code, 404, "{url}");
            assert_eq!(response.body.as_slice(), NOT_FOUND_HTML, "{url}");
        }
    }

    #[test]
    fn every_response_carries_a_certificate_and_the_security_headers() {
        for url in [
            "/",
            "/send",
            "/receive",
            "/account",
            "/approvals",
            "/notifications",
            "/settings",
            "/members",
            "/claim",
            "/app.js",
            "/app.css",
            "/nothing-here",
        ] {
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

    /// The CSP would block them, so a page that has them is broken. Each page
    /// loads the app's script and styles from the wallet's own origin, and
    /// nothing else.
    #[test]
    fn the_pages_hold_no_inline_script_or_style() {
        const SCRIPT: &str = r#"<script type="module" src="/app.js"></script>"#;
        const STYLES: &str = r#"<link rel="stylesheet" href="/app.css">"#;
        for page in [
            INDEX_HTML,
            SEND_HTML,
            RECEIVE_HTML,
            ACCOUNT_HTML,
            APPROVALS_HTML,
            NOTIFICATIONS_HTML,
            SETTINGS_HTML,
            MEMBERS_HTML,
            CLAIM_HTML,
            NOT_FOUND_HTML,
        ] {
            let page = std::str::from_utf8(page).unwrap().to_ascii_lowercase();
            assert_eq!(page.matches(SCRIPT).count(), 1, "a page loads the app once");
            assert_eq!(
                page.matches(STYLES).count(),
                1,
                "a page loads its styles once"
            );
            let page = page.replace(SCRIPT, "");
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
