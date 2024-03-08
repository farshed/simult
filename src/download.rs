use crate::error::DownloadError;
use futures::{stream::FuturesUnordered, StreamExt};
use std::{
    fmt::format,
    io::SeekFrom,
    path::{Path, PathBuf},
};
use tokio::{
    fs,
    io::{AsyncSeekExt, AsyncWriteExt},
};

pub struct Downloader {
    client: reqwest::Client,
    output_dir: PathBuf,
    conn_count: usize,
}

impl Downloader {
    /// Creates a new Downloader
    pub fn new(output_dir: &str, conn_count: usize) -> Self {
        Self {
            client: reqwest::Client::new(),
            output_dir: PathBuf::from(output_dir),
            conn_count,
        }
    }

    /// Downloads the file at the given `url` with the best possible strategy.
    pub async fn download(&self, url: &str) -> Result<(), DownloadError> {
        let response = self.client.head(url).send().await?;
        let headers = response.headers();
        let content_length: u64 = headers
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        let accept_ranges = headers
            .get(reqwest::header::ACCEPT_RANGES)
            .and_then(|v| v.to_str().ok())
            // .and_then(|v| Some(v.trim() != "none"))
            .is_some();

        println!(
            "accept_ranges: {}, content_length: {}",
            accept_ranges, content_length
        );

        if content_length > 0 && accept_ranges {
            self.parallel(url, content_length).await?;
        } else {
            self.sequential(url).await?;
        }

        Ok(())
    }

    /// Assumes that the host supports [Range requests](https://developer.mozilla.org/en-US/docs/Web/HTTP/Range_requests) request and tries to download the file at the given `url` in parallel.
    pub async fn parallel(&self, url: &str, content_length: u64) -> Result<(), DownloadError> {
        let output_path = self.get_output_path(url);
        let mut file = fs::File::create(&output_path).await?;
        let chunk_size = content_length / self.conn_count as u64;

        let mut futures: FuturesUnordered<_> = (0..self.conn_count)
            .map(|i| {
                let start = i as u64 * chunk_size;
                let end = if i == self.conn_count - 1 {
                    content_length - 1
                } else {
                    start + chunk_size - 1
                };

                let client = self.client.clone();
                let range = format!("bytes={}-{}", start, end);
                let url = url.to_string();

                tokio::spawn(async move {
                    let mut stream = client
                        .get(&url)
                        .header(reqwest::header::RANGE, range)
                        .send()
                        .await?
                        .bytes_stream();

                    file.seek(SeekFrom::Start(start)).await?;

                    while let Some(chunk) = stream.next().await {
                        file.write_all(&chunk?).await?;
                    }

                    Ok::<(), DownloadError>(())
                })
            })
            .collect();

        while let Some(result) = futures.next().await {
            result?;
        }

        Ok(())
    }

    /// Downloads the file at the given `url` serially.
    pub async fn sequential(&self, url: &str) -> Result<(), DownloadError> {
        let response = self.client.get(url).send().await?.bytes().await?;
        let output_path = self.get_output_path(url);
        let mut file = fs::File::create(&output_path).await?;
        file.write_all(&response).await?;
        Ok(())
    }

    fn get_output_path(&self, url: &str) -> String {
        let filename = url::Url::parse(url)
            .ok()
            .and_then(|u| {
                u.path_segments()
                    .map(|segments| segments.last().unwrap_or_else(|| "unnamed").to_owned())
            })
            .unwrap_or_else(|| "unnamed".to_owned());

        let mut output_path = self.output_dir.join(&filename);
        let mut i = 1;

        while output_path.exists() {
            output_path = self.output_dir.join(format!("{} ({})", &filename, i));
            i += 1;
        }

        output_path.to_string_lossy().to_string()
    }
}
