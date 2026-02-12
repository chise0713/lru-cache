pub mod ipc;

use std::{
    ffi::OsStr,
    io::{self, Error, ErrorKind},
    os::unix::ffi::OsStrExt as _,
    path::Path,
};

const NUL: u8 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Directory<'a> {
    Tag(&'a str),
    Path(&'a Path),
}

impl<'a> Directory<'a> {
    pub fn tag(&self) -> Option<&str> {
        match self {
            Self::Tag(tag) => Some(tag),
            Self::Path(_) => None,
        }
    }

    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Tag(_) => None,
            Self::Path(path) => Some(path),
        }
    }
}

#[derive(Debug)]
enum DirectoryInner {
    Tag(Box<str>),
    Path(Box<Path>),
}

#[derive(Debug)]
pub struct Request {
    amount: u64,
    directory: DirectoryInner,
}

impl Request {
    pub fn new(amount: u64, directory: Directory) -> io::Result<Self> {
        let directory = match directory {
            Directory::Tag(tag) => DirectoryInner::Tag(Box::from(tag)),
            Directory::Path(path) => {
                if path.as_os_str().as_bytes().contains(&NUL) {
                    return Err(Error::new(ErrorKind::InvalidData, "path contains NUL"));
                }
                DirectoryInner::Path(Box::from(path))
            }
        };

        Ok(Self { amount, directory })
    }

    #[inline(always)]
    pub fn amount(&self) -> u64 {
        self.amount
    }

    #[inline(always)]
    pub fn directory(&self) -> Directory<'_> {
        match &self.directory {
            DirectoryInner::Tag(tag) => Directory::Tag(tag),
            DirectoryInner::Path(path) => Directory::Path(path),
        }
    }
}

#[derive(Debug)]
#[repr(transparent)]
pub struct Response {
    evict: Box<[u8]>,
}

impl Response {
    pub fn new<I, P>(evict: I) -> Result<Self, io::Error>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let mut buf = Vec::new();

        for (i, path) in evict.into_iter().enumerate() {
            let path = path.as_ref();

            if !path.is_absolute() {
                return Err(Error::new(ErrorKind::InvalidInput, "path is not absolute"));
            }
            if path.as_os_str().as_bytes().contains(&NUL) {
                return Err(Error::new(ErrorKind::InvalidData, "path contains NUL"));
            }

            if i != 0 {
                buf.push(b'\0');
            }

            buf.extend_from_slice(path.as_os_str().as_bytes());
        }

        Ok(Self {
            evict: buf.into_boxed_slice(),
        })
    }

    pub fn evict(&self) -> impl Iterator<Item = &Path> {
        self.evict
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|slice| Path::new(OsStr::from_bytes(slice)))
    }

    #[inline(always)]
    pub fn as_bytes(&self) -> &[u8] {
        &self.evict
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_ok() {
        let req = Request::new(42, Directory::Path(Path::new("/tmp"))).unwrap();

        assert_eq!(req.amount(), 42);
        assert_eq!(req.directory(), Directory::Path(Path::new("/tmp")));
    }

    #[test]
    fn test_request_rejects_nul() {
        let err = Request::new(1, Directory::Path(Path::new("/tmp\0bad"))).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
    }

    #[test]
    fn test_response_empty() {
        let resp = Response::new([] as [&Path; 0]).unwrap();

        assert_eq!(resp.as_bytes(), b"");
        assert_eq!(resp.evict().count(), 0);
    }

    #[test]
    fn test_response_single() {
        let resp = Response::new(["/tmp"]).unwrap();

        assert_eq!(resp.as_bytes(), b"/tmp");

        let collected: Vec<_> = resp.evict().collect();
        assert_eq!(collected.len(), 1);
        assert_eq!(collected[0], Path::new("/tmp"));
    }

    #[test]
    fn test_response_multiple() {
        let resp = Response::new(["/a", "/b", "/c"]).unwrap();

        assert_eq!(resp.as_bytes(), b"/a\0/b\0/c");

        let collected: Vec<_> = resp.evict().collect();
        assert_eq!(collected.len(), 3);
        assert_eq!(collected[0], Path::new("/a"));
        assert_eq!(collected[1], Path::new("/b"));
        assert_eq!(collected[2], Path::new("/c"));
    }

    #[test]
    fn test_response_rejects_relative() {
        let err = Response::new(["relative/path"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidInput);
    }

    #[test]
    fn test_response_rejects_nul() {
        let err = Response::new([Path::new("/tmp\0bad")]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
    }

    #[test]
    fn test_response_roundtrip_integrity() {
        let paths = ["/var", "/usr/bin", "/home/user"];
        let resp = Response::new(paths).unwrap();

        let roundtrip: Vec<_> = resp.evict().map(|p| p.to_str().unwrap()).collect();

        assert_eq!(roundtrip, paths);
    }
}
