use std::{io::{self, ErrorKind, Write}, sync::Arc};

use mio::{event, net::TcpStream};
use rustls::{ServerConnection, ServerConfig};

use crate::helpers::{Read2, Write2};

pub struct TLStream {
    pub tcp: TcpStream,
    pub tls: ServerConnection,
}

impl TLStream {
    pub fn new(tcp: TcpStream, config: Arc<ServerConfig>) -> Self {
        Self {
            tcp, 
            tls: ServerConnection::new(config).unwrap() 
        }
    }
}

impl Write2 for TLStream {
    fn write2(&mut self, buffer: &[u8]) -> (usize, io::Result<()>) {
        let mut bytes_writ = 0;
        loop {
            let mut tcp_used = false;
            let mut tls_used = false;
            while self.tls.wants_write() {
                match self.tls.write_tls(&mut self.tcp) {
                    Ok(0) => {return (bytes_writ, Err(ErrorKind::ConnectionAborted.into()))},
                    Ok(_) => { tls_used = true },
                    Err(ref e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(ref e) if e.kind() == ErrorKind::Interrupted => continue,
                    Err(e) => return (bytes_writ, Err(e)),
                };
            };
            let (bytes, error) = self.tls.writer().write2(&buffer[bytes_writ..]);
            if bytes > 0 {
                bytes_writ += bytes;
                tcp_used = true;
            }
            match error {
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => {},
                Err(e) => return (bytes_writ, Err(e)),
                _ => {},
            }
            if !tcp_used && !tls_used {
                if self.tls.wants_write() {
                    return (bytes_writ, Err(ErrorKind::WouldBlock.into()));
                }
                else {
                    return (bytes_writ, Ok(()));
                }
            }
        }
    }

    fn flush2(&mut self) -> io::Result<()> {
        while self.tls.wants_write() {
            match self.tls.write_tls(&mut self.tcp) {
                Ok(0) => return Err(ErrorKind::ConnectionAborted.into()),
                Ok(_) => continue,
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => return Err(ErrorKind::WouldBlock.into()),
                Err(ref e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
        };
        //TODO: is this really necessary?
        self.tls.writer().flush()?;
        Ok(())
    }
}

impl Read2 for TLStream {
    fn read2(&mut self, buffer: &mut [u8]) -> (usize, io::Result<()>) {
        let mut bytes_read = 0;
        loop {
            while self.tls.wants_read() {
                match self.tls.read_tls(&mut self.tcp) {
                    Ok(0) => return (bytes_read, Err(ErrorKind::ConnectionAborted.into())),
                    Ok(_) => {},
                    Err(ref e) if e.kind() == ErrorKind::WouldBlock => return (bytes_read, Err(ErrorKind::WouldBlock.into())),
                    Err(ref e) if e.kind() == ErrorKind::Interrupted => continue,
                    Err(e) => return (bytes_read, Err(e)),
                }
                match self.tls.process_new_packets() {
                    Ok(_) => {},
                    Err(_) => return (bytes_read, Err(ErrorKind::ConnectionAborted.into())),
                }
            }
            let (bytes, error) = self.tls.reader().read2(&mut buffer[bytes_read..]);
            bytes_read += bytes;
            match error {
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => return (bytes_read, Err(ErrorKind::WouldBlock.into())),
                Err(e) => return (bytes_read, Err(e)),
                _ => {},
            }
            if !self.tls.wants_read() { return (bytes_read, Ok(())) }
        }
    }
}

impl event::Source for TLStream {
    fn register(
            &mut self,
            registry: &mio::Registry,
            token: mio::Token,
            interests: mio::Interest,
        ) -> io::Result<()> {
        self.tcp.register(registry, token, interests)
    }

    fn deregister(&mut self, registry: &mio::Registry) -> io::Result<()> {
        self.tcp.deregister(registry)
    }

    fn reregister(
            &mut self,
            registry: &mio::Registry,
            token: mio::Token,
            interests: mio::Interest,
        ) -> io::Result<()> {
        self.tcp.reregister(registry, token, interests)
    }
}