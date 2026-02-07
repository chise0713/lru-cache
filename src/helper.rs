//! helper functions that's not part of main api
use std::os::unix::ffi::OsStrExt as _;

use crate::Response;

pub fn evict_raw_nul_separated(resp: &Response) -> Box<[u8]> {
    let mut buf = Vec::new();
    resp.evict().for_each(|e| {
        buf.extend_from_slice(e.as_os_str().as_bytes());
        buf.push(b'\0');
    });
    if let Some(b) = buf.pop()
        && b != 0
    {
        buf.push(b);
    };
    buf.into_boxed_slice()
}
