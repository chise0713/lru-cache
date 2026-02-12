use std::{
    ffi::OsStr,
    fs::{self, Permissions},
    io::{Error, ErrorKind, IoSlice, Read as _, Result, Write as _},
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

use nix::libc::PATH_MAX;

use crate::{NUL, Request, Response};

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
        let mut amount_buf: [u8; _] = 0u64.to_be_bytes();
        self.0.read_exact(&mut amount_buf)?;
        let amount = u64::from_be_bytes(amount_buf);

        let mut path_len_buf: [u8; _] = 0u16.to_be_bytes();
        self.0.read_exact(&mut path_len_buf)?;
        let path_len = u16::from_be_bytes(path_len_buf);

        if path_len as usize > PATH_MAX as usize {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "path length exceed `PATH_MAX`",
            ));
        }

        let mut buf = unsafe { Box::new_zeroed_slice(path_len as usize).assume_init() };
        self.0.read_exact(&mut buf)?;
        let directory = Path::new(OsStr::from_bytes(&buf));

        Request::new(amount, directory)
    }

    pub fn send_response(mut self, resp: Response) -> Result<()> {
        self.0.write_all(resp.as_bytes())
    }
}

#[repr(transparent)]
pub struct Client(UnixStream);

impl Client {
    pub fn send_request<P: AsRef<Path>>(path: P, req: Request) -> Result<Self> {
        let mut stream = UnixStream::connect(path)?;

        let amount_buf = req.amount().to_be_bytes();

        let path_bytes = req.directory().as_os_str().as_bytes();
        let path_len: u16 = path_bytes
            .len()
            .try_into()
            .map_err(|_| Error::new(ErrorKind::InvalidInput, "path too long"))?;
        let path_len_buf = path_len.to_be_bytes();

        let io_slice = &[
            IoSlice::new(&amount_buf),
            IoSlice::new(&path_len_buf),
            IoSlice::new(req.directory().as_os_str().as_bytes()),
        ];

        _ = stream.write_vectored(io_slice)?;

        Ok(Self(stream))
    }

    pub fn read_response(mut self) -> Result<Response> {
        let mut buf = Vec::new();
        self.0.read_to_end(&mut buf)?;

        if buf.is_empty() {
            return Response::new([] as [&Path; 0]);
        }

        if buf.first() == Some(&NUL)
            || buf.last() == Some(&NUL)
            || buf.windows(2).any(|w| w == [NUL, NUL])
        {
            return Err(Error::new(
                ErrorKind::InvalidData,
                "empty path, leading/trailing NUL",
            ));
        }

        let evict = buf
            .split(|&b| b == NUL)
            .map(|s| Path::new(OsStr::from_bytes(s)));

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
        let temp_dir = tempfile::tempdir().unwrap();
        let sock_path = Arc::new(temp_dir.path().join("ipc-test.sock"));

        let b = Arc::new(Barrier::new(2));
        let daemon_thread = thread::spawn({
            let b = b.clone();
            let sock_path = sock_path.clone();
            move || {
                let daemon = Daemon::bind(sock_path.as_ref()).unwrap();

                b.wait();

                let mut accepted = daemon.accept().unwrap();

                let request = accepted.read_request().unwrap();
                assert_eq!(request.amount(), 42);
                assert_eq!(request.directory().as_os_str().as_bytes().len(), 0);

                let resp = Response::new([Path::new("/foo"), Path::new("/bar")]).unwrap();
                accepted.send_response(resp).unwrap();
            }
        });

        b.wait();
        assert_eq!(
            sock_path.metadata().unwrap().permissions().mode() & 0o7777,
            0o600
        );

        let req = Request::new(42, "").unwrap();
        let client = Client::send_request(sock_path.as_ref(), req).unwrap();
        let resp = client.read_response().unwrap();

        let evict_ref: Box<[&Path]> = resp.evict().map(AsRef::as_ref).collect();
        assert_eq!(evict_ref.as_ref(), ["/foo", "/bar"]);

        daemon_thread.join().unwrap();
    }

    #[test]
    fn test_faulty_request() {
        Request::new(42, "\0").unwrap_err();
    }
}
