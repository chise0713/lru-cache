pub mod helper;
pub mod ipc;

use std::{
    io::{self, Error, ErrorKind},
    path::Path,
};

#[derive(Debug)]
pub struct Request {
    amount: u64,
}

impl Request {
    pub fn new(amount: u64) -> Self {
        Self { amount }
    }

    #[inline(always)]
    pub fn amount(&self) -> u64 {
        self.amount
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
