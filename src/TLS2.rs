use std::{collections::HashMap, io::{self, ErrorKind}, net::SocketAddr, sync::Arc};

use mio::{net::TcpListener, Events, Interest, Poll, Token};
use rustls::ServerConfig;

use crate::{helpers::{Read2, Write2}, SERVER, TLS::TLStream};

pub struct TLServer2 {
    config: Arc<ServerConfig>,
    listener: TcpListener,
    poll: Poll,
    events: Events,
    clients: HashMap<u64, TLStream>,
    queued_reader: Option<u64>,
}

pub enum ServerEvent {
    CLIENT_JOINED,
    CLIENT_READABLE,
    CLIENT_WRITABLE,
    CLIENT_LEFT,
}

impl TLServer2 {
    pub fn new(address: SocketAddr, config: ServerConfig) -> Self {
        let poll = Poll::new().unwrap();
        let registry = poll.registry();
        let mut listener = TcpListener::bind(address).unwrap();

        registry.register(&mut listener, SERVER, Interest::READABLE | Interest::WRITABLE).unwrap();

        Self {
            config: Arc::new(config),
            listener,
            poll,
            events: Events::with_capacity(64),
            clients: HashMap::with_capacity(1024),
            queued_reader: None,
        }
    }
    pub fn serve(&mut self) -> Result<(u64, ServerEvent), io::Error> {
        if let Some(id) = self.queued_reader.take() {
            return Ok((id, ServerEvent::CLIENT_READABLE));
        }
        loop {
            self.poll();
            for event in self.events.iter() {
                match event.token() {
                    SERVER => {
                        match self.listener.accept() {
                            Ok((connection, _)) => {
                                let mut id = fastrand::u64(..);
                                while self.clients.contains_key(&id) {
                                    id = fastrand::u64(..);
                                }
                                let mut stream = TLStream::new(connection, self.config.clone());
                                let token = Token(id as usize);
                                let interests = Interest::READABLE | Interest::WRITABLE;

                                let registry = self.poll.registry();
                                registry.register(&mut stream, token, interests).unwrap();

                                self.clients.insert(id, stream);

                                return Ok((id, ServerEvent::CLIENT_JOINED))
                            }
                            Err(e) if e.kind() == ErrorKind::WouldBlock => {},
                            Err(e) => {
                                println!("TCPSERVER: connection refused due to error accepting: {:?}", e);
                            },
                        }
                    }
                    token => {
                        let id = token.0 as u64;
                        let stream = self.clients.get_mut(&id).unwrap();
                        if event.is_writable() {
                            if event.is_readable() {
                                self.queued_reader = Some(id);
                            };
                            match stream.flush2() {
                                Ok(_) => {},
                                Err(e) if e.kind() == ErrorKind::WouldBlock => {},
                                Err(e) if e.kind() == ErrorKind::ConnectionAborted => {
                                    return Ok((id, ServerEvent::CLIENT_LEFT));
                                },
                                Err(e) => {
                                    println!("TLS_SERVER: dropped client on account of error: {e}");
                                    self.drop_client(id);
                                },
                            } 
                            return Ok((id, ServerEvent::CLIENT_WRITABLE))
                        }
                        if event.is_readable() {
                            return Ok((id, ServerEvent::CLIENT_READABLE))
                        }
                    }
                }
            }
        }
    }
    /// returns the amount of bytes written into the buffer
    pub fn read_from_client<'buf>(&mut self, id: u64, buffer: &'buf mut[u8]) -> (u64, Result<(), io::Error>) {
        let stream = self.clients.get_mut(&id).unwrap();
        let (bytes_read, err) = stream.read2(buffer);
        (bytes_read as u64, err)
    }
    pub fn write_to_client(&mut self, id: u64, buffer: &[u8]) -> (u64, Result<(), io::Error>) {
        let stream = self.clients.get_mut(&id).unwrap();
        let (bytes_writ, err) = stream.write2(buffer);
        (bytes_writ as u64, err)
    }
    pub fn drop_client(&mut self, id: u64) {
        let stream = self.clients.get_mut(&id).unwrap();
        
        let registry = self.poll.registry();
        registry.deregister(stream).unwrap();

        self.clients.remove(&id).unwrap();
    }
    fn poll(&mut self) {
        match self.poll.poll(&mut self.events, None) {
            Ok(_) => {},
            Err(ref e) if e.kind() == ErrorKind::Interrupted => {},
            Err(e) => panic!("{e}"),
        }
    }
}