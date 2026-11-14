//! Word-synced lyrics from the [AMLL TTML DB](https://api.amll.dev).
//!
//! The database files community TTML by GitHub account. A search finds the
//! track (and its author), and a second request returns the document itself.

use std::path::Path;

use anyhow::Result;
use serde_json::Value;

use super::remote::{
    duration_matches, get_json, same_track, string_at, strings_at, ttml_in, with_cache,
};
use super::{Attribution, Credit, Lyrics, Query, cache_key, ttml};

const BASE: &str = "https://api.amll.dev";
const SOURCE: &str = "amll_ttml_db";

/// The DB's lyrics for `query`, or `None` when it has none or cannot be read.
pub async fn fetch_amll(
    http: &reqwest::Client,
    cache_dir: &Path,
    query: &Query,
) -> Result<Option<Lyrics>> {
    let key = format!("amll-{}", cache_key(query));
    with_cache(cache_dir, &key, request_from(http, BASE, query)).await
}

async fn request_from(http: &reqwest::Client, base: &str, query: &Query) -> Result<Option<Lyrics>> {
    let Some(root) = get_json(
        http,
        &format!("{base}/v1/lyrics/search"),
        &[
            ("trackName", query.title.as_str()),
            ("artistName", query.artist.as_str()),
        ],
        &[],
    )
    .await?
    else {
        return Ok(None);
    };
    let items = root
        .get("data")
        .and_then(|data| data.get("items"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let Some(candidate) = items.iter().find(|item| matches(item, query)) else {
        return Ok(None);
    };
    let Some(id) = string_at(candidate, "id") else {
        return Ok(None);
    };
    let maker = author(candidate);

    let Some(root) = get_json(
        http,
        &format!("{base}/v1/lyrics/get"),
        &[("id", id.as_str())],
        &[],
    )
    .await?
    else {
        return Ok(None);
    };
    let data = root.get("data").unwrap_or(&root);
    let Some(document) = ttml_in(data) else {
        return Ok(None);
    };
    let Some(mut lyrics) = ttml::parse(&document) else {
        return Ok(None);
    };
    lyrics.attribution = Some(Attribution {
        source: SOURCE.to_string(),
        uploader: None,
        maker,
    });
    Ok(Some(lyrics))
}

fn matches(candidate: &Value, query: &Query) -> bool {
    let titles = strings_at(candidate, "musicNames");
    let artists = strings_at(candidate, "artistNames");
    titles.iter().any(|title| {
        artists
            .iter()
            .any(|artist| same_track(title, artist, query))
    }) && duration_matches(0, query)
}

/// AMLL files the document by GitHub account, which gives a profile.
fn author(candidate: &Value) -> Option<Credit> {
    let login = strings_at(candidate, "authorUsernames")
        .into_iter()
        .next()?;
    let valid = !login.is_empty()
        && login.len() <= 39
        && login.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    valid.then(|| Credit {
        url: Some(format!("https://github.com/{login}")),
        name: login,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn server(answers: Vec<String>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut paths = Vec::new();
            for answer in answers {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = [0u8; 8192];
                let len = socket.read(&mut bytes).await.unwrap();
                paths.push(
                    String::from_utf8_lossy(&bytes[..len])
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or_default()
                        .to_string(),
                );
                socket.write_all(answer.as_bytes()).await.unwrap();
            }
            paths
        });
        (format!("http://{address}"), server)
    }

    fn json(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn query() -> Query {
        Query {
            artist: "Artist".into(),
            title: "Song".into(),
            album: "Album".into(),
            duration_ms: 120_000,
        }
    }

    #[tokio::test]
    async fn the_search_matches_the_track_and_the_document_becomes_lyrics() {
        let search = r#"{"data":{"items":[{"id":"one","musicNames":["Other"],"artistNames":["Artist"]},{"id":"two","musicNames":["Song"],"artistNames":["Artist"],"authorUsernames":["octocat"],"authorIds":["1"]}]}}"#;
        let document = r#"{"data":{"lyrics":"<tt xmlns=\"http://www.w3.org/ns/ttml\" itunes:timing=\"word\"><body><div><p begin=\"1s\" end=\"2s\"><span begin=\"1s\" end=\"2s\">Hello</span></p></div></body></tt>"}}"#;
        let (api, server) = server(vec![json(search), json(document)]).await;
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let lyrics = request_from(&http, &api, &query()).await.unwrap().unwrap();
        assert_eq!(lyrics.lines[0].text, "Hello");
        let attribution = lyrics.attribution.unwrap();
        assert_eq!(attribution.source, "amll_ttml_db");
        assert_eq!(
            attribution.maker.unwrap().url.unwrap(),
            "https://github.com/octocat"
        );
        let paths = server.await.unwrap();
        assert!(paths[0].starts_with("/v1/lyrics/search?"));
        assert!(paths[1].starts_with("/v1/lyrics/get?"));
    }

    #[test]
    fn matching_requires_the_title_and_artist() {
        let candidate = serde_json::json!({
            "musicNames": ["Song"],
            "artistNames": ["Other"],
        });
        assert!(!matches(&candidate, &query()));
        let candidate = serde_json::json!({
            "musicNames": ["Song (Live)"],
            "artistNames": ["Artist"],
        });
        assert!(matches(&candidate, &query()));
    }
}
