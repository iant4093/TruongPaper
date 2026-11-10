//! Photo API v1 HTTP client behind an injectable transport.
//!
//! [`PhotoTransport`] is the only place that touches the network. Production code uses
//! `reqwest::Client`; tests use [`FnTransport`] so no real request is ever made.

use crate::error::{PhotoApiError, PhotoApiResult};
use crate::{ApiPhotoResponse, API_RANDOM_URL, PUBLIC_CATALOG_URL};
use std::future::Future;
use truongpaper_core::{parse_album_slug, CatalogPage, PublicCatalogAlbum};
use url::Url;

/// Safety cap on catalog pagination (100 albums per page).
pub const MAX_CATALOG_PAGES: usize = 50;

/// Status code and body of a completed HTTP exchange.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// JSON response helper.
    pub fn json(status: u16, body: &str) -> Self {
        Self {
            status,
            body: body.as_bytes().to_vec(),
        }
    }
}

/// Minimal HTTP GET abstraction. Implementations must return non-2xx responses as `Ok` and
/// reserve `Err` for transport failures.
pub trait PhotoTransport: Send + Sync {
    fn fetch(
        &self,
        url: &str,
        query: &[(&str, &str)],
    ) -> impl Future<Output = PhotoApiResult<HttpResponse>> + Send;
}

impl PhotoTransport for reqwest::Client {
    async fn fetch(&self, url: &str, query: &[(&str, &str)]) -> PhotoApiResult<HttpResponse> {
        let mut req = self.get(url);
        if !query.is_empty() {
            req = req.query(query);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| PhotoApiError::Transport(e.to_string()))?;
        let status = resp.status().as_u16();
        let body = resp
            .bytes()
            .await
            .map_err(|e| PhotoApiError::Transport(e.to_string()))?
            .to_vec();
        Ok(HttpResponse { status, body })
    }
}

impl<T: PhotoTransport + ?Sized> PhotoTransport for &T {
    fn fetch(
        &self,
        url: &str,
        query: &[(&str, &str)],
    ) -> impl Future<Output = PhotoApiResult<HttpResponse>> + Send {
        (**self).fetch(url, query)
    }
}

/// Transport backed by a closure. Intended for tests and offline tooling.
pub struct FnTransport<F>(pub F);

impl<F> PhotoTransport for FnTransport<F>
where
    F: Fn(&str, &[(&str, &str)]) -> PhotoApiResult<HttpResponse> + Send + Sync,
{
    async fn fetch(&self, url: &str, query: &[(&str, &str)]) -> PhotoApiResult<HttpResponse> {
        (self.0)(url, query)
    }
}

/// Query for `GET /v1/photos/random`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RandomPhotoQuery {
    /// Album slug (or album URL, normalized here). `None` = whole portfolio.
    pub album: Option<String>,
    /// `featured=true` when set.
    pub featured: bool,
}

impl RandomPhotoQuery {
    /// Builds the query string pairs: `album=<slug>` then `featured=true`.
    pub fn to_pairs(&self) -> PhotoApiResult<Vec<(&'static str, String)>> {
        let mut pairs = Vec::new();
        if let Some(album) = &self.album {
            let slug = parse_album_slug(album).ok_or_else(|| {
                PhotoApiError::InvalidFilter(format!("invalid album slug: {album}"))
            })?;
            pairs.push(("album", slug));
        }
        if self.featured {
            pairs.push(("featured", "true".to_string()));
        }
        Ok(pairs)
    }
}

/// Maps an HTTP status to the documented API error classes.
pub fn check_status(status: u16) -> PhotoApiResult<()> {
    match status {
        200..=299 => Ok(()),
        403 | 429 | 503 => Err(PhotoApiError::RateLimited(status)),
        400 => Err(PhotoApiError::InvalidParameters),
        404 => Err(PhotoApiError::NoPhotos),
        other => Err(PhotoApiError::UnexpectedStatus(other)),
    }
}

/// Client for the Ian Truong Photography public API.
#[derive(Debug, Clone)]
pub struct PhotoApiClient<T> {
    transport: T,
    random_url: String,
    catalog_url: String,
}

impl<T: PhotoTransport> PhotoApiClient<T> {
    /// Client pointed at the production endpoints.
    pub fn new(transport: T) -> Self {
        Self::with_endpoints(transport, API_RANDOM_URL, PUBLIC_CATALOG_URL)
    }

    /// Client with custom endpoints (staging, tests).
    pub fn with_endpoints(
        transport: T,
        random_url: impl Into<String>,
        catalog_url: impl Into<String>,
    ) -> Self {
        Self {
            transport,
            random_url: random_url.into(),
            catalog_url: catalog_url.into(),
        }
    }

