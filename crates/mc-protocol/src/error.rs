use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("unexpected end of packet data")]
    Eof,
    #[error("VarInt/VarLong too long")]
    VarTooLong,
    #[error("string too long: {len} > {max}")]
    StringTooLong { len: usize, max: usize },
    #[error("invalid UTF-8 in string")]
    InvalidUtf8,
    #[error("invalid data: {0}")]
    Invalid(&'static str),
    #[error("packet too large: {0} bytes")]
    TooLarge(usize),
    #[error("decompression failed: {0}")]
    Decompress(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}
