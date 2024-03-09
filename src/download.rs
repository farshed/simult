use crate::error::DownloadError;
use futures::{stream::FuturesUnordered, StreamExt};
use std::{
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
        let conn_count = if conn_count > 0 { conn_count } else { 1 };
        Self {
            client: reqwest::Client::new(),
            output_dir: PathBuf::from(output_dir),
            conn_count,
        }
    }

    /// Downloads the file at the given `url` with the best possible strategy.
    pub async fn download(&self, url: &str) -> Result<PathBuf, DownloadError> {
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

        let output_path = if content_length > 0 && accept_ranges {
            self.parallel(url, content_length).await?
        } else {
            self.sequential(url).await?
        };

        Ok(output_path)
    }

    pub async fn download_multiple(
        &'static self,
        urls: &[String],
    ) -> Result<Vec<PathBuf>, DownloadError> {
        let chunked = urls.chunks(self.conn_count);

        for batch in chunked {
            let mut futures: FuturesUnordered<_> = batch
                .iter()
                .map(|url| {
                    let url = url.to_string();
                    tokio::spawn(async move { self.sequential(&url).await })
                })
                .collect();
        }

        Ok(Vec::new())
    }

    /// Assumes that the host supports [Range requests](https://developer.mozilla.org/en-US/docs/Web/HTTP/Range_requests) and tries to download the file at the given `url` in parallel.
    pub async fn parallel(&self, url: &str, content_length: u64) -> Result<PathBuf, DownloadError> {
        let chunk_size = content_length / self.conn_count as u64;
        let output_path = self.get_output_path(url);

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
                let output_path = output_path.clone();

                tokio::spawn(async move {
                    let mut file = fs::OpenOptions::new()
                        .write(true)
                        .create(true)
                        .open(&output_path)
                        .await?;

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
            let _ = result?;
        }

        Ok(PathBuf::from(&output_path))
    }

    /// Downloads the file at the given `url` serially.
    pub async fn sequential(&self, url: &str) -> Result<PathBuf, DownloadError> {
        let mut stream = self.client.get(url).send().await?.bytes_stream();
        let output_path = self.get_output_path(url);
        let mut file = fs::File::create(&output_path).await?;

        while let Some(chunk) = stream.next().await {
            file.write_all(&chunk?).await?;
        }

        Ok(PathBuf::from(&output_path))
    }

    fn get_output_path(&self, url: &str) -> String {
        let filename = url::Url::parse(url)
            .ok()
            .and_then(|u| {
                u.path_segments()
                    .map(|segments| segments.last().unwrap_or_else(|| "unnamed").to_owned())
            })
            .unwrap_or_else(|| "unnamed".to_owned());

        let p = Path::new(&filename);
        let file_stem = p
            .file_stem()
            .map(|v| v.to_string_lossy().to_string())
            .unwrap_or("unnamed".to_owned());
        let ext = p
            .extension()
            .map(|v| v.to_string_lossy().to_string())
            .unwrap_or("".to_owned());

        let mut output_path = self.output_dir.join(&filename);
        let mut i = 1;

        while output_path.exists() {
            output_path = self
                .output_dir
                .join(format!("{} ({}).{}", &file_stem, i, ext));
            i += 1;
        }

        output_path.to_string_lossy().to_string()
    }
}
