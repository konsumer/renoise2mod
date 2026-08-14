use std::io;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to read .xrns file: {0}")]
    Io(#[from] io::Error),

    #[error("failed to open .xrns as a zip archive: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("Song.xml not found inside .xrns archive")]
    MissingSongXml,

    #[error("failed to parse Song.xml: {0}")]
    Xml(#[from] roxmltree::Error),

    #[error("malformed Song.xml: {0}")]
    MalformedSong(String),

    #[error("{0}")]
    Conversion(String),
}

pub type Result<T> = std::result::Result<T, Error>;
