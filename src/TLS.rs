use std::{io::{self, ErrorKind, Read, Write}, sync::Arc};

use mio::{event, net::TcpStream};
use rustls::{ServerConnection, ServerConfig};

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
    pub fn handshake(&mut self) -> Result<(), io::Error> {
        let mut eof = false;
        loop {
            let until_handshaked = self.tls.is_handshaking();
            while self.tls.wants_write() {
                if self.tls.write_tls(&mut self.tcp)? == 0 {
                    // EOF
                    self.tcp.flush()?;
                    return Ok(())
                }
            }
            self.tcp.flush()?;
            
            if !until_handshaked {
                return Ok(());
            }

            while !eof && self.tls.wants_read() {
                match self.tls.read_tls(&mut self.tcp) {
                    Ok(0) => eof = true,
                    Ok(_) => break,
                    Err(ref err) if err.kind() == io::ErrorKind::Interrupted => {},
                    Err(err) => return Err(err),
                };
            }
            match self.tls.process_new_packets() {
                Ok(_) => {},
                Err(e) => {
                    println!("rustls error: {e}");
                    return Err(ErrorKind::ConnectionAborted.into())
                },
            };

            if until_handshaked && !self.tls.is_handshaking() && self.tls.wants_write() {
                continue
            }
            if eof {
                return Err(io::Error::from(io::ErrorKind::UnexpectedEof))
            }
        }
    }
}

impl Write for TLStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        while self.tls.wants_write() {
            match self.tls.write_tls(&mut self.tcp) {
                Ok(0) => return Ok(0),
                Ok(_) => {},
                Err(ref e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(ref e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
        };
        self.tls.writer().write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        while self.tls.wants_write() {
            match self.tls.write_tls(&mut self.tcp) {
                Ok(0) => return Err(ErrorKind::ConnectionAborted.into()),
                Ok(_) => continue,
                Err(e) if e.kind() == ErrorKind::WouldBlock => return Err(ErrorKind::WouldBlock.into()),
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
        };
        Ok(())
    }
}

impl Read for TLStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        while self.tls.wants_read() {
            match self.tls.read_tls(&mut self.tcp) {
                Ok(0) => return Ok(0),
                Ok(_) => {},
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
            match self.tls.process_new_packets() {
                Ok(_) => {},
                Err(_) => return Err(ErrorKind::ConnectionAborted.into()),
            }
        }
        self.tls.reader().read(buffer)
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