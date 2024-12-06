use sha1::{Sha1, Digest};
use base64::prelude::*;

pub fn compute_sec_websocket_accept(key: &str) -> String {
    let mut sha1 = Sha1::new();
    sha1.update(format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11"));
    BASE64_STANDARD.encode(sha1.finalize())
}