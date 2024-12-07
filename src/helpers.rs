use std::cmp::Ordering;
use std::fs;

use camino::{Utf8Component, Utf8Path, Utf8PathBuf};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

use chrono::*;

use crate::*;
use crate::http::*;

pub fn get_domain_certs(path: &str) -> Vec<CertificateDer> {
    CertificateDer::pem_file_iter(path)
        .unwrap()
        .map(|cert| cert.unwrap())
        .collect()
}

pub fn get_private_key(path: &str) -> PrivateKeyDer<'_> {
    PrivateKeyDer::from_pem_file(path).unwrap()
}

pub fn path_is_sane(path: &Utf8Path) -> bool {
    let mut sane = true;
    for segment in path.components() {
        match segment {
            Utf8Component::Prefix(_) => sane = false,
            Utf8Component::RootDir => sane = false,
            Utf8Component::CurDir => continue,
            Utf8Component::ParentDir => sane = false,
            Utf8Component::Normal(_) => continue,
        };
    }
    return sane;
}

impl From<Status> for Response {
    fn from(status: Status) -> Response {
        Response {
            version: Version::V_1_1,
            status,
            headers: vec![],
            body: "".into(),
        }
    }
}

// impl From<Vec<u8>> for Response {
//     fn from(payload: Vec<u8>) -> Response {
//         Response {
//             version: Version::V_1_1,
//             status: Status::Ok,
//             headers: vec![],
//             body: payload,
//         }
//     }
// }

// impl From<String> for Response {
//     fn from(payload: String) -> Response {
//         payload.into_bytes().into()
//     }
// }

impl From<Utf8PathBuf> for Response {
    //TODO: proper error handling via HTTP status line
    fn from(path: Utf8PathBuf) -> Response {
        Response {
            version: Version::V_1_1,
            status: Status::Ok,
            headers: vec![],
            body: path,
        }
    }
}

pub struct VirtualFile {
    pub root: Utf8PathBuf,
    pub path: Utf8PathBuf,
    pub if_modified_since: Option<DateTime<Utc>>,
}

impl VirtualFile {
    pub fn new(root: Utf8PathBuf, default: &str, request: Request) -> Self {
        Self {
            root,
            path: 
                match request.path == Utf8Path::new("") {
                    true => default.into(),
                    false => request.path,
                },
            if_modified_since: 
                match request.headers.get("if-modified-since") {
                    Some(date) => {
                        let date: DateTime<Utc> = DateTime::parse_from_rfc2822(date)
                            .unwrap()
                            .into();
                        Some(date)
                    },
                    None => None,
            },
        }
    }
}

impl From<VirtualFile> for Response {
    fn from(file: VirtualFile) -> Response {
        if path_is_sane(&file.path) {
            let virtual_path = file.root.join(file.path);
            println!("{:#?}", virtual_path);
            match file.if_modified_since {
                Some(date) => {
                    let last_modified: DateTime<Utc> = fs::metadata(&virtual_path).unwrap().modified().unwrap().into();
                    match date.cmp(&last_modified) {
                        Ordering::Less => virtual_path.into(),
                        Ordering::Equal => Status::NotModified.into(),
                        Ordering::Greater => Status::NotModified.into(),
                    }
                },
                None => virtual_path.into(),
            }
        }
        else {
            ().into()
        }
    }
}

impl From<()> for Response {
    fn from(_: ()) -> Response {
        Response {
            version: Version::V_1_1,
            status: Status::NotFound,
            headers: vec![],
            body: Utf8PathBuf::new(),
        }
    }
}

impl From<Request> for Utf8PathBuf {
    fn from(r: Request) -> Utf8PathBuf {
        r.path
    }
}

pub trait Read2 {
    fn read2(&mut self, buffer: &mut [u8]) -> (usize, io::Result<()>);
}

impl<T: Read> Read2 for T {
    fn read2(&mut self, buffer: &mut [u8]) -> (usize, io::Result<()>) {
        let mut bytes_writ = 0;
        loop {
            match self.read(&mut buffer[bytes_writ..]) {
                Ok(0) if  buffer.is_empty() => return (bytes_writ, Ok(())),
                Ok(0) if !buffer.is_empty() => return (bytes_writ, Err(ErrorKind::ConnectionAborted.into())),
                Ok(bytes) => {
                    bytes_writ += bytes;
                },
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => return (bytes_writ, Err(ErrorKind::WouldBlock.into())),
                Err(ref e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return (bytes_writ, Err(e)),
            }
        };
    }
}

pub trait Write2 {
    fn write2(&mut self, buffer: &[u8]) -> (usize, io::Result<()>);
    fn flush2(&mut self) -> io::Result<()>;
}

impl<T: Write> Write2 for T {
    fn write2(&mut self, buffer: &[u8]) -> (usize, io::Result<()>) {
        let mut bytes_read = 0;
        loop {
            match self.write(&buffer[bytes_read..]) {
                Ok(0) => return (bytes_read, Ok(())),
                Ok(bytes) => {
                    bytes_read += bytes;
                },
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => return (bytes_read, Err(ErrorKind::WouldBlock.into())),
                Err(ref e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return (bytes_read, Err(e)),
            }
        };
    }
    fn flush2(&mut self) -> io::Result<()> {
        self.flush()
    }
}