    /// `GET /v1/photos/random` with optional `album` / `featured` parameters.
    pub async fn fetch_random(&self, query: &RandomPhotoQuery) -> PhotoApiResult<ApiPhotoResponse> {
        let pairs = query.to_pairs()?;
        let borrowed: Vec<(&str, &str)> = pairs.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let resp = self.transport.fetch(&self.random_url, &borrowed).await?;
        check_status(resp.status)?;
        Ok(serde_json::from_slice(&resp.body)?)
    }

    /// Downloads `imageUrl` bytes. Counts against the same rate limit as `fetch_random`.
    pub async fn download_image(&self, image_url: &str) -> PhotoApiResult<Vec<u8>> {
        let resp = self.transport.fetch(image_url, &[]).await?;
        match resp.status {
            200..=299 => Ok(resp.body),
            403 | 429 | 503 => Err(PhotoApiError::RateLimited(resp.status)),
            other => Err(PhotoApiError::UnexpectedStatus(other)),
        }
    }

    /// One page of `GET /api/public/albums`.
    pub async fn fetch_catalog_page(&self, cursor: Option<&str>) -> PhotoApiResult<CatalogPage> {
        let mut url = Url::parse(&self.catalog_url)
            .map_err(|e| PhotoApiError::Transport(format!("bad catalog url: {e}")))?;
        if let Some(c) = cursor {
            url.query_pairs_mut().append_pair("cursor", c);
        }
        let resp = self.transport.fetch(url.as_str(), &[]).await?;
        check_status(resp.status)?;
        Ok(serde_json::from_slice(&resp.body)?)
    }

    /// Follows `nextCursor` until exhausted (at most [`MAX_CATALOG_PAGES`] pages).
    pub async fn fetch_full_catalog(&self) -> PhotoApiResult<Vec<PublicCatalogAlbum>> {
        let mut all = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_CATALOG_PAGES {
            let page = self.fetch_catalog_page(cursor.as_deref()).await?;
            all.extend(page.items);
            match page.next_cursor {
                Some(next) if !next.is_empty() => cursor = Some(next),
                _ => return Ok(all),
            }
        }
        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::sync::Arc;

    const SAMPLE: &str = r#"{"albumSlug":"bald-mountain-11-1",
        "albumUrl":"https://iantruongphotography.com/album/bald-mountain-11-1",
        "bytes":17616840,"fallbackUsed":false,"featured":true,"height":4505,
        "id":"e95f46a6b4d6b9b772df7c79",
        "imageUrl":"https://photos-api.iantruongphotography.com/v1/images/c7/YWxi",
        "width":6757}"#;

    type Calls = Arc<Mutex<Vec<(String, Vec<(String, String)>)>>>;

    fn recording<F>(calls: Calls, f: F) -> impl PhotoTransport
    where
        F: Fn(&str) -> HttpResponse + Send + Sync + 'static,
    {
        FnTransport(move |url: &str, q: &[(&str, &str)]| {
            calls.lock().push((
                url.to_string(),
                q.iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            ));
            Ok(f(url))
        })
    }

