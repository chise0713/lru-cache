use std::{
    ffi::OsStr,
    fs::{self, Permissions},
    io::{Error, ErrorKind, IoSlice, Read as _, Result, Write},
    os::{
        fd::{AsFd, BorrowedFd},
        unix::{
            ffi::OsStrExt as _,
            fs::PermissionsExt as _,
            net::{UnixListener, UnixStream},
        },
    },
    path::Path,
    time::Duration,
};

use crate::{Directory, NUL, Request, Response};

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
        let stream = self.ln.accept()?.0;
        let timeout = Some(Duration::from_secs(5));
        stream.set_read_timeout(timeout)?;
        stream.set_write_timeout(timeout)?;
        Ok(Accepted(stream))
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PackedLen(u16);

impl PackedLen {
    // https://github.com/torvalds/linux/blob/v6.19/include/uapi/linux/limits.h#L13
    const PATH_MAX: usize = 4096;
    const FLAG_TAG: u16 = 1 << 15;
    const LEN_MASK: u16 = !Self::FLAG_TAG;

    pub fn new(len: usize, tag: bool) -> Result<Self> {
        if len > Self::PATH_MAX {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "path length exceed `PATH_MAX`",
            ));
        }

        let len = len as u16;

        let mut value = len;
        if tag {
            value |= Self::FLAG_TAG;
        }

        Ok(Self(value))
    }

    #[expect(clippy::len_without_is_empty)]
    #[inline(always)]
    pub const fn len(self) -> u16 {
        self.0 & Self::LEN_MASK
    }

    #[inline(always)]
    pub const fn tag(self) -> bool {
        self.0 & Self::FLAG_TAG != 0
    }

    #[inline(always)]
    pub const fn as_bytes(self) -> [u8; 2] {
        self.0.to_be_bytes()
    }
}

impl TryFrom<u16> for PackedLen {
    type Error = Error;

    fn try_from(value: u16) -> Result<Self> {
        let len = value & Self::LEN_MASK;
        if len as usize > Self::PATH_MAX {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "path length exceed `PATH_MAX`",
            ));
        }

        Ok(PackedLen(value))
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
        let packed_len = PackedLen::try_from(path_len)?;

        let buf = if packed_len.len() != 0 {
            &mut vec![0; packed_len.len() as usize].into_boxed_slice()
        } else {
            [].as_mut()
        };
        self.0.read_exact(buf)?;

        let directory = if packed_len.tag() {
            Directory::Tag(
                str::from_utf8(buf)
                    .map_err(|_| Error::new(ErrorKind::InvalidData, "tag is not valid UTF-8"))?,
            )
        } else {
            Directory::Path(Path::new(OsStr::from_bytes(buf)))
        };

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

        let amount_buf: [u8; _] = req.amount().to_be_bytes();

        let (packed_len, bytes) = match req.directory() {
            Directory::Tag(tag) => (PackedLen::new(tag.len(), true)?, tag.as_bytes()),
            Directory::Path(path) => (
                PackedLen::new(path.as_os_str().as_bytes().len(), false)?,
                path.as_os_str().as_bytes(),
            ),
        };
        let packed_len_buf = packed_len.as_bytes();

        let mut io_slice = [
            IoSlice::new(&amount_buf),
            IoSlice::new(&packed_len_buf),
            IoSlice::new(bytes),
        ];

        write_all_vectored(&mut stream, &mut io_slice)?;

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

// FIXME: remove this after `Write::write_all_vectored` stablize
// https://doc.rust-lang.org/1.93.1/src/std/io/mod.rs.html#1937-1952
fn write_all_vectored<W: Write>(write: &mut W, mut bufs: &mut [IoSlice<'_>]) -> Result<()> {
    // Guarantee that bufs is empty if it contains no data,
    // to avoid calling write_vectored if there is no data to be written.
    IoSlice::advance_slices(&mut bufs, 0);
    while !bufs.is_empty() {
        match write.write_vectored(bufs) {
            Ok(0) => {
                return Err(Error::new(
                    ErrorKind::WriteZero,
                    "failed to write whole buffer",
                ));
            }
            Ok(n) => IoSlice::advance_slices(&mut bufs, n),
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
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
                assert_eq!(request.directory().path(), Some(Path::new("/foo")));

                let resp = Response::new([Path::new("/foo")]).unwrap();
                accepted.send_response(resp).unwrap();

                let mut accepted = daemon.accept().unwrap();

                let request = accepted.read_request().unwrap();
                assert_eq!(request.amount(), 7);
                assert_eq!(request.directory().tag(), Some("bar"));

                let resp = Response::new([Path::new("/bar")]).unwrap();
                accepted.send_response(resp).unwrap();
            }
        });

        b.wait();
        assert_eq!(
            sock_path.metadata().unwrap().permissions().mode() & 0o7777,
            0o600
        );

        let req = Request::new(42, Directory::Path(Path::new("/foo"))).unwrap();
        let client = Client::send_request(sock_path.as_ref(), req).unwrap();
        let resp = client.read_response().unwrap();

        let evict_ref: Box<[&Path]> = resp.evict().map(AsRef::as_ref).collect();
        assert_eq!(evict_ref.as_ref(), ["/foo"]);

        let req = Request::new(7, Directory::Tag("bar")).unwrap();
        let client = Client::send_request(sock_path.as_ref(), req).unwrap();
        let resp = client.read_response().unwrap();

        let evict_ref: Box<[&Path]> = resp.evict().map(AsRef::as_ref).collect();
        assert_eq!(evict_ref.as_ref(), ["/bar"]);

        daemon_thread.join().unwrap();
    }

    #[test]
    fn test_faulty_request() {
        Request::new(42, Directory::Path(Path::new("\0"))).unwrap_err();
    }

    #[test]
    fn test_packed_len_tag_flag_roundtrip() {
        let p = PackedLen::new(123, true).unwrap();
        assert_eq!(p.len(), 123);
        assert!(p.tag());

        let raw = u16::from_be_bytes(p.as_bytes());
        let decoded = PackedLen::try_from(raw).unwrap();

        assert_eq!(decoded.len(), 123);
        assert!(decoded.tag());
    }

    #[test]
    fn test_packed_len_without_tag() {
        let p = PackedLen::new(321, false).unwrap();
        assert_eq!(p.len(), 321);
        assert!(!p.tag());
    }

    #[test]
    fn test_packed_len_rejects_overflow() {
        let value = 0xFFFF;
        let result = PackedLen::try_from(value);
        assert!(result.is_err());
    }

    #[test]
    fn test_reject_non_utf8_tag() {
        let bad = b"\xFF\xFF";
        let packed = PackedLen::new(2, true).unwrap();
        let mut buf = Vec::new();
        buf.extend_from_slice(&0u64.to_be_bytes());
        buf.extend_from_slice(&packed.as_bytes());
        buf.extend_from_slice(bad);

        let temp_dir = tempfile::tempdir().unwrap();
        let sock_path = temp_dir.path().join("ipc-invalid-tag.sock");

        let daemon = Daemon::bind(&sock_path).unwrap();

        thread::spawn({
            let sock_path = sock_path.clone();
            move || {
                // malfunction mock client
                let mut stream = UnixStream::connect(sock_path).unwrap();
                stream.write_all(&buf).unwrap();
            }
        });

        let mut accepted = daemon.accept().unwrap();
        let err = accepted.read_request().unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
    }
}
