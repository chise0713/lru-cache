use std::{
    ffi::OsStr,
    fs::{self, Permissions},
    io::{Error, ErrorKind, Read as _, Result, Write as _},
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

use crate::{Request, Response, helper::evict_raw_nul_separated};

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

    pub fn set_nonblocking(&self, nonblocking: bool) -> Result<()> {
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
    pub fn read_request(&mut self) -> Result<Request> {
        let mut buf = [0u8; 8];
        self.0.read_exact(&mut buf)?;
        Ok(Request::new(u64::from_be_bytes(buf)))
    }

    pub fn send_response(mut self, resp: Response) -> Result<()> {
        self.0.write_all(&evict_raw_nul_separated(&resp))
    }
}

#[repr(transparent)]
pub struct Client(UnixStream);

impl Client {
    pub fn send_request<P: AsRef<Path>>(path: P, req: Request) -> Result<Self> {
        let mut stream = UnixStream::connect(path)?;
        stream.write_all(&req.amount().to_be_bytes())?;
        Ok(Self(stream))
    }

    pub fn read_response(mut self) -> Result<Response> {
        let mut buf = Vec::new();
        self.0.read_to_end(&mut buf)?;

        if buf.is_empty() {
            return Response::new([] as [&Path; 0]);
        }

        let evict: Box<[Box<Path>]> = buf
            .split(|&b| b == 0)
            .map(|s| {
                if s.is_empty() {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        "empty path, leading/trailing NUL",
                    ));
                }
                let path = Path::new(OsStr::from_bytes(s));
                Ok(Box::from(path))
            })
            .collect::<Result<_>>()?;

        Response::new(evict)
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

                let request = accepted.read_request().unwrap();
                assert_eq!(request.amount(), 42);

                let resp = Response::new([Path::new("/foo"), Path::new("/bar")]).unwrap();
                accepted.send_response(resp).unwrap();
            }
        });

        b.wait();
        assert_eq!(
            sock_path.metadata().unwrap().permissions().mode() & 0o7777,
            0o600
        );

        let req = Request::new(42);
        let client = Client::send_request(sock_path, req).unwrap();
        let resp = client.read_response().unwrap();

        let evict_ref: Box<[&Path]> = resp.evict().map(AsRef::as_ref).collect();
        assert_eq!(evict_ref.as_ref(), ["/foo", "/bar"]);

        daemon_thread.join().unwrap();
    }
}
