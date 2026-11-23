/*
 * SPDX-FileCopyrightText: 2026 Coffey Labs LLC
 *
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! `GET /logo` (branding spec BT-5 to BT-8).

use common::Server;
use http_proto::{HttpRequest, HttpResponse};
use hyper::{StatusCode, header};
use inbuxa_features::branding::logo::Logo;

const LOGO_CACHE: &str = "public, max-age=300";
const SVG_CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; sandbox";

/// The domain a `/logo` request asks about: `?domain=`, else the `Host`
/// (BT-2).
fn requested_domain(req: &HttpRequest) -> String {
    req.uri()
        .query()
        .and_then(|query| {
            http_proto::form_urlencoded::parse(query.as_bytes())
                .find(|(key, _)| key == "domain")
                .map(|(_, value)| value.into_owned())
        })
        .filter(|domain| !domain.is_empty())
        .or_else(|| {
            req.headers()
                .get(header::HOST)
                .and_then(|host| host.to_str().ok())
                .map(str::to_string)
        })
        .unwrap_or_default()
}

/// Whether `/logo` was asked for the dark-mode logo, with `?theme=dark`
/// (BT-27). Anything else is the light one.
fn wants_dark(req: &HttpRequest) -> bool {
    req.uri().query().is_some_and(|query| {
        http_proto::form_urlencoded::parse(query.as_bytes())
            .any(|(key, value)| key == "theme" && value.eq_ignore_ascii_case("dark"))
    })
}

/// Whether the browser is loading `/logo` as an image, as an `<img>` does
/// (BT-26). A link opened, a frame, a script or a client that doesn't say
/// isn't.
fn loads_as_image(req: &HttpRequest) -> bool {
    req.headers()
        .get("sec-fetch-dest")
        .is_some_and(|dest| dest.as_bytes().eq_ignore_ascii_case(b"image"))
}

/// An `https:` URL; an `http:` one stored before the fork (BT-4) is never
/// redirected to.
fn is_https(url: &str) -> bool {
    url.get(..6)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("https:"))
}

/// `GET /logo`: the image, a redirect to a URL logo, or `404` when no
/// custom logo applies. The server never fetches a URL (BT-7). With
/// `?theme=dark`, a dark-mode logo where one is set (BT-27).
pub async fn logo(server: &Server, req: &HttpRequest) -> trc::Result<HttpResponse> {
    let response = match server
        .logos_for(&requested_domain(req), wants_dark(req))
        .await?
        .into_iter()
        .find(|logo| !matches!(logo, Logo::Url(url) if !is_https(url)))
    {
        Some(Logo::Image {
            content_type,
            bytes,
        }) => {
            let response = HttpResponse::new(StatusCode::OK)
                .with_content_type(content_type)
                .with_binary_body(bytes);
            if content_type == "image/svg+xml" {
                // BT-8: a scripted SVG must never run on the server's origin
                response.with_header(header::CONTENT_SECURITY_POLICY, SVG_CSP)
            } else {
                response
            }
            .with_cache_control(LOGO_CACHE)
        }
        Some(Logo::Url(url)) => {
            // A domain's administrator chooses the URL: only an image load
            // follows it, so /logo can't send a person to any page it likes.
            // The answer depends on who asks, so no cache keeps it
            if loads_as_image(req) {
                HttpResponse::new(StatusCode::FOUND).with_header(header::LOCATION, url)
            } else {
                HttpResponse::new(StatusCode::NOT_FOUND)
            }
            .with_no_store()
            .with_header(header::VARY, "Sec-Fetch-Dest")
        }
        None => HttpResponse::new(StatusCode::NOT_FOUND).with_cache_control(LOGO_CACHE),
    };
    Ok(response
        .with_header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .with_header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"))
}