    #[tokio::test]
    async fn random_sends_expected_query_matrix() {
        let cases: Vec<(RandomPhotoQuery, Vec<(&str, &str)>)> = vec![
            (RandomPhotoQuery::default(), vec![]),
            (
                RandomPhotoQuery {
                    album: None,
                    featured: true,
                },
                vec![("featured", "true")],
            ),
            (
                RandomPhotoQuery {
                    album: Some("bald-mountain-11-1".into()),
                    featured: false,
                },
                vec![("album", "bald-mountain-11-1")],
            ),
            (
                RandomPhotoQuery {
                    album: Some(
                        "https://iantruongphotography.com/album/Bald-Mountain-11-1/".into(),
                    ),
                    featured: true,
                },
                vec![("album", "bald-mountain-11-1"), ("featured", "true")],
            ),
        ];
        for (query, expected) in cases {
            let calls: Calls = Default::default();
            let client = PhotoApiClient::new(recording(calls.clone(), |_| {
                HttpResponse::json(200, SAMPLE)
            }));
            let photo = client.fetch_random(&query).await.unwrap();
            assert_eq!(photo.id, "e95f46a6b4d6b9b772df7c79");
            assert_eq!(photo.width, Some(6757));
            assert!(photo.featured && !photo.fallback_used);
            let calls = calls.lock();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].0, API_RANDOM_URL);
            let got: Vec<(&str, &str)> = calls[0]
                .1
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            assert_eq!(got, expected);
        }
    }

    #[tokio::test]
    async fn random_rejects_invalid_album_before_any_request() {
        let calls: Calls = Default::default();
        let client = PhotoApiClient::new(recording(calls.clone(), |_| {
            HttpResponse::json(200, SAMPLE)
        }));
        let q = RandomPhotoQuery {
            album: Some("bad slug!".into()),
            featured: false,
        };
        assert!(matches!(
            client.fetch_random(&q).await,
            Err(PhotoApiError::InvalidFilter(_))
        ));
        assert!(calls.lock().is_empty());
    }

    #[tokio::test]
    async fn random_maps_error_statuses() {
        for (status, body, check) in [
            (400, r#"{"error":"invalid_parameters"}"#, 0),
            (404, r#"{"error":"no_photos"}"#, 1),
            (403, "", 2),
            (429, "", 2),
            (503, "", 2),
            (500, "", 3),
        ] {
            let client = PhotoApiClient::new(FnTransport(move |_: &str, _: &[(&str, &str)]| {
                Ok(HttpResponse::json(status, body))
            }));
            let err = client
                .fetch_random(&RandomPhotoQuery::default())
                .await
                .unwrap_err();
            match check {
                0 => assert!(matches!(err, PhotoApiError::InvalidParameters)),
                1 => assert!(matches!(err, PhotoApiError::NoPhotos)),
                2 => assert!(matches!(err, PhotoApiError::RateLimited(s) if s == status)),
                _ => assert!(matches!(err, PhotoApiError::UnexpectedStatus(500))),
            }
        }
    }

    #[tokio::test]
    async fn random_reports_bad_json_and_transport_errors() {
        let client = PhotoApiClient::new(FnTransport(|_: &str, _: &[(&str, &str)]| {
            Ok(HttpResponse::json(200, "{nope"))
        }));
        assert!(matches!(
            client.fetch_random(&RandomPhotoQuery::default()).await,
            Err(PhotoApiError::Json(_))
        ));
        let client = PhotoApiClient::new(FnTransport(|_: &str, _: &[(&str, &str)]| {
            Err(PhotoApiError::Transport("offline".into()))
        }));
        assert!(matches!(
            client.fetch_random(&RandomPhotoQuery::default()).await,
            Err(PhotoApiError::Transport(_))
        ));
    }

    #[tokio::test]
    async fn download_image_returns_bytes_and_maps_throttle() {
        let client = PhotoApiClient::new(FnTransport(|url: &str, _: &[(&str, &str)]| {
            Ok(match url {
                "https://img/ok" => HttpResponse {
                    status: 200,
                    body: vec![1, 2, 3],
                },
                "https://img/throttled" => HttpResponse {
                    status: 503,
                    body: vec![],
                },
                _ => HttpResponse {
                    status: 404,
                    body: vec![],
                },
            })
        }));
        assert_eq!(
            client.download_image("https://img/ok").await.unwrap(),
            vec![1, 2, 3]
        );
        assert!(matches!(
            client.download_image("https://img/throttled").await,
            Err(PhotoApiError::RateLimited(503))
        ));
        assert!(matches!(
            client.download_image("https://img/missing").await,
            Err(PhotoApiError::UnexpectedStatus(404))
        ));
    }

    #[tokio::test]
    async fn catalog_follows_cursor_pages() {
        let page1 = r#"{"items":[{"albumId":"1","type":"photo","title":"A","imageCount":8,
            "coverImageUrl":"https://c/1.jpg","coverThumbnailUrl":"https://c/1t.jpg","slug":"a-1"}],
            "nextCursor":"abc 123"}"#;
        let page2 = r#"{"items":[{"albumId":"2","type":"photo","title":"B","imageCount":1,
            "coverImageUrl":"https://c/2.jpg","coverThumbnailUrl":"https://c/2t.jpg","slug":"b-2"}],
            "nextCursor":null}"#;
        let calls: Calls = Default::default();
        let client = PhotoApiClient::new(recording(calls.clone(), move |url| {
            if url.contains("cursor=") {
                HttpResponse::json(200, page2)
            } else {
                HttpResponse::json(200, page1)
            }
        }));
        let all = client.fetch_full_catalog().await.unwrap();
        assert_eq!(
            all.iter().map(|a| a.slug.as_str()).collect::<Vec<_>>(),
            ["a-1", "b-2"]
        );
        assert_eq!(all[0].image_count, 8);
        let calls = calls.lock();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, PUBLIC_CATALOG_URL);
        assert_eq!(
            calls[1].0,
            "https://iantruongphotography.com/api/public/albums?type=photo&limit=100&cursor=abc+123"
        );
    }

    #[tokio::test]
    async fn catalog_error_status_is_reported() {
        let client = PhotoApiClient::new(FnTransport(|_: &str, _: &[(&str, &str)]| {
            Ok(HttpResponse::json(503, ""))
        }));
        assert!(matches!(
            client.fetch_full_catalog().await,
            Err(PhotoApiError::RateLimited(503))
        ));
    }

    #[test]
    fn borrowed_transport_is_a_transport() {
        fn takes<T: PhotoTransport>(_: &T) {}
        let t = FnTransport(|_: &str, _: &[(&str, &str)]| Ok(HttpResponse::json(200, "{}")));
        takes(&&t);
        takes(&reqwest::Client::new());
    }
}
