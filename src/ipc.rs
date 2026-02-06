use std::{
    ffi::OsStr,
    fs::{self, Permissions},
    io::{Read as _, Result, Write as _},
    os::{
        fd::{AsFd, BorrowedFd},
        unix::{
            ffi::OsStrExt as _,
            fs::PermissionsExt as _,
            net::{UnixListener, UnixStream},
        },
    },
    path::Path,
};

use crate::{Request, Response};

#[must_use]
pub struct Daemon {
    ln: UnixListener,
    path: Box<Path>,
}

impl Daemon {
    pub fn bind<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let ln = UnixListener::bind(path)?;
        fs::set_permissions(path, Permissions::from_mode(0o600))?;
        Ok(Self {
            ln,
            path: Box::from(path),
        })
    }

    pub fn accept(&self) -> Result<Accepted> {
        Ok(Accepted(self.ln.accept()?.0))
    }

    pub fn set_unblocking(&self, nonblocking: bool) -> Result<()> {
        self.ln.set_nonblocking(nonblocking)
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

impl AsFd for Daemon {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.ln.as_fd()
    }
}

#[repr(transparent)]
pub struct Accepted(UnixStream);

impl Accepted {
    pub fn size(&mut self) -> Result<Request> {
        let mut buf = [0u8; 8];
        self.0.read_exact(&mut buf)?;
        Ok(Request::new(u64::from_be_bytes(buf)))
    }

    pub fn respon(mut self, resp: Response) -> Result<()> {
        let evict = resp.evict();
        let mut buf = Vec::new();
        evict.for_each(|e| {
            buf.extend_from_slice(e.as_os_str().as_bytes());
            buf.push(b'\0');
        });
        if let Some(b) = buf.pop()
            && b != 0
        {
            buf.push(b);
        };
        self.0.write_all(&buf)
    }
}

#[repr(transparent)]
pub struct Client(UnixStream);

impl Client {
    pub fn request<P: AsRef<Path>>(path: P, req: Request) -> Result<Self> {
        let mut stream = UnixStream::connect(path)?;
        stream.write_all(&req.amount().to_be_bytes())?;
        Ok(Self(stream))
    }

    pub fn evict(self) -> Result<Box<[Box<Path>]>> {
        let raw_data = self.raw_data()?;
        let evict = raw_data
            .split(|b| *b == 0)
            .map(|s| Box::from(Path::new(OsStr::from_bytes(s))))
            .collect();
        Ok(evict)
    }

    pub fn raw_data(mut self) -> Result<Box<[u8]>> {
        let mut buf = Vec::new();
        self.0.read_to_end(&mut buf)?;
        Ok(buf.into_boxed_slice())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Barrier},
        thread,
    };

    use super::*;

    #[test]
    fn test_daemon_client_roundtrip() {
        let sock_path = Path::new("/tmp/test_ipc.sock");

        let b = Arc::new(Barrier::new(2));
        let daemon_thread = thread::spawn({
            let b = b.clone();
            move || {
                let daemon = Daemon::bind(sock_path).unwrap();

                b.wait();

                let mut accepted = daemon.accept().unwrap();

                let request = accepted.size().unwrap();
                assert_eq!(request.amount(), 42);

                let resp = Response::new([Path::new("foo"), Path::new("bar")]);
                accepted.respon(resp).unwrap();
            }
        });

        b.wait();
        assert_eq!(
            sock_path.metadata().unwrap().permissions().mode() & 0o7777,
            0o600
        );

        let req = Request::new(42);
        let client = Client::request(sock_path, req).unwrap();
        let evict = client.evict().unwrap();

        let evict_ref: Box<[&Path]> = evict.iter().map(AsRef::as_ref).collect();
        assert_eq!(evict_ref.as_ref(), ["foo", "bar"]);

        daemon_thread.join().unwrap();
    }
}
