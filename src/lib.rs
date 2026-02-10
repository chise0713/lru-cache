pub mod helper;
pub mod ipc;

use std::{
    io::{self, Error, ErrorKind},
    os::unix::ffi::OsStrExt as _,
    path::Path,
};

const NUL: u8 = 0;

#[derive(Debug)]
pub struct Request {
    amount: u64,
    directory: Box<Path>,
}

impl Request {
    pub fn new<P: AsRef<Path>>(amount: u64, directory: P) -> io::Result<Self> {
        let directory = directory.as_ref();

        if directory.as_os_str().as_bytes().contains(&NUL) {
            return Err(Error::new(ErrorKind::InvalidData, "path contains NUL"));
        }

        Ok(Self {
            amount,
            directory: Box::from(directory),
        })
    }

    #[inline(always)]
    pub fn amount(&self) -> u64 {
        self.amount
    }

    #[inline(always)]
    pub fn directory(&self) -> &Path {
        &self.directory
    }
}

#[derive(Debug)]
pub struct Response {
    evict: Box<[Box<Path>]>,
}

impl Response {
    pub fn new<I, P>(evict: I) -> Result<Self, io::Error>
    where
        I: IntoIterator<Item = P>,
        P: Into<Box<Path>>,
    {
        let evict: Box<[Box<Path>]> = evict.into_iter().map(Into::into).collect();
        if evict.iter().any(|path| !path.is_absolute()) {
            return Err(Error::new(ErrorKind::InvalidInput, "path is not absolute"))?;
        }
        Ok(Self { evict })
    }

    pub fn evict(&self) -> impl Iterator<Item = &Path> {
        self.evict.iter().map(AsRef::as_ref)
    }
}
