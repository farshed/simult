use thiserror::Error;

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error(transparent)]
    RequestError(#[from] reqwest::Error),

    #[error(transparent)]
    JoinError(#[from] tokio::task::JoinError),

    #[error(transparent)]
    FileWriteError(#[from] std::io::Error),
}
