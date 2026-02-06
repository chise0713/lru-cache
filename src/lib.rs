use std::path::Path;

pub mod ipc;

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
    pub fn new<I, P>(evict: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<Box<Path>>,
    {
        Self {
            evict: evict.into_iter().map(Into::into).collect(),
        }
    }

    pub fn evict(&self) -> impl Iterator<Item = &Path> {
        self.evict.iter().map(AsRef::as_ref)
    }
}
