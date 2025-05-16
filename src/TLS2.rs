use std::{collections::HashMap, io::{self, ErrorKind, Read, Write}, net::SocketAddr, sync::Arc};

use mio::{net::TcpListener, Events, Interest, Poll, Token};
use rustls::ServerConfig;

use crate::{helpers::{Read2}, SERVER, TLS::TLStream};

pub struct TLServer {
    pub config: Arc<ServerConfig>,
    pub listener: TcpListener,
    pub poll: Poll,
    pub events: Events,
    pub clients: HashMap<ClientID, TLStream>,
    pub queued_reader: Option<ClientID>,
    pub events_processed: usize,
}

pub type ClientID = usize;

#[derive(Debug)]
pub enum ServerEvent {
    CLIENT_JOINED,
    CLIENT_READABLE,
    CLIENT_WRITABLE,
    CLIENT_LEFT,
}

impl TLServer {
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
            events_processed: 0,
        }
    }
    pub fn serve(&mut self) -> Result<(ClientID, ServerEvent), io::Error> {
        if let Some(id) = self.queued_reader.take() {
            return Ok((id, ServerEvent::CLIENT_READABLE));
        }
        loop {
            for event in self.events.iter().skip(self.events_processed) {
                self.events_processed += 1;

                match event.token() {
                    SERVER => {
                        match self.listener.accept() {
                            Ok((connection, _)) => {
                                let mut client = TLStream::new(connection, self.config.clone());
                                let id = self.register(&mut client).unwrap();
                                self.clients.insert(id, client);

                                return Ok((id, ServerEvent::CLIENT_JOINED))
                            }
                            Err(e) if e.kind() == ErrorKind::WouldBlock => {},
                            Err(e) => {
                                println!("TLServer: connection refused due to error accepting: {:?}", e);
                            },
                        }
                    }
                    client => {
                        let id = client.0 as ClientID;
                        let stream = self.clients.get_mut(&id).unwrap();
                        if stream.tls.is_handshaking() {
                            match stream.handshake() {
                                Ok(_) => {},
                                Err(e) if e.kind() == ErrorKind::WouldBlock => {},
                                Err(e) if e.kind() == ErrorKind::Interrupted => {},
                                Err(e) => {
                                    println!("TLServer: dropped client on account of error when handshaking: {e}");
                                    self.drop_client(id);
                                    return Ok((id, ServerEvent::CLIENT_LEFT));
                                },
                            };
                            continue
                        }
                        if event.is_writable() {
                            if event.is_readable() {
                                self.queued_reader = Some(id);
                            };
                            match stream.flush() {
                                Ok(_) => {},
                                Err(e) if e.kind() == ErrorKind::WouldBlock => {},
                                Err(e) if e.kind() == ErrorKind::ConnectionAborted => {
                                    return Ok((id, ServerEvent::CLIENT_LEFT));
                                },
                                Err(e) => {
                                    println!("TLServer: dropped client on account of error when flushing: {e}");
                                    self.drop_client(id);
                                    return Ok((id, ServerEvent::CLIENT_LEFT));
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
            self.events_processed = 0;
            self.poll();
        }
    }
    /// returns the amount of bytes written into the buffer
    pub fn read_from_client(&mut self, id: ClientID, buffer: &mut[u8]) -> (u64, Result<(), io::Error>) {
        let stream = self.clients.get_mut(&id).unwrap();
        let (bytes_read, err) = stream.read2(buffer);
        (bytes_read as u64, err)
    }
    pub fn write_to_client(&mut self, id: ClientID, delivery: &mut impl Read) -> io::Result<()> {
        let stream = self.clients.get_mut(&id).unwrap();
        let writ = io::copy(delivery, stream)?;
        stream.flush()?;
        println!("finished with {writ} bytes");
        Ok(())
    }
    pub fn drop_client(&mut self, id: ClientID) {
        let stream = self.clients.get_mut(&id).unwrap();
        
        let registry = self.poll.registry();
        registry.deregister(stream).unwrap();

        self.clients.remove(&id).unwrap();
    }
    pub fn poll(&mut self) {
        match self.poll.poll(&mut self.events, None) {
            Ok(_) => {},
            Err(ref e) if e.kind() == ErrorKind::Interrupted => {},
            Err(e) => panic!("{e}"),
        }
    }
    pub fn register(&mut self, client: &mut TLStream) -> io::Result<ClientID> {
        let registry = self.poll.registry();
        let mut id = fastrand::usize(..);
        while self.clients.contains_key(&id) {
            id = fastrand::usize(..);
        }
        let token = Token(id as usize);
        let interests = Interest::READABLE | Interest::WRITABLE;
        registry.register(client, token, interests)?;

        return Ok(id)
    }
